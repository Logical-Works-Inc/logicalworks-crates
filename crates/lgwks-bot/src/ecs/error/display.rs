//! How a world-layer failure renders: every untrusted field escaped through the
//! kernel's one adapter.

use std::fmt;

use super::EcsError;
use crate::error::Escaped;

impl fmt::Display for EcsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The scrutinee is `*self` so each pattern's type is the enum's own
        // type rather than a reference to it, and non-`Copy` payloads are bound
        // by `ref`. `field` is `&'static str` and the two lengths are `usize`,
        // so those bind by copy from behind the deref.
        match *self {
            // A kernel failure renders as itself: the wrapper adds no words, so
            // a log line reads the same on every layer it crosses.
            Self::Kernel(ref source) => write!(f, "{source}"),
            // One `write!`, so there is a single fallible step and no partially
            // rendered sentence if the formatter refuses part-way. `cause` is
            // escaped for the same reason `field`'s neighbours are: it names
            // something from the spec document.
            Self::IncompleteSpec { field, ref cause } => {
                write!(
                    f,
                    "incomplete bot spec: missing {field}: {}",
                    Escaped(cause)
                )
            }
            Self::UnregisteredDomain { ref domain } => {
                write!(f, "unregistered domain: {}", Escaped(domain))
            }
            Self::DuplicateDomain {
                ref domain,
                role,
                first,
                second,
            } => write!(
                f,
                "duplicate {role} domain {} declared at positions {first} and {second}",
                Escaped(domain)
            ),
            Self::SpecTooLarge { bytes, limit } => {
                write!(f, "bot spec is {bytes} bytes, over the {limit}-byte limit")
            }
            Self::MalformedSpec { ref cause } => {
                write!(f, "malformed bot spec: {}", Escaped(cause))
            }
            Self::UnsupportedSpecVersion { found, supported } => write!(
                f,
                "bot spec declares version {found}, but this build materializes version \
                 {supported} only"
            ),
            Self::UnknownCondition {
                ref condition,
                ref argument_parse,
            } => write!(
                f,
                // `argument_parse` distinguishes an identifier this build does
                // not know from one whose threshold argument does not parse --
                // two different repairs -- so it is rendered rather than dropped.
                "unknown condition {}: not in the supported wire vocabulary, or its threshold \
                 argument did not parse ({})",
                Escaped(condition),
                Escaped(argument_parse)
            ),
            Self::TickInsideRuntime => f.write_str(
                "tick: the synchronous adapter cannot park a thread an async runtime is driving; \
                 await `tick_async` on that runtime, or call the tick outside it",
            ),
            Self::EvaluateError { ref cause } => {
                write!(f, "evaluate: {}", Escaped(cause))
            }
            Self::TypeMismatch {
                site,
                chain,
                expected,
                observed,
            } => match chain {
                Some(index) => write!(
                    f,
                    "{site}: type mismatch on chain {index} — expected {expected}, got {observed}"
                ),
                None => write!(
                    f,
                    "{site}: type mismatch — expected {expected}, got {observed}"
                ),
            },
            Self::PollStalled { chain, deadline } => write!(
                f,
                "source poll for chain {chain} was cancelled at its declared {deadline:?} \
                 per-poll deadline: nothing was read, nothing committed, and the other chains \
                 of this tick were not stopped"
            ),
            Self::CredentialRejected {
                ref domain,
                status,
                ref needs,
            } => write!(
                f,
                "{} was refused by its upstream with HTTP {status}: {needs}",
                Escaped(domain)
            ),
            Self::UnstableObservation {
                ref domain,
                unstable,
            } => write!(
                f,
                "source {} read a subject that did not settle ({}); no reading was committed and \
                 the observation is pending the next tick",
                Escaped(domain),
                unstable
            ),
            Self::PollDeadlineUnbounded { deadline } => write!(
                f,
                "per-poll deadline of {deadline:?} bounds no wait: every poll would be \
                 cancelled before it was first polled"
            ),
            Self::PollDeadlineExceeded { deadline, ceiling } => write!(
                f,
                "per-poll deadline of {deadline:?} is above the {ceiling:?} ceiling: a source \
                 that stops answering would hold the tick for longer than any bound this bot \
                 grants"
            ),
            // The payload's own `Display` renders the cause through `Escaped`,
            // at the point the untrusted bytes are reachable, so this arm wraps
            // the whole report only to keep the boundary uniform.
            Self::PendingTransition {
                ref work,
                outstanding,
            } => write!(
                f,
                "tick left work unfinished: {} ({outstanding} open)",
                Escaped(&work.to_string())
            ),
            Self::NoSuchWork { work } => write!(
                f,
                "no held effect at chain {} entry {}: only an entry whose outcome is \
                 indeterminate, or one that was given up on, can be settled with evidence",
                work.chain(),
                work.entry()
            ),
            // The generation the caller named is rendered first and the one in
            // the slot second, in the order the mistake happened: a reader
            // checking this against their own ledger wants to see their number
            // where they left it, next to the one that replaced it.
            Self::EvidenceSuperseded {
                work,
                named,
                current,
            } => write!(
                f,
                "evidence names binding {named} of chain {} entry {}, which has \
                 been superseded by binding {current}: nothing was changed, re-read \
                 pending() and report the key it hands back now",
                work.chain(),
                work.entry()
            ),
            Self::EvidenceStaleAttempt {
                work,
                reported,
                outstanding,
            } => write!(
                f,
                "evidence was about attempt {} of chain {} entry {}, but that \
                 entry is on attempt {} now: nothing was changed, and the \
                 report is about an attempt that is over rather than one outstanding",
                reported.get(),
                work.chain(),
                work.entry(),
                outstanding.get()
            ),
            Self::EvidenceContradicted {
                work,
                settled,
                submitted,
            } => write!(
                f,
                "evidence {} contradicts {settled} already recorded for chain {} entry \
                 {}: nothing was changed, and repeating the same evidence would have \
                 succeeded",
                submitted,
                work.chain(),
                work.entry()
            ),
            Self::EffectUnrecorded {
                key: _,
                evidence,
                ref cause,
            } => {
                write!(
                    f,
                    "the effect is {evidence} and only its record is missing: {cause}. \
                     Retry the journal append, not the action"
                )
            }
        }
    }
}

impl std::error::Error for EcsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // The wrapper renders as the error it carries, so its cause chain is
        // the carried error's: reporting the carried error as the source
        // too would print one failure twice. Every other cause is data
        // (`&'static str`, escaped `String`), never a wrapped error, so there
        // is no deeper source to forward; string causes are intentional here
        // (see the `error` module header), not a missing `#[from]` impl.
        if let Self::Kernel(ref source) = *self {
            source.source()
        } else {
            None
        }
    }
}
