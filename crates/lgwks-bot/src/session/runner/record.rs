//! What a session keeps as it runs: receipts, charges, transcript records and
//! the rendering that feeds them. Split from `runner.rs` (#990).

use super::Session;
use crate::session::{
    DecisionReceipt, DegradedReason, Interpolate, RECEIPT_VERSION, RecordedDecision, ResourceAxis,
    SessionError, TemplateInterpolator, TranscriptEntry, Verdict, options_revision, prompt_bytes,
};

impl Session {
    /// Build one receipt and write it through the journal, or refuse the
    /// answer.
    ///
    /// The session's own copy is stored only after the sink has taken it, and
    /// the acceptance that comes back is stored beside it: what the journal
    /// said about the receipt is its own fact, and a receipt that carried its
    /// own acceptance would be the session answering a question only the sink
    /// can answer.
    pub(super) fn write_receipt(
        &mut self,
        node_id: &str,
        options: &[String],
        verdict: &Verdict,
        selected: Option<String>,
        route: Option<&str>,
    ) -> Result<(), SessionError> {
        let receipt = DecisionReceipt {
            version: RECEIPT_VERSION,
            session: self.id.clone(),
            flow: self.flow_revision.clone(),
            node: node_id.to_owned(),
            options: options_revision(options),
            resolution: verdict.resolution().clone(),
            provenance: verdict.provenance().clone(),
            selected,
            route: route.map(str::to_owned),
        };
        // The sink takes it first. The session's own copy is stored only once
        // the journal has accepted it, so a refusal leaves no receipt held.
        let acceptance = self.journal.record_decision(&receipt).map_err(|cause| {
            SessionError::ReceiptNotRecorded {
                node: node_id.to_owned(),
                cause,
            }
        })?;
        self.decisions.push(RecordedDecision {
            receipt,
            acceptance,
        });
        Ok(())
    }

