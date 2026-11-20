//! The session layer's error: why a flow was refused at load, or a session
//! refused a step, above the kernel's own [`KernelError`].
//!
//! Defined where it is raised (INV-BOT-ERROR-TYPED, ADR 0061). Every variant
//! here is raised by the flow parser, validator, scope or runner, and
//! [`SessionError::Kernel`] carries a kernel failure unchanged, so `retry_class`
//! of a wrapped failure is the kernel's own.

use self::display::TerminalText;
use super::{JournalError, ResourceAxis, Terminal};
use crate::error::{DispatchCertainty, Escaped, KernelError, RetryClass};

/// Error from loading a flow or driving a session.
///
/// `#[non_exhaustive]`: the vocabulary is expected to grow as the flow language
/// does, and a consumer matching on it must keep a wildcard arm.
///
/// # Examples
///
/// ```
/// use lgwks_bot::session::SessionError;
///
/// let refusal = SessionError::SessionTerminated;
/// assert!(!refusal.to_string().is_empty());
/// ```
#[derive(Debug, lgwks_macros::Error)]
#[non_exhaustive]
pub enum SessionError {
    /// A kernel failure, carried unchanged: the wrapper adds no words, and its
    /// cause chain is the carried error's.
    #[error(transparent)]
    Kernel(#[from] KernelError),
    /// The serialized flow exceeds [`crate::session::MAX_FLOW_BYTES`].
    #[error("flow is {bytes} bytes, over the {limit}-byte limit")]
    FlowTooLarge {
        /// Observed length in bytes.
        bytes: usize,
        /// The applied bound.
        limit: usize,
    },
    /// The serialized flow is not valid JSON for the flow schema.
    #[error("malformed flow: {}", Escaped(cause))]
    MalformedFlow {
        /// The parser's escaped diagnostic.
        cause: String,
    },
    /// A flow transition points at a node that is not in the node map.
    #[error(
        "flow transition from {} targets missing node {}",
        Escaped(from),
        Escaped(target)
    )]
    InvalidTransitionTarget {
        /// The node or field that owns the transition.
        from: String,
        /// The unresolved target node id.
        target: String,
    },
    /// An ask option is missing its route.
    #[error(
        "ask node {} has no route for option {}",
        Escaped(node),
        Escaped(option)
    )]
    MissingAskRoute {
        /// The ask node id.
        node: String,
        /// The option value without a route.
        option: String,
    },
    /// A declared variable has no ask node that writes it.
    #[error("declared variable {} is never written", Escaped(name))]
    VariableNeverWritten {
        /// The declared variable name.
        name: String,
    },
    /// A node reads a variable that no path reaching it has assigned yet.
    ///
    /// Distinct from [`SessionError::VariableNeverWritten`], which is a writer
    /// that does not exist anywhere, and from [`SessionError::VariableUnset`],
    /// which is the same condition found at run time. This one is the
    /// load-time defect where a writer exists but executes only *after* the
    /// read, or only on another branch: the document is accepted, and the
    /// session then fails on its first step with no provider involved.
    #[error(
        "flow node {} reads variable {} before any reaching assignment",
        Escaped(node),
        Escaped(name)
    )]
    VariableReadBeforeInit {
        /// The node performing the read.
        node: String,
        /// The variable read before any reaching assignment.
        name: String,
    },
    /// A flow reads a variable that was not declared.
    #[error("flow reads undeclared variable {}", Escaped(name))]
    UndeclaredVariable {
        /// The variable name that was read.
        name: String,
    },
    /// A node in the flow document cannot be reached from the entry node.
    #[error("flow node {} is unreachable", Escaped(node))]
    UnreachableNode {
        /// The unreachable node id.
        node: String,
    },
    /// The number of flow nodes exceeds the declared execution budget.
    #[error("flow declares {steps} nodes over its {budget}-step budget")]
    FlowBudgetExceeded {
        /// The number of declared nodes.
        steps: usize,
        /// The declared maximum.
        budget: usize,
    },
    /// A node kind in a JSON flow is outside the closed node language.
    #[error("flow node {} has unknown kind {}", Escaped(node), Escaped(kind))]
    UnknownNodeKind {
        /// The node carrying the unknown kind.
        node: String,
        /// The unknown kind label.
        kind: String,
    },
    /// A non-terminal node has no continuation.
    #[error("flow node {} has no continuation", Escaped(node))]
    MissingTransition {
        /// The node without a continuation.
        node: String,
    },
    /// A flow template is syntactically invalid.
    #[error("flow node {} has malformed {field} template", Escaped(node))]
    MalformedTemplate {
        /// The node containing the template.
        node: String,
        /// The template field being checked.
        field: &'static str,
    },
    /// A declared variable is used with a value of the wrong type.
    #[error("value for variable {} has the wrong type", Escaped(name))]
    VariableTypeMismatch {
        /// The variable name.
        name: String,
    },
    /// An ask node offers a candidate that the variable it writes cannot hold.
    ///
    /// A load-time verdict, not a runtime one. Every candidate and the
    /// variable's declared type are known once the document is parsed, so a
    /// flow whose only answers the variable cannot store is refused when the
    /// document is loaded: there is no answer the person could give that would
    /// repair it, and asking them is charging a conversation step for an
    /// authoring error.
    #[error(
        "ask node {} offers option {option:?} for {expected} variable {}, which cannot store \
         it: {}",
        Escaped(node),
        Escaped(variable),
        Escaped(cause)
    )]
    AskOptionNotAssignable {
        /// The ask node carrying the candidate.
        node: String,
        /// The variable the ask writes.
        variable: String,
        /// The candidate option string.
        option: String,
        /// Stable label of the declared type the variable holds.
        expected: &'static str,
        /// Why the candidate cannot be stored.
        cause: String,
    },
    /// A terminal declaration on a `handoff` or `refer` node contradicts the
    /// outcome that node's kind already carries.
    ///
    /// A load-time verdict, and a refusal rather than an override. The node
    /// names the target it hands to or refers to; a declared outcome naming a
    /// different target, or a refusal, is a second statement in the same
    /// document that cannot both be true. Accepting it and executing the node
    /// anyway is how a document that says "this handoff is not authorized"
    /// produces a permissive handoff classification, so the conflict is
    /// refused where it was authored. A declaration that *repeats* the node's
    /// outcome is accepted and is what the session executes.
    #[error(
        "terminal declaration on node {} contradicts the outcome its kind carries: {} declared \
         as {}",
        Escaped(node),
        TerminalText(intrinsic),
        TerminalText(declared)
    )]
    ConflictingTerminalDeclaration {
        /// The node carrying both outcomes.
        node: String,
        /// The outcome the node's kind implies.
        intrinsic: Terminal,
        /// The outcome the document declared for it.
        declared: Terminal,
    },
    /// One answer utterance is larger than the session may accept.
    ///
    /// Checked at the ingress, before the utterance reaches the resolver, the
    /// journal, or the retained transcript, so an oversized line costs one
    /// comparison rather than a copy of itself.
    #[error("answer utterance of {bytes} bytes exceeds the {limit}-byte utterance limit")]
    UtteranceTooLarge {
        /// Bytes offered.
        bytes: usize,
        /// The applied utterance ceiling.
        limit: usize,
    },
    /// A value decoded from an accepted candidate is larger than the scope may
    /// retain.
    #[error(
        "value for variable {} occupies {bytes} bytes, over the {limit}-byte value limit",
        Escaped(variable)
    )]
    ValueTooLarge {
        /// The variable being written.
        variable: String,
        /// Bytes the stored value occupies when rendered.
        bytes: usize,
        /// The applied value ceiling.
        limit: usize,
    },
    /// An ask node offers a candidate whose stored value is larger than the
    /// session may retain.
    ///
    /// The load-time half of [`SessionError::ValueTooLarge`], on the same
    /// argument as [`SessionError::AskOptionNotAssignable`]: the candidate list
    /// and the applied ceiling are both known when the document loads, so an ask
    /// whose answer could never be stored is refused there rather than after the
    /// person has answered it.
    #[error(
        "ask node {} offers option {option:?} storing {bytes} bytes for variable {}, over the \
         {limit}-byte value limit",
        Escaped(node),
        Escaped(variable)
    )]
    AskOptionTooLarge {
        /// The ask node carrying the candidate.
        node: String,
        /// The variable the ask writes.
        variable: String,
        /// The candidate option string.
        option: String,
        /// Bytes the stored value would occupy when rendered.
        bytes: usize,
        /// The applied value ceiling.
        limit: usize,
    },
    /// One rendered record payload is larger than the session may retain.
    #[error(
        "record for node {} is {bytes} bytes, over the {limit}-byte record limit",
        Escaped(node)
    )]
    RecordTooLarge {
        /// The node whose text this is.
        node: String,
        /// Bytes of the rendered payload.
        bytes: usize,
        /// The applied per-record ceiling.
        limit: usize,
    },
    /// A template's expansion would exceed the per-record ceiling.
    ///
    /// Refused before the expansion is allocated. A validated document is
    /// small; what it can *expand to* is not, because a placeholder repeats.
    /// The compiled template's expanded size is therefore computed with
    /// checked arithmetic and compared with the ceiling first, so an
    /// amplification that would materialise a multi-gigabyte string from a
    /// sub-megabyte document costs one checked addition per template part and
    /// allocates nothing.
    #[error(
        "template on node {} would expand to {bytes} bytes, over the {limit}-byte record limit",
        Escaped(node)
    )]
    TemplateExpansionTooLarge {
        /// The node whose template this is.
        node: String,
        /// Bytes the expansion would occupy.
        bytes: usize,
        /// The applied per-record ceiling.
        limit: usize,
    },
    /// A session's retained records have reached its byte ceiling.
    ///
    /// The aggregate bound. The per-record ceiling bounds one write; this one
    /// bounds the sum, so a flow whose graph lets the cursor loop cannot retain
    /// an unbounded conversation out of individually acceptable records.
    #[error("session would retain {bytes} bytes, over the {limit}-byte session limit")]
    SessionRetentionExceeded {
        /// Bytes that would be retained by the refused write.
        bytes: usize,
        /// The applied session ceiling.
        limit: usize,
    },
    /// A configured byte ceiling is larger than the operator's hard ceiling.
    ///
    /// Author- and operator-supplied bounds may tighten a ceiling and may not
    /// raise it: a document cannot grant itself a larger budget than the
    /// embedding application enforces. Refused rather than clamped, because a
    /// flow that asks for ten gigabytes and silently receives the shipped eight
    /// mebibytes has been neither obeyed nor corrected, and the next reader of
    /// that document would have to run it to find out which happened.
    #[error(
        "{} limit of {requested} bytes exceeds the operator's {ceiling}-byte ceiling",
        axis.label()
    )]
    ResourceLimitAboveCeiling {
        /// The byte axis the limit applies to.
        axis: ResourceAxis,
        /// The value the document or caller asked for.
        requested: usize,
        /// The operator's hard ceiling for that axis.
        ceiling: usize,
    },
    /// A predicate could not compare values of different types.
    #[error("predicate values have incompatible types")]
    PredicateTypeMismatch,
    /// A predicate or interpolation needs a variable that has no value yet.
    #[error("variable {} has no value", Escaped(name))]
    VariableUnset {
        /// The variable name.
        name: String,
    },
    /// An answer could not be converted to the declared variable type.
    #[error(
        "value {value:?} cannot be assigned to variable {}: {}",
        Escaped(variable),
        Escaped(reason)
    )]
    InvalidVariableValue {
        /// The variable receiving the answer.
        variable: String,
        /// The answer that failed conversion.
        value: String,
        /// Why the conversion refused.
        ///
        /// Carried rather than dropped: "cannot be assigned to variable x" and
        /// "it is not a decimal integer" are different facts, and a caller that
        /// only sees the first has to guess which of the four reasons it was.
        reason: String,
    },
    /// A session has already reached a terminal outcome.
    #[error("session has already terminated")]
    SessionTerminated,
    /// The current session node is not an ask node.
    #[error("session is not awaiting an answer")]
    SessionNotAwaitingAnswer,
    /// A resolver returned an option index outside the candidate list.
    #[error("resolver returned an invalid option for node {}", Escaped(node))]
    ResolverReturnedInvalidOption {
        /// The current ask node id.
        node: String,
    },
    /// The journal refused to record a decision receipt.
    ///
    /// The decision itself was reachable — the resolver produced a verdict and
    /// the flow had a transition for it — but the audit record for that decision
    /// was not accepted, so the answer was **not** applied. This variant exists
    /// because a receipt and a transition are one event: a session that moved
    /// while nothing recorded why would be a run whose transcript describes
    /// something the run never decided.
    #[error(
        "decision at node {} was not recorded: {}",
        Escaped(node),
        Escaped(&cause.to_string())
    )]
    ReceiptNotRecorded {
        /// The current ask node id.
        node: String,
        /// The journal's own refusal.
        cause: JournalError,
    },
    /// A session attempted to execute more nodes than its flow budget allows.
    #[error("session attempted {steps} steps over its {budget}-step budget")]
    SessionBudgetExceeded {
        /// The number of attempted steps.
        steps: usize,
        /// The flow's declared maximum.
        budget: usize,
    },
}

