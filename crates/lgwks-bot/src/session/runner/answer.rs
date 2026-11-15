//! The answer path: one free-text answer judged by the resolver and applied to
//! the session. Split from `runner.rs` (#990).

use std::collections::BTreeMap;

use super::Session;
use crate::session::{NodeId, NodeKind, Question, Resolution, ResourceAxis, SessionError, Verdict};

/// What one answer is being judged against: the ask it answers, the resolver's
/// verdict on it, and the person's own words.
///
/// Gathered so the helpers that apply a verdict take one borrowed value rather
/// than six positional arguments that can be passed in the wrong order.
struct Turn<'a> {
    /// The ask node being answered.
    node_id: &'a str,
    /// The variable a resolved answer binds.
    var: &'a str,
    /// The options the ask offered.
    options: &'a [String],
    /// Where each option leads.
    routes: &'a BTreeMap<String, NodeId>,
    /// The resolver's verdict.
    verdict: &'a Verdict,
    /// What the person said.
    utterance: &'a str,
}

impl Session {
    /// Receipts the verdict, then records the person's answer.
    ///
    /// Every re-ask arm begins with this pair, and the order is the contract: the
    /// receipt is the durable fact that a verdict was reached, and the transcript
    /// line is what a reader sees later. Recording first would put an utterance
    /// in the transcript for a verdict that was never durably reached.
    fn receipt_and_record(&mut self, turn: &Turn<'_>) -> Result<(), SessionError> {
        self.write_receipt(turn.node_id, turn.options, turn.verdict, None, None)?;
        self.record(turn.node_id, "user", turn.utterance)
    }