    /// Charge one bounded step or return [`SessionError::SessionBudgetExceeded`].
    pub(super) fn charge_step(&mut self) -> Result<(), SessionError> {
        let attempted = self
            .steps
            .checked_add(1)
            .ok_or(SessionError::SessionBudgetExceeded {
                steps: usize::MAX,
                budget: self.flow.bounds.budget,
            })?;
        if attempted > self.flow.bounds.budget {
            let refusal = Err(SessionError::SessionBudgetExceeded {
                steps: attempted,
                budget: self.flow.bounds.budget,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "charge_step: returning an error to the caller");
            return refusal;
        }
        self.steps = attempted;
        Ok(())
    }

    /// Renders `text` and records it as the assistant's turn.
    ///
    /// `say` and `refer` both speak before either continues, and inline they
    /// each rendered and recorded as two statements, so the drive loop carried
    /// four of them across two arms and a reader could not see that the pair is
    /// one step. It matters that the record follows the render: a render that
    /// refuses must leave nothing said.
    pub(in crate::session) fn speak(
        &mut self,
        node_id: &str,
        text: &str,
    ) -> Result<(), SessionError> {
        let rendered = self.render(node_id, text)?;
        self.record(node_id, "assistant", &rendered)
    }

    /// Render one node's template under this session's record ceiling.
    ///
    /// The ceiling is applied to the *computed* expansion, inside the
    /// interpolator, before the output buffer exists. What that buys is the
    /// whole of the amplification fix: a valid document can name a placeholder
    /// a million times, and the cost of discovering that is a million additions
    /// rather than a multi-gigabyte allocation.
    pub(in crate::session) fn render(
        &self,
        node_id: &str,
        template: &str,
    ) -> Result<String, SessionError> {
        TemplateInterpolator::new().interpolate(
            template,
            &self.scope,
            node_id,
            self.limits.get(ResourceAxis::Record),
        )
    }

    /// Charge `bytes` against the session's retention ceiling.
    ///
    /// Charged before the allocation, so a refused write leaves nothing behind
    /// to clean up: the journal has not been told, the transcript has not grown,
    /// and the counter has not moved.
    pub(super) fn charge_retention(&mut self, bytes: usize) -> Result<(), SessionError> {
        let limit = self.limits.get(ResourceAxis::Session);
        let total =
            self.retained
                .checked_add(bytes)
                .ok_or(SessionError::SessionRetentionExceeded {
                    bytes: usize::MAX,
                    limit,
                })?;
        if total > limit {
            let refusal = Err(SessionError::SessionRetentionExceeded {
                bytes: total,
                limit,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "charge_retention: returning an error to the caller");
            return refusal;
        }
        self.retained = total;
        Ok(())
    }

    /// Append one transcript record and forward it to the journal seam.
    ///
    /// Two ceilings, in this order, and both before either sink is touched: the
    /// payload against the per-record ceiling, which is what makes the refusal
    /// name the node whose text is too long, and the whole record against the
    /// aggregate, which is what bounds a conversation the graph lets repeat.
    /// The record's cost counts the path node and the role as well as the text,
    /// because all three are owned copies this session retains.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::RecordTooLarge`] or
    /// [`SessionError::SessionRetentionExceeded`]. Neither has written anything.
    pub(in crate::session) fn record(
        &mut self,
        node_id: &str,
        role: &str,
        text: &str,
    ) -> Result<(), SessionError> {
        let record_bytes = self.limits.get(ResourceAxis::Record);
        if text.len() > record_bytes {
            let refusal = Err(SessionError::RecordTooLarge {
                node: node_id.to_owned(),
                bytes: text.len(),
                limit: record_bytes,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "record: returning an error to the caller");
            return refusal;
        }
        let cost = text
            .len()
            .saturating_add(node_id.len())
            .saturating_add(role.len());
        self.charge_retention(cost)?;
        self.journal.record(node_id, role, text);
        self.transcript.push(TranscriptEntry {
            path_node: node_id.to_owned(),
            role: role.to_owned(),
            text: text.to_owned(),
        });
        Ok(())
    }

    /// Record the standard prompt for an ask node.
    ///
    /// The prompt is built from the node's candidates, so its size is checked
    /// before it is built rather than after: a node with many long options
    /// would otherwise allocate the joined string and *then* discover that the
    /// record ceiling refuses it. `checked_mul` for the separators and
    /// `checked_add` for each option, with the overflow case reported as the
    /// same refusal rather than wrapped.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::RecordTooLarge`] for a prompt over the ceiling.
    pub(in crate::session) fn record_prompt(
        &mut self,
        node_id: &str,
        options: &[String],
    ) -> Result<(), SessionError> {
        let bytes = prompt_bytes(options);
        let record_bytes = self.limits.get(ResourceAxis::Record);
        if bytes > record_bytes {
            let refusal = Err(SessionError::RecordTooLarge {
                node: node_id.to_owned(),
                bytes,
                limit: record_bytes,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "record_prompt: returning an error to the caller");
            return refusal;
        }
        let prompt = format!("Choose one: {}", options.join(", "));
        self.record(node_id, "assistant", &prompt)
    }

    /// Records that the resolver could not reach a verdict.
    ///
    /// A distinct role rather than a second assistant prompt, because the two
    /// failures need different repairs and a transcript is where that is
    /// decided: an absent resolution says the person's words did not fit the
    /// options, and a degraded one says the resolver never got to compare them.
    pub(super) fn record_degraded(
        &mut self,
        node_id: &str,
        reason: DegradedReason,
    ) -> Result<(), SessionError> {
        let text = format!("Resolver unavailable: {reason}");
        self.record(node_id, "resolver-degraded", &text)
    }

    /// Records that a confirmed phrase's option is no longer offered.
    ///
    /// A distinct role for the reason the degraded case has one: this record is
    /// read by whoever maintains the vocabulary, and it has to name both halves
    /// of the broken binding — the question it was confirmed in and the option
    /// that went away — because either one alone is unactionable. "A stale alias
    /// was ignored" tells that reader nothing they can repair.
    pub(super) fn record_stale_alias(
        &mut self,
        node_id: &str,
        question: &str,
        option: &str,
    ) -> Result<(), SessionError> {
        let text = format!("Superseded alias: question {question} no longer offers \"{option}\"");
        self.record(node_id, "resolver-stale-alias", &text)
    }
}
