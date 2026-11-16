//! How a world-layer failure classifies: what was dispatched, and what a retry
//! may do.

use super::EcsError;
use crate::ecs::EffectEvidence;
use crate::error::{DispatchCertainty, RetryClass};

impl EcsError {
    /// What this failure establishes about the effect it was making.
    ///
    /// Exhaustive over the vocabulary and computed from the value, never from
    /// the rendered cause. A wrapped kernel failure answers what the kernel
    /// answers. Every other variant names a refusal that happens *before* an
    /// effect is attempted — a spec that does not validate, a tick that may not
    /// park the thread it was called on, an erased verb handed the wrong type —
    /// so all of them answer [`DispatchCertainty::Refused`], except the
    /// three that say otherwise below.
    #[must_use]
    pub(crate) fn dispatch_certainty(&self) -> DispatchCertainty {
        match *self {
            Self::Kernel(ref source) => source.dispatch_certainty(),
            // The action ran and the outcome is a fact. Refused means "nothing
            // left the process", and NotDelivered means "a retry is a retry" —
            // both are lies about an effect that already returned.
            Self::EffectUnrecorded { evidence, .. } => match evidence {
                EffectEvidence::Applied => DispatchCertainty::Occurred,
                EffectEvidence::NotApplied => DispatchCertainty::NotDelivered,
            },
            // A cancelled observation read nothing, so nothing left the process
            // on this account and the next tick is a plain retry. Folding it
            // into the `Refused` arm below would make a caller classify a source
            // that recovers on the next tick as one whose wiring will never work.
            Self::PollStalled { .. } => DispatchCertainty::NotDelivered,
            // An unsettled reading is the same fact as a cancelled poll: the
            // value that was to be replaced is still the one the substrate
            // holds, so nothing was committed and a later tick is a plain retry.
            Self::UnstableObservation { .. } => DispatchCertainty::NotDelivered,
            Self::IncompleteSpec { .. }
            | Self::UnregisteredDomain { .. }
            | Self::DuplicateDomain { .. }
            | Self::SpecTooLarge { .. }
            | Self::MalformedSpec { .. }
            | Self::UnsupportedSpecVersion { .. }
            | Self::UnknownCondition { .. }
            | Self::TickInsideRuntime
            | Self::PollDeadlineUnbounded { .. }
            | Self::PollDeadlineExceeded { .. }
            // A rejected credential is a refusal: it happens before any effect
            // is attempted, and it is not repaired by presenting the same
            // authority again.
            | Self::CredentialRejected { .. }
            | Self::EvaluateError { .. }
            | Self::TypeMismatch { .. }
            | Self::PendingTransition { .. }
            | Self::NoSuchWork { .. }
            | Self::EvidenceSuperseded { .. }
            | Self::EvidenceStaleAttempt { .. }
            | Self::EvidenceContradicted { .. } => DispatchCertainty::Refused,
        }
    }

    /// What this failure permits a caller to do about it.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::ecs::EcsError;
    /// use lgwks_bot::error::{KernelError, RetryClass};
    ///
    /// // A wrapped kernel failure keeps the class the kernel gave it.
    /// let lost = EcsError::from(KernelError::EffectIndeterminate {
    ///     domain: String::from("gh::issue"),
    ///     cause: String::from("timed out"),
    /// });
    /// assert_eq!(lost.retry_class(), RetryClass::RequiresEvidence);
    /// assert_eq!(EcsError::TickInsideRuntime.retry_class(), RetryClass::Never);
    /// ```
    #[must_use]
    pub fn retry_class(&self) -> RetryClass {
        // A recording failure never re-enters the action, whatever the outcome
        // was: the append is the only thing left to do, and it is not an effect
        // attempt. `Never` here means "never dispatch again for this failure",
        // not "abandon the entry" — `failure_state` maps this variant to a
        // pending-record hold rather than to terminal abandonment.
        match *self {
            Self::EffectUnrecorded { .. } => RetryClass::Never,
            Self::Kernel(ref source) => source.retry_class(),
            ref other => other.dispatch_certainty().retry_class(),
        }
    }
}