    /// Applies a resolved verdict: receipt, bind, record, then continue.
    ///
    /// A helper rather than the arm inline: as one block the arm held the
    /// option lookup, the route lookup, the receipt, the binding, the
    /// transcript line and the transition -- six fallible steps in one
    /// statement. The order is the contract `answer` documents above and is
    /// enforced here: a journal that refuses the receipt aborts the answer with
    /// the session untouched.
    fn accept_resolved(&mut self, turn: &Turn<'_>, index: usize) -> Result<(), SessionError> {
        let Turn {
            node_id,
            var,
            options,
            routes,
            verdict,
            utterance,
        } = *turn;
        let Some(option) = options.get(index) else {
            let refusal = Err(SessionError::ResolverReturnedInvalidOption {
                node: node_id.to_owned(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "accept_resolved: returning an error to the caller");
            return refusal;
        };
        let Some(target) = routes.get(option).cloned() else {
            let refusal = Err(SessionError::MissingAskRoute {
                node: node_id.to_owned(),
                option: option.clone(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "accept_resolved: returning an error to the caller");
            return refusal;
        };
        self.write_receipt(
            node_id,
            options,
            verdict,
            Some(option.clone()),
            Some(&target),
        )?;
        self.scope
            .set_from_answer_within(var, option, self.limits.get(ResourceAxis::Value))?;
        self.last_utterance = Some(utterance.to_owned());
        self.record(node_id, "user", utterance)?;
        self.current = Some(target);
        self.drive()
    }

    /// Receipts a verdict, records the answer, then re-asks over `ask`.
    ///
    /// The shape every non-resolved verdict shares. `ask` is the option list the
    /// prompt should carry: the full list for an absent, stale or degraded answer,
    /// and a strictly smaller one for an ambiguous tie. That difference is the
    /// whole reason an ambiguous answer terminates rather than looping, so it is
    /// a parameter here instead of a branch repeated at each call site.
    fn reask(&mut self, turn: &Turn<'_>, ask: &[String]) -> Result<(), SessionError> {
        self.receipt_and_record(turn)?;
        self.record_prompt(turn.node_id, ask)
    }

    /// Applies one resolver verdict to the session.
    ///
    /// Each arm is its own refusal or its own transcript, and reading them side
    /// by side is what makes the differences between them -- a narrowed re-ask,
    /// a withdrawn binding, a degraded cause under its own role -- visible.
    /// Inside `answer` they read as one statement.
    fn apply_resolution(
        &mut self,
        turn: &Turn<'_>,
        resolution: Resolution,
    ) -> Result<(), SessionError> {
        let options = turn.options;
        match resolution {
            Resolution::Resolved { index, .. } => self.accept_resolved(turn, index),
            Resolution::Ambiguous { tied, .. } => {
                // Narrow the re-ask to the options still in play. Repeating the
                // full list is what a two-way verdict forced, and it is why an
                // ambiguous answer could loop: the same utterance resolves the
                // same way every time it is asked, so the session never leaves
                // the node. A re-ask that is strictly smaller terminates.
                let narrowed: Vec<String> = tied
                    .iter()
                    .filter_map(|candidate| options.get(*candidate).cloned())
                    .collect();
                // A narrowed set of one is not a question -- there is nothing
                // to choose between -- so the full list is asked again.
                let ask: &[String] = if narrowed.len() >= 2 {
                    &narrowed
                } else {
                    options
                };
                self.reask(turn, ask)
            }
            Resolution::Absent { .. } => self.reask(turn, options),
            Resolution::StaleAlias {
                question: bound_question,
                option,
            } => {
                // Re-ask, as for `Absent`, but record what was withdrawn under
                // its own role. The resolver is not asked to forget the binding:
                // the seam is read-only by design, and a session that silently
                // rewrote its resolver's learned vocabulary would be editing the
                // audit record it is supposed to be producing. The record names
                // the binding and the question; retiring it is the owner's call.
                // A re-ask is a decision, so it is receipted like every other
                // verdict; the receipt names the withdrawal, and the roles
                // below name it again for whoever reads the transcript.
                self.record_stale_alias(turn.node_id, &bound_question, &option)?;
                self.reask(turn, options)
            }
            Resolution::Degraded { reason } => {
                // Re-ask, as for `Absent`, but record the cause under its own
                // role: a transcript that renders a degraded re-ask exactly as
                // an unclear one is how an operator concludes the person was
                // being difficult while the embedder was down.
                self.record_degraded(turn.node_id, reason)?;
                self.reask(turn, options)
            }
        }
    }

    /// Submit one free-text answer. Unrecognized input is recorded, the same
    /// ask remains current, and the prompt is recorded again.
    ///
    /// Every verdict — resolved, ambiguous, absent, or degraded — writes a
    /// [`DecisionReceipt`](crate::session::DecisionReceipt), because a re-ask is
    /// a decision too and the one an operator most needs to see afterwards.
    ///
    /// The receipt is written through the journal before the transition it
    /// describes is applied, and a journal that refuses it aborts the answer
    /// with [`SessionError::ReceiptNotRecorded`] leaving the session exactly as
    /// it was: not advanced, not terminated, the variable unwritten, the
    /// transcript untouched, and no receipt held. Recording is part of
    /// accepting an answer, not a report written beside it, so the one failure
    /// this ordering leaves behind is a decision that was reached and recorded
    /// whose answer could not then be stored — which is the direction an audit
    /// record must err in, and the opposite of an accepted answer with no
    /// record of why.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::BTreeMap;
    /// use lgwks_bot::SessionError;
    /// use lgwks_bot::session::{FlowBounds, FlowSpec, NodeKind, Session, Terminal, VarType};
    ///
    /// let options = vec!["small".to_owned(), "large".to_owned()];
    /// let vars = BTreeMap::from([("size".to_owned(), VarType::Choice(options.clone()))]);
    /// let routes = BTreeMap::from([
    ///     ("small".to_owned(), "end".to_owned()),
    ///     ("large".to_owned(), "end".to_owned()),
    /// ]);
    /// let ask = NodeKind::Ask { var: "size".to_owned(), options, routes };
    /// let nodes = BTreeMap::from([("ask".to_owned(), ask), ("end".to_owned(), NodeKind::End)]);
    /// let flow = FlowSpec::from_nodes(vars, "ask", nodes, FlowBounds::new(8))?;
    /// let mut session = Session::new("demo", flow)?;
    ///
    /// session.answer("small")?;
    /// assert_eq!(session.terminal(), Some(&Terminal::Completed));
    /// assert!(matches!(session.answer("large"), Err(SessionError::SessionTerminated)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn answer(&mut self, utterance: &str) -> Result<(), SessionError> {
        if self.terminal.is_some() {
            let refusal = Err(SessionError::SessionTerminated);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "answer: returning an error to the caller");
            return refusal;
        }
        let Some(node_id) = self.current.clone() else {
            let refusal = Err(SessionError::SessionNotAwaitingAnswer);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "answer: returning an error to the caller");
            return refusal;
        };
        let Some(NodeKind::Ask {
            var,
            options,
            routes,
        }) = self.flow.node(&node_id).cloned()
        else {
            let refusal = Err(SessionError::SessionNotAwaitingAnswer);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "answer: returning an error to the caller");
            return refusal;
        };
        // The ingress ceiling, and it comes before the step is charged and
        // before the utterance is copied anywhere: a refused line must neither
        // consume budget nor leave a record that reads as part of the
        // conversation. The check is one comparison against a bounded length,
        // so refusing costs nothing that accepting would not have cost more of.
        let utterance_limit = self.limits.get(ResourceAxis::Utterance);
        if utterance.len() > utterance_limit {
            let refusal = Err(SessionError::UtteranceTooLarge {
                bytes: utterance.len(),
                limit: utterance_limit,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "answer: returning an error to the caller");
            return refusal;
        }
        self.charge_step()?;
        let question =
            Question::new(&node_id, &options).with_domain(self.scope.answer_domain(&var));
        let verdict = self.resolver.resolve(utterance, &question);
        let turn = Turn {
            node_id: &node_id,
            var: &var,
            options: &options,
            routes: &routes,
            verdict: &verdict,
            utterance,
        };
        self.apply_resolution(&turn, verdict.resolution().clone())
    }
}
