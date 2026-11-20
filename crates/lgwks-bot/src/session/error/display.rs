//! How a terminal outcome renders inside a session-layer failure line: the two
//! untrusted fields a `Terminal` can carry escaped through `Debug`.

use std::fmt;

use crate::session::Terminal;

/// One terminal outcome as the sentence fragment a refusal line embeds.
///
/// One rendering for every caller composing a sentence, so none can reimplement
/// the escaping and get it subtly wrong.
pub(super) struct TerminalText<'terminal>(pub(super) &'terminal Terminal);

impl fmt::Display for TerminalText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self.0 {
            Terminal::Completed => f.write_str("completed"),
            Terminal::Referred { ref target } => write!(f, "referred to {target:?}"),
            Terminal::HandedOff { ref target } => write!(f, "handed off to {target:?}"),
            Terminal::Refused { ref reason } => write!(f, "refused because {reason:?}"),
        }
    }
}
