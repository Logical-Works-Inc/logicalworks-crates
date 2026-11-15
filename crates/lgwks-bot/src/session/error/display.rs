//! How a session-layer failure renders: every untrusted field escaped through
//! the kernel's one adapter.

use std::fmt;
// `write!` and `write_str` into a `String` need `fmt::Write` in scope;
// `Display`'s own `Formatter` already implements it, so this names the same
// sink trait rather than introducing another.
use std::fmt::Write as FmtWrite;

use super::SessionError;
use crate::error::Escaped;
use crate::session::Terminal;

/// Renders a node whose declared terminal contradicts its kind's own outcome.
///
/// Rendered into a buffer and written once: inline, the arm was five fallible
/// writes in one statement, so reading the sentence also meant reading five
/// propagation operators, and a writer that failed part-way through had already
/// emitted a truncated sentence.
fn write_conflicting_declaration(
    formatter: &mut fmt::Formatter<'_>,
    node: &str,
    intrinsic: &Terminal,
    declared: &Terminal,
) -> fmt::Result {
    let mut rendered = String::new();
    write!(rendered, "terminal declaration on node {} ", Escaped(node))?;
    rendered.write_str("contradicts the outcome its kind carries: ")?;
    write_terminal_to(&mut rendered, intrinsic)?;
    rendered.write_str(" declared as ")?;
    write_terminal_to(&mut rendered, declared)?;
    formatter.write_str(&rendered)
}

