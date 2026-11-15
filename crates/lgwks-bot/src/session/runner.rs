//! The session runner: the cursor over a validated flow. Split from `session.rs` (#526).
//!
//! The cursor and its node steps live here; the answer path (`answer`) and the
//! record-keeping every step shares (`record`) are the two children.

use super::{
    FlowSpec, Journal, MemoryJournal, NodeId, NodeKind, Predicate, RecordedDecision, Resolver,
    ResourceLimits, SessionError, SessionId, Terminal, TranscriptEntry, VarScope, revision_of,
    validate_flow_within,
};

mod answer;
mod record;

/// Synchronous cursor over one validated flow and its conversation state.
#[non_exhaustive]
pub struct Session {
    /// Stable session identity.
    pub(super) id: SessionId,
    /// Validated flow document owned by this session.
    pub(super) flow: FlowSpec,
    /// Digest of the flow document, computed once at construction.
    flow_revision: String,
    /// Current node, or `None` after terminal completion.
    pub(super) current: Option<NodeId>,
    /// Typed variable scope.
    pub(super) scope: VarScope,
    /// Nodes executed in order, including repeated cycle visits.
    pub(super) visited: Vec<NodeId>,
    /// Transcript mirrored to the journal seam.
    pub(super) transcript: Vec<TranscriptEntry>,
    /// Pluggable free-text resolver.
    pub(super) resolver: Box<dyn Resolver>,
    /// Pluggable record sink.
    pub(super) journal: Box<dyn Journal>,
    /// Decision receipts in insertion order, with how each was accepted.
    decisions: Vec<RecordedDecision>,
    /// Terminal outcome, if the cursor has stopped.
    pub(super) terminal: Option<Terminal>,
    /// Number of charged execution and answer steps.
    pub(super) steps: usize,
    /// Last accepted answer, used by [`NodeKind::Route`].
    last_utterance: Option<String>,
    /// Effective byte ceilings for this session.
    pub(super) limits: ResourceLimits,
    /// Bytes retained so far: every transcript record and visited node id.
    pub(super) retained: usize,
}