impl SessionError {
    /// What this failure establishes about the effect it was making.
    ///
    /// A session refusal happens before any effect: every variant of this layer
    /// answers [`DispatchCertainty::Refused`], and a wrapped kernel failure
    /// answers what the kernel answers.
    #[must_use]
    pub(crate) fn dispatch_certainty(&self) -> DispatchCertainty {
        match *self {
            Self::Kernel(ref source) => source.dispatch_certainty(),
            Self::FlowTooLarge { .. }
            | Self::MalformedFlow { .. }
            | Self::InvalidTransitionTarget { .. }
            | Self::MissingAskRoute { .. }
            | Self::VariableNeverWritten { .. }
            | Self::VariableReadBeforeInit { .. }
            | Self::UndeclaredVariable { .. }
            | Self::UnreachableNode { .. }
            | Self::FlowBudgetExceeded { .. }
            | Self::UnknownNodeKind { .. }
            | Self::MissingTransition { .. }
            | Self::MalformedTemplate { .. }
            | Self::VariableTypeMismatch { .. }
            | Self::AskOptionNotAssignable { .. }
            | Self::ConflictingTerminalDeclaration { .. }
            | Self::UtteranceTooLarge { .. }
            | Self::ValueTooLarge { .. }
            | Self::AskOptionTooLarge { .. }
            | Self::RecordTooLarge { .. }
            | Self::TemplateExpansionTooLarge { .. }
            | Self::SessionRetentionExceeded { .. }
            | Self::ResourceLimitAboveCeiling { .. }
            | Self::PredicateTypeMismatch
            | Self::VariableUnset { .. }
            | Self::InvalidVariableValue { .. }
            | Self::SessionTerminated
            | Self::SessionNotAwaitingAnswer
            | Self::ResolverReturnedInvalidOption { .. }
            | Self::ReceiptNotRecorded { .. }
            | Self::SessionBudgetExceeded { .. } => DispatchCertainty::Refused,
        }
    }

    /// What this failure permits a caller to do about it.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::{error::RetryClass, session::SessionError};
    ///
    /// assert_eq!(SessionError::SessionTerminated.retry_class(), RetryClass::Never);
    /// ```
    #[must_use]
    pub fn retry_class(&self) -> RetryClass {
        match *self {
            Self::Kernel(ref source) => source.retry_class(),
            ref other => other.dispatch_certainty().retry_class(),
        }
    }
}

mod display;

#[cfg(test)]
mod tests;