/// Renders one terminal outcome into any `fmt::Write` sink.
///
/// One rendering against a buffer, so a caller composing a sentence cannot
/// reimplement the escaping and get it subtly wrong.
fn write_terminal_to<W: fmt::Write>(sink: &mut W, terminal: &Terminal) -> fmt::Result {
    match *terminal {
        Terminal::Completed => sink.write_str("completed"),
        Terminal::Referred { ref target } => write!(sink, "referred to {target:?}"),
        Terminal::HandedOff { ref target } => write!(sink, "handed off to {target:?}"),
        Terminal::Refused { ref reason } => write!(sink, "refused because {reason:?}"),
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            // A kernel failure renders as itself: the wrapper adds no words.
            Self::Kernel(ref source) => write!(f, "{source}"),
            Self::FlowTooLarge { bytes, limit } => {
                write!(f, "flow is {bytes} bytes, over the {limit}-byte limit")
            }
            Self::MalformedFlow { ref cause } => {
                write!(f, "malformed flow: {}", Escaped(cause))
            }
            Self::InvalidTransitionTarget {
                ref from,
                ref target,
            } => write!(
                f,
                "flow transition from {} targets missing node {}",
                Escaped(from),
                Escaped(target)
            ),
            Self::MissingAskRoute {
                ref node,
                ref option,
            } => write!(
                f,
                "ask node {} has no route for option {}",
                Escaped(node),
                Escaped(option)
            ),
            Self::VariableNeverWritten { ref name } => {
                write!(f, "declared variable {} is never written", Escaped(name))
            }
            Self::VariableReadBeforeInit { ref node, ref name } => write!(
                f,
                "flow node {} reads variable {} before any reaching assignment",
                Escaped(node),
                Escaped(name)
            ),
            Self::UndeclaredVariable { ref name } => {
                write!(f, "flow reads undeclared variable {}", Escaped(name))
            }
            Self::UnreachableNode { ref node } => {
                write!(f, "flow node {} is unreachable", Escaped(node))
            }
            Self::FlowBudgetExceeded { steps, budget } => {
                write!(
                    f,
                    "flow declares {steps} nodes over its {budget}-step budget"
                )
            }
            Self::UnknownNodeKind { ref node, ref kind } => {
                write!(
                    f,
                    "flow node {} has unknown kind {}",
                    Escaped(node),
                    Escaped(kind)
                )
            }
            Self::MissingTransition { ref node } => {
                write!(f, "flow node {} has no continuation", Escaped(node))
            }
            // `field` is `&'static str` from this crate, so it is not a payload.
            Self::MalformedTemplate { ref node, field } => {
                write!(
                    f,
                    "flow node {} has malformed {field} template",
                    Escaped(node)
                )
            }
            Self::VariableTypeMismatch { ref name } => {
                write!(f, "value for variable {} has the wrong type", Escaped(name))
            }
            // `option` renders through `Debug`, which escapes control characters
            // itself; the identifiers are wrapped, and `expected` is a
            // crate-controlled label rather than a payload.
            Self::AskOptionNotAssignable {
                ref node,
                ref variable,
                ref option,
                expected,
                ref cause,
            } => write!(
                f,
                "ask node {} offers option {option:?} for {expected} variable {}, \
                 which cannot store it: {}",
                Escaped(node),
                Escaped(variable),
                Escaped(cause)
            ),
            // Both outcomes render through `write_terminal_to`, which escapes the
            // two untrusted fields a `Terminal` can carry.
            Self::ConflictingTerminalDeclaration {
                ref node,
                ref intrinsic,
                ref declared,
            } => write_conflicting_declaration(f, node, intrinsic, declared),
            Self::UtteranceTooLarge { bytes, limit } => write!(
                f,
                "answer utterance of {bytes} bytes exceeds the {limit}-byte utterance limit"
            ),
            Self::ValueTooLarge {
                ref variable,
                bytes,
                limit,
            } => write!(
                f,
                "value for variable {} occupies {bytes} bytes, over the {limit}-byte value limit",
                Escaped(variable)
            ),
            // `option` renders through `Debug` and escapes itself; the two
            // identifiers are the payloads that do not.
            Self::AskOptionTooLarge {
                ref node,
                ref variable,
                ref option,
                bytes,
                limit,
            } => write!(
                f,
                "ask node {} offers option {option:?} storing {bytes} bytes for variable {}, \
                 over the {limit}-byte value limit",
                Escaped(node),
                Escaped(variable)
            ),
            Self::RecordTooLarge {
                ref node,
                bytes,
                limit,
            } => write!(
                f,
                "record for node {} is {bytes} bytes, over the {limit}-byte record limit",
                Escaped(node)
            ),
            Self::TemplateExpansionTooLarge {
                ref node,
                bytes,
                limit,
            } => write!(
                f,
                "template on node {} would expand to {bytes} bytes, over the {limit}-byte \
                 record limit",
                Escaped(node)
            ),
            Self::SessionRetentionExceeded { bytes, limit } => write!(
                f,
                "session would retain {bytes} bytes, over the {limit}-byte session limit"
            ),
            // `axis` is a crate-controlled label, not a payload.
            Self::ResourceLimitAboveCeiling {
                axis,
                requested,
                ceiling,
            } => write!(
                f,
                "{} limit of {requested} bytes exceeds the operator's {ceiling}-byte ceiling",
                axis.label()
            ),
            Self::PredicateTypeMismatch => f.write_str("predicate values have incompatible types"),
            Self::VariableUnset { ref name } => {
                write!(f, "variable {} has no value", Escaped(name))
            }
            // `value` renders through `Debug`, which escapes control characters
            // itself; the variable name does not, so only that one is wrapped.
            Self::InvalidVariableValue {
                ref variable,
                ref value,
                ref reason,
            } => write!(
                f,
                // `reason` is untrusted like the other two: it is the decoder's
                // own rendering, and a caller may reach it through an answer that
                // reached the declaration. Escaped for the same reason `value`
                // renders through `Debug` and `variable` through `Escaped`.
                "value {value:?} cannot be assigned to variable {}: {}",
                Escaped(variable),
                Escaped(reason)
            ),
            Self::SessionTerminated => f.write_str("session has already terminated"),
            Self::SessionNotAwaitingAnswer => f.write_str("session is not awaiting an answer"),
            Self::ResolverReturnedInvalidOption { ref node } => {
                write!(
                    f,
                    "resolver returned an invalid option for node {}",
                    Escaped(node)
                )
            }
            Self::ReceiptNotRecorded {
                ref node,
                ref cause,
            } => {
                write!(
                    f,
                    "decision at node {} was not recorded: {}",
                    Escaped(node),
                    Escaped(&cause.to_string())
                )
            }
            Self::SessionBudgetExceeded { steps, budget } => {
                write!(
                    f,
                    "session attempted {steps} steps over its {budget}-step budget"
                )
            }
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            // The wrapper renders as the error it carries, so its cause chain is
            // the carried error's: reporting it as the source too would print
            // one failure twice.
            Self::Kernel(ref source) => source.source(),
            // All other causes are data (`&'static str`, escaped `String`), never
            // a wrapped error; the journal's refusal is rendered into the line.
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
            | Self::SessionBudgetExceeded { .. } => None,
        }
    }
}