impl Session {
    /// Start a session with the default language resolver and memory journal.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::BTreeMap;
    /// use lgwks_bot::session::{FlowBounds, FlowSpec, NodeKind, Session, Terminal};
    ///
    /// let nodes = BTreeMap::from([("end".to_owned(), NodeKind::End)]);
    /// let flow = FlowSpec::from_nodes(BTreeMap::new(), "end", nodes, FlowBounds::new(4))?;
    /// let session = Session::new("demo", flow)?;
    /// assert_eq!(session.terminal(), Some(&Terminal::Completed));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(id: impl Into<SessionId>, flow: FlowSpec) -> Result<Self, SessionError> {
        Self::with_components(
            id,
            flow,
            crate::language::LanguageResolver::new(),
            MemoryJournal::new(),
        )
    }

    /// Start a session with a custom resolver and the default memory journal.
    pub fn with_resolver<R>(
        id: impl Into<SessionId>,
        flow: FlowSpec,
        resolver: R,
    ) -> Result<Self, SessionError>
    where
        R: Resolver + 'static,
    {
        Self::with_components(id, flow, resolver, MemoryJournal::new())
    }

    /// Start a session with a custom resolver, a custom journal, and explicit
    /// byte ceilings.
    ///
    /// The operator's knob. `limits` may be tighter than
    /// [`ResourceLimits::shipped`] and may not be looser; a value above the
    /// shipped ceiling is refused with [`SessionError::ResourceLimitAboveCeiling`]
    /// before the flow is even examined. What the session ends up enforcing is
    /// the narrower of `limits` and whatever the document asks for in its
    /// [`FlowBounds`](super::FlowBounds), per axis, so neither side can widen
    /// what the other narrowed.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::ResourceLimitAboveCeiling`] for a ceiling above the
    /// operator's, and every error [`Self::with_components`] returns.
    pub fn with_components_and_limits<R, J>(
        id: impl Into<SessionId>,
        flow: FlowSpec,
        resolver: R,
        journal: J,
        limits: ResourceLimits,
    ) -> Result<Self, SessionError>
    where
        R: Resolver + 'static,
        J: Journal + 'static,
    {
        // Both sets are checked against the ceiling and then narrowed against
        // each other, in that order: a request above the ceiling is a refusal
        // wherever it comes from, and two in-range requests combine by taking
        // the smaller. Doing it the other way round would let a document's
        // in-range tightening hide an operator's out-of-range one.
        let operator = limits.within_ceiling()?;
        let requested = flow.bounds.effective_resources().within_ceiling()?;
        let effective = operator.narrowed(requested);
        // Re-validated here rather than trusted from construction, because the
        // public `json` façade parses a `FlowSpec` without calling
        // `FlowSpec::from_json`, and because this is the point at which the
        // effective ceilings — the ones this session will actually enforce —
        // become known to validation.
        validate_flow_within(&flow, effective)?;
        let scope = VarScope::new(flow.vars.clone())?;
        let flow_revision = revision_of(&flow)?;
        let mut session = Self {
            id: id.into(),
            current: Some(flow.entry.clone()),
            flow,
            flow_revision,
            scope,
            visited: Vec::new(),
            transcript: Vec::new(),
            resolver: Box::new(resolver),
            journal: Box::new(journal),
            decisions: Vec::new(),
            terminal: None,
            steps: 0,
            last_utterance: None,
            limits: effective,
            retained: 0,
        };
        session.drive()?;
        Ok(session)
    }

    /// Start a session with explicit byte ceilings and the default seams.
    ///
    /// # Errors
    ///
    /// As [`Self::with_components_and_limits`].
    pub fn with_limits(
        id: impl Into<SessionId>,
        flow: FlowSpec,
        limits: ResourceLimits,
    ) -> Result<Self, SessionError> {
        Self::with_components_and_limits(
            id,
            flow,
            crate::language::LanguageResolver::new(),
            MemoryJournal::new(),
            limits,
        )
    }

    /// Start a session with both replacement seams installed.
    pub fn with_components<R, J>(
        id: impl Into<SessionId>,
        flow: FlowSpec,
        resolver: R,
        journal: J,
    ) -> Result<Self, SessionError>
    where
        R: Resolver + 'static,
        J: Journal + 'static,
    {
        Self::with_components_and_limits(id, flow, resolver, journal, ResourceLimits::shipped())
    }

    /// Return the session identity.
    #[must_use]
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// Return the current node id, or `None` once terminal.
    #[must_use]
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Return the current node kind, or `None` once terminal.
    #[must_use]
    pub fn current_node(&self) -> Option<&NodeKind> {
        self.current.as_deref().and_then(|id| self.flow.node(id))
    }

    /// Return the typed variable scope.
    #[must_use]
    pub fn scope(&self) -> &VarScope {
        &self.scope
    }

    /// Return the visited node path.
    #[must_use]
    pub fn visited(&self) -> &[NodeId] {
        &self.visited
    }

    /// Return transcript records in insertion order.
    #[must_use]
    pub fn transcript(&self) -> &[TranscriptEntry] {
        &self.transcript
    }

    /// Return the decision receipts written so far, in insertion order.
    ///
    /// This is the audit record. [`Self::transcript`] is a rendering of the
    /// same events for a person to read; these are what a consumer reconciles
    /// against, and they carry what a rendering cannot: the policy in force,
    /// the model if one was consulted, the candidate set, and where each
    /// receipt was accepted.
    #[must_use]
    pub fn decisions(&self) -> &[RecordedDecision] {
        &self.decisions
    }

    /// Return the digest of the flow document this session is running.
    #[must_use]
    pub fn flow_revision(&self) -> &str {
        &self.flow_revision
    }

    /// Return the terminal outcome, if reached.
    #[must_use]
    pub fn terminal(&self) -> Option<&Terminal> {
        self.terminal.as_ref()
    }

    /// Return the number of charged steps.
    #[must_use]
    pub fn steps(&self) -> usize {
        self.steps
    }

    /// Return the byte ceilings this session enforces.
    ///
    /// The effective set: the operator's shipped ceiling, narrowed by whatever
    /// this session's caller and this flow's [`FlowBounds`](super::FlowBounds)
    /// asked for. Reported because a bound a caller cannot read back is a bound
    /// they have to discover by running into it.
    #[must_use]
    pub const fn limits(&self) -> ResourceLimits {
        self.limits
    }

    /// Return the bytes this session currently retains.
    ///
    /// The sum of every transcript record's payload, path node, and role, plus
    /// every visited node id. Each is an owned allocation the session holds, so
    /// the total is what a caller needs to bound their own memory rather than
    /// an estimate of it.
    #[must_use]
    pub const fn retained_bytes(&self) -> usize {
        self.retained
    }

    /// Advances the session until it needs an answer or reaches an outcome.
    fn drive(&mut self) -> Result<(), SessionError> {
        loop {
            let Some(node_id) = self.current.clone() else {
                return Ok(());
            };
            self.charge_step()?;
            // The path is retained like a record is, and charged like one: a
            // visited id is an owned copy of the node id the document carries,
            // so the same loop that produces unbounded records would otherwise
            // produce an unbounded path out of the same repetition.
            self.charge_retention(node_id.len())?;
            self.visited.push(node_id.clone());
            let Some(kind) = self.flow.node(&node_id).cloned() else {
                let refusal = Err(SessionError::InvalidTransitionTarget {
                    from: "<session>".into(),
                    target: node_id,
                });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "drive: returning an error to the caller");
                return refusal;
            };
            if self.step(&node_id, kind)? {
                return Ok(());
            }
        }
    }

    /// Speaks a `say` node's text, then follows its single continuation.
    ///
    /// Speaking before the transition is the contract: a render that refuses must
    /// leave the session on the node it was already on, not halfway to the next.
    fn speak_then_follow(&mut self, node_id: &str, text: &str) -> Result<(), SessionError> {
        self.speak(node_id, text)?;
        let Some(target) = self.flow.edge_targets(node_id).into_iter().next() else {
            let refusal = Err(SessionError::MissingTransition {
                node: node_id.to_owned(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "speak_then_follow: returning an error to the caller");
            return refusal;
        };
        self.current = Some(target);
        Ok(())
    }

    /// Moves to whichever edge the `branch` node's predicate selects.
    ///
    /// A `branch` has exactly two edges and its predicate chooses between them,
    /// so this is a lookup rather than a decision: the predicate is the whole of
    /// the choice and it has already been validated at load.
    fn take_branch(
        &mut self,
        when: &Predicate,
        then: NodeId,
        otherwise: NodeId,
    ) -> Result<(), SessionError> {
        self.current = Some(if when.evaluate(&self.scope)? {
            then
        } else {
            otherwise
        });
        Ok(())
    }

    /// Speaks a `refer` node's text, then ends with the referral it declares.
    ///
    /// The text is emitted before the outcome is set, and the only declared
    /// outcome a `refer` node can carry is the referral itself -- validation
    /// refuses anything else -- so this text is never spoken for an outcome that
    /// contradicts it.
    pub(super) fn refer(&mut self, node_id: &str, text: &str) -> Result<(), SessionError> {
        self.speak(node_id, text)?;
        self.finish(node_id)
    }

    /// Advances one node, and answers whether the session must stop and wait.
    ///
    /// `Ok(true)` is the ask: the only kind that stops the loop short of an
    /// outcome. Each arm is the one step its kind performs, named below.
    pub(super) fn step(&mut self, node_id: &str, kind: NodeKind) -> Result<bool, SessionError> {
        match kind {
            NodeKind::Say { text } => self.speak_then_follow(node_id, &text),
            NodeKind::Ask { options, .. } => {
                self.record_prompt(node_id, &options)?;
                return Ok(true);
            }
            NodeKind::Branch {
                when,
                then,
                otherwise,
                ..
            } => self.take_branch(&when, then, otherwise),
            // The two terminal kinds that emit nothing before they end.
            // Both read the outcome from the document rather than
            // constructing it from the node, so a declared outcome is
            // executed here and not only checked at load.
            NodeKind::Handoff { .. } | NodeKind::End => self.finish(node_id),
            NodeKind::Refer { text, .. } => self.refer(node_id, &text),
            NodeKind::Route { dispatch, fallback } => {
                self.current = Some(if self.last_utterance.is_some() {
                    dispatch
                } else {
                    fallback
                });
                Ok(())
            }
        }?;
        Ok(false)
    }

    /// End the session with the outcome the document gives one terminal node.
    ///
    /// The one place a session reads a terminal outcome. `drive` reaches this
    /// for every terminal node kind, so the outcome validation checked and the
    /// outcome execution returns are the same value from the same calculation
    /// ([`FlowSpec::effective_terminal`]) and cannot drift: an earlier runner
    /// consulted the terminal map for `End` alone, and a refusal declared on a
    /// `handoff` node was accepted by validation and ignored here.
    fn finish(&mut self, node_id: &str) -> Result<(), SessionError> {
        let Some(outcome) = self.flow.effective_terminal(node_id) else {
            let refusal = Err(SessionError::MalformedFlow {
                cause: format!("node {node_id:?} reached with no terminal outcome"),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "finish: returning an error to the caller");
            return refusal;
        };
        self.terminal = Some(outcome);
        self.current = None;
        Ok(())
    }
}

/// Reports the cursor's position and the session's accounting.
///
/// Manual rather than derived: the session holds its resolver and its journal
/// behind `dyn` seams, so a derive is not available, and it holds the transcript
/// in full, which a derived rendering would print entry by entry. What a reader
/// of a session's `Debug` wants is where the cursor is, what it is holding, and
/// what it has spent — so the collections are printed as the counts that answer
/// that, and `finish_non_exhaustive` marks the omission rather than hiding it.
impl core::fmt::Debug for Session {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("current", &self.current)
            .field("terminal", &self.terminal)
            .field("steps", &self.steps)
            .field("visited", &self.visited.len())
            .field("transcript", &self.transcript.len())
            .field("decisions", &self.decisions.len())
            .field("retained", &self.retained)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}
