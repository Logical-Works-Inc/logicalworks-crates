//! How a failure renders: every untrusted field escaped, once, here.

use std::fmt;

use super::KernelError;

/// Render untrusted text so it cannot forge a log record.
///
/// Control characters are the whole of the problem: a newline ends the record,
/// and an ESC sequence executes in whatever terminal later reads it. Both are
/// rendered as escapes, so the payload is still complete and still readable,
/// but it cannot add a physical line or reach the terminal.
///
/// Backslash is deliberately *not* escaped, for two reasons. It cannot forge
/// anything on its own, and leaving it alone makes this pass idempotent with
/// the constructor-level escaping that `MalformedSpec` and `MalformedFlow`
/// already perform: that escaping turns a real newline into the two characters
/// `\` and `n`, which this pass then leaves exactly as it found them. Escaping
/// backslash would render them `\\n`, and each additional pass would add
/// another. The cost is a presentational ambiguity — a payload holding a
/// literal backslash-then-n reads the same as an escaped newline — and that is
/// a triage nuisance rather than a forgery, which makes it the cheaper of the
/// two prices.
///
/// Applied at the rendering site rather than in the enum, so the structured
/// payload a consumer matches on keeps the original bytes.
///
/// `pub(crate)` because it is one implementation, not one per module: every
/// layer's error renders its untrusted fields through the same adapter rather
/// than with an escaping pass of its own.
pub(crate) struct Escaped<'a>(pub(crate) &'a str);

/// Writes one glyph's escaped form.
///
/// A helper rather than a fourth match arm inline: the loop body was four
/// fallible writes in one statement, so reading the escaping rules also meant
/// reading four propagation operators to see that each arm writes one thing.
fn write_escaped(formatter: &mut fmt::Formatter<'_>, glyph: char) -> fmt::Result {
    match glyph {
        '\n' => formatter.write_str("\\n"),
        '\r' => formatter.write_str("\\r"),
        '\t' => formatter.write_str("\\t"),
        control if control.is_control() => write!(formatter, "\\u{{{:x}}}", u32::from(control)),
        plain => {
            let mut buffer = [0_u8; 4];
            formatter.write_str(plain.encode_utf8(&mut buffer))
        }
    }
}

impl fmt::Display for Escaped<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for glyph in self.0.chars() {
            write_escaped(f, glyph)?;
        }
        Ok(())
    }
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The scrutinee is `*self` so each pattern's type is the enum's own
        // type rather than a reference to it, and non-`Copy` payloads are bound
        // by `ref`.
        match *self {
            // The deficit renders itself; every untrusted field it holds is
            // wrapped there, for the reason `Deficit`'s `Display` states.
            Self::CapabilityDenied { ref deficit } => {
                write!(f, "capability denied: {deficit}")
            }
            Self::DomainError {
                ref domain,
                certainty: _,
                ref cause,
            } => {
                write!(f, "{}: {}", Escaped(domain), Escaped(cause))
            }
            Self::EffectIndeterminate {
                ref domain,
                ref cause,
            } => {
                // Names the indeterminacy, not just the domain: an operator
                // reading this line has to know the effect may be live, because
                // that is what makes the retry unsafe.
                write!(
                    f,
                    "{}: may have taken effect — {}",
                    Escaped(domain),
                    Escaped(cause)
                )
            }
            Self::CredentialExpired {
                ref capabilities,
                expired_at,
                now,
            } => {
                let names: Vec<&str> = capabilities.iter().map(|cap| cap.as_str()).collect();
                write!(
                    f,
                    "the credential expired at {expired_at:?} and this call is at {now:?}; it \
                     covered [{}], and re-granting is the repair",
                    names.join(", ")
                )
            }
            Self::Conflict {
                ref domain,
                ref record,
                expected,
                found,
            } => write!(
                f,
                "{} refused a write to record {}: expected version {expected}, found \
                 version {found}; re-read the record and name the version found",
                Escaped(domain),
                Escaped(record)
            ),
            Self::ActionNotDeclared { action } => write!(
                f,
                "action {action} is not declared by this bot: nothing was changed, and \
                 the key did not come from this bot's pending()"
            ),
            Self::EffectUnsettled { action } => write!(
                f,
                "no attempt on action {action} was begun: the journal records an \
                 earlier attempt on it as dispatched with no outcome, so it may \
                 already be live. Settle it with the key pending() hands back \
                 before anything is sent again"
            ),
            // Matched by reference so the cause renders through its own
            // `Display`: the refusal is read once, here, and the variant keeps
            // the whole of it for a caller that wants to tell the broker's
            // refusal from the journal's.
            Self::EffectRefused { ref cause } => {
                write!(f, "nothing left the process: {cause}")
            }
        }
    }
}

impl std::error::Error for KernelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // All causes are data (`Cap`, `&'static str`, escaped `String`), never
        // a wrapped error: there is no deeper source to forward. String
        // causes are intentional here (see the module header), not a missing
        // `#[from]` impl.
        None
    }
}
