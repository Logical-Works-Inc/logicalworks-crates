//! How a [`FlowError`] reads to a person: one line, located, injection-safe.

use std::fmt;

use super::FlowError;

impl fmt::Display for FlowError {
    /// `<path>: <what happened>`. Step-supplied text is rendered with `{:?}`,
    /// which escapes control characters, so a reason cannot forge a log line.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Cancelled { ref at } => write!(formatter, "{at}: cancelled"),
            Self::TimedOut { ref at, after } => {
                write!(formatter, "{at}: timed out after {after:?}")
            }
            Self::Exhausted {
                ref at,
                attempts,
                ref last,
            } => write!(
                formatter,
                "{at}: gave up after {attempts} attempts; last: {last}"
            ),
            Self::Throttled {
                ref at,
                attempts,
                ref last,
            } => write!(
                formatter,
                "{at}: retry budget spent after {attempts} attempts; last: {last}"
            ),
            Self::Failed { ref at, ref reason } => write!(formatter, "{at}: failed: {reason:?}"),
            Self::Transient { ref at, ref reason } => {
                write!(formatter, "{at}: failed (transient): {reason:?}")
            }
            Self::Bot { ref at, ref source } => write!(formatter, "{at}: {source}"),
            Self::UndeclaredDomains { ref at, ref needs } => write!(
                formatter,
                "{at}: this flow names domains its registry does not declare: {needs}"
            ),
            Self::Blocked {
                ref at,
                ref deficit,
            } => {
                write!(formatter, "{at}: blocked: {deficit}")
            }
            Self::Refused {
                ref at,
                ref refusal,
                ref provenance,
            } => write!(formatter, "{at}: {refusal} (from {provenance})"),
            Self::Intervention {
                ref at,
                ref intervention,
            } => write!(formatter, "{at}: {intervention}"),
            Self::TooDeep { ref at, limit } => {
                write!(formatter, "{at}: steps nested deeper than {limit}")
            }
            Self::Justified {
                ref at,
                ref rule,
                ref reason,
            } => write!(
                formatter,
                "{at}: justification {rule:?} stopped the run: {reason:?}"
            ),
            Self::Graph { ref at, ref reason } => {
                write!(formatter, "{at}: the graph is not runnable: {reason:?}")
            }
            Self::Parked {
                ref at,
                ref id,
                ref digest,
                expires_at,
            } => write!(
                formatter,
                "{at}: parked awaiting approval {id} for digest {digest}, owed by {expires_at}ns on the host clock"
            ),
            Self::ApprovalDenied { ref at, ref id } => {
                write!(formatter, "{at}: approval {id} was denied")
            }
            Self::ApprovalExpired { ref at, ref id } => {
                write!(
                    formatter,
                    "{at}: approval {id} lapsed before it was decided"
                )
            }
            Self::NeedsApproval { ref at, ref want } => write!(
                formatter,
                "{at}: this action hands an effect out of the process and no approval matches digest {want}; decide one through the host"
            ),
            Self::InvalidTenant { reason } => write!(formatter, "invalid tenant: {reason}"),
            Self::InvalidName { reason } => write!(formatter, "invalid task name: {reason}"),
            Self::InvalidRequestKey { reason } => {
                write!(formatter, "invalid request key: {reason}")
            }
            Self::InvalidBusinessKey { reason } => {
                write!(formatter, "invalid business key: {reason}")
            }
            Self::InvalidBound { what, value, max } => {
                write!(formatter, "{what}: {value} is outside 1..={max}")
            }
            Self::Incompatible { ref at, ref drift } => write!(
                formatter,
                "{at}: refusing to resume under a different definition: {drift}"
            ),
            Self::Store { ref at, ref source } => {
                write!(formatter, "{at}: the run store refused: {source}")
            }
            Self::RunInFlight { run } => write!(
                formatter,
                "run {} is already being resumed on this host; nothing ran, and a resume after it finishes replays what it recorded",
                run.id().to_hex()
            ),
            Self::ResumesSaturated { ceiling } => write!(
                formatter,
                "this host is already resuming {ceiling} runs, the most it takes at once; nothing ran"
            ),
        }
    }
}
