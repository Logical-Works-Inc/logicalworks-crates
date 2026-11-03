//! The dialect reader: which COBOL the source is written in (#777).
//!
//! Three dialects are admitted: IBM Enterprise COBOL, GnuCOBOL and Micro
//! Focus. Detection reads compiler directives only, never the program's
//! meaning, and scores hints per dialect: `PROCESS` and `CBL` lines are IBM's
//! own, `EJECT`, `SKIP1`, `SKIP2` and `TITLE` are IBM listing controls,
//! `>>DEFINE`, `>>IF` and `>>D` are GnuCOBOL's own, `>>CALL-CONVENTION` is
//! Micro Focus's own, `$SET` is GnuCOBOL's and Micro Focus's shared spelling
//! so it scores both, `>>SOURCE` is the shared format directive so it scores
//! GnuCOBOL and IBM together, and `*>` floating comments are GnuCOBOL's. The
//! majority wins; a tie, or no hint at all, is [`Dialect::Unknown`], never a
//! guess at one.
//!
//! A line shaped like a directive that matches no known spelling is an
//! [`UnknownDirective`]: named with its line, collected beside the verdict,
//! and never treated as another dialect.

use super::error::LegacyError;
use super::{LegacyLimits, uppercased};

/// Which COBOL a source is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum Dialect {
    /// IBM Enterprise COBOL.
    IbmEnterprise,
    /// GnuCOBOL.
    Gnu,
    /// Micro Focus.
    MicroFocus,
    /// No directive names a dialect, or two tie.
    Unknown,
}

impl Dialect {
    /// The dialect name the verdict prints: `ibm-enterprise`, `gnucobol`,
    /// `micro-focus` or `unknown`.
    #[must_use]
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::IbmEnterprise => "ibm-enterprise",
            Self::Gnu => "gnucobol",
            Self::MicroFocus => "micro-focus",
            Self::Unknown => "unknown",
        }
    }
}

/// A directive-shaped line no known spelling admits, with its line.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct UnknownDirective {
    /// The directive text, as written.
    pub(crate) text: String,
    /// The one-based physical line that carries it.
    pub(crate) line: usize,
}

/// The IBM listing controls: page and spacing directives no other dialect owns.
const IBM_LISTING: [&str; 4] = ["EJECT", "SKIP1", "SKIP2", "TITLE"];

/// Strips fixed-format sequence columns 1-6, the reader's own column rule.
///
/// A fixed-format line carries its sequence in columns 1-6: when those six
/// bytes are digits or blanks, the code starts after them, so `000100 $SET`
/// reads as `$SET` and `000200>>SOURCE` as `>>SOURCE`. A free-format line
/// never looks like that, so it is returned whole.
fn strip_sequence(line: &str) -> &str {
    let (Some(sequence), Some(code)) = (line.get(..6), line.get(6..)) else {
        return line;
    };
    if code.is_empty() {
        return line;
    }
    if sequence
        .bytes()
        .all(|cell| cell == b' ' || cell.is_ascii_digit())
    {
        code
    } else {
        line
    }
}

/// Whether `line` is shaped like a directive: a `>>` or `$` lead, or an IBM
/// listing control standing alone.
fn is_directive_shaped(line: &str) -> bool {
    let upper = uppercased(line);
    let trimmed = upper.trim_start();
    if trimmed.starts_with(">>") || trimmed.starts_with('$') || trimmed.starts_with("*>") {
        return true;
    }
    let word = match trimmed.split_whitespace().next() {
        Some(word) => word,
        None => return false,
    };
    let bare = match word.strip_suffix('.') {
        Some(rest) => rest,
        None => word,
    };
    if bare == "PROCESS" || bare == "CBL" {
        return true;
    }
    IBM_LISTING.contains(&bare)
}

/// Scores one directive-shaped line: IBM, GnuCOBOL and Micro Focus hints.
///
/// Returns the hints each, and whether the line matched a known spelling. An
/// unmatched directive-shaped line is unknown, not another dialect.
fn score_line(line: &str) -> ((u32, u32, u32), bool) {
    let upper = uppercased(line);
    let trimmed = upper.trim_start();
    if trimmed.starts_with(">>SOURCE") {
        return ((1, 1, 0), true);
    }
    if trimmed.starts_with(">>") {
        if trimmed.starts_with(">>DEFINE")
            || trimmed.starts_with(">>IF")
            || trimmed.starts_with(">>ELSE")
            || trimmed.starts_with(">>END-IF")
            || trimmed.starts_with(">>D ")
            || trimmed == ">>D"
        {
            return ((0, 1, 0), true);
        }
        if trimmed.starts_with(">>CALL-CONVENTION") {
            return ((0, 0, 1), true);
        }
        return ((0, 0, 0), false);
    }
    if trimmed.starts_with("$SET") {
        return ((0, 1, 1), true);
    }
    if trimmed.starts_with('$') {
        return ((0, 0, 0), false);
    }
    let word = match trimmed.split_whitespace().next() {
        Some(word) => word,
        None => return ((0, 0, 0), false),
    };
    let bare = match word.strip_suffix('.') {
        Some(rest) => rest,
        None => word,
    };
    if bare == "PROCESS" || bare == "CBL" || IBM_LISTING.contains(&bare) {
        return ((1, 0, 0), true);
    }
    if trimmed.starts_with("*>") {
        return ((0, 1, 0), true);
    }
    ((0, 0, 0), false)
}

/// Reads the dialect of `source` from its compiler directives.
///
/// # Errors
///
/// [`LegacyError::SourceTooLarge`] past the byte ceiling, or
/// [`LegacyError::TooManyNodes`] when unknown directives outnumber the node
/// ceiling: each unknown is a node the caller must read.
pub(crate) fn detect_dialect(
    source: &str,
    limits: &LegacyLimits,
) -> Result<(Dialect, Vec<UnknownDirective>), LegacyError> {
    limits.check_bytes(source.len())?;
    let mut ibm: u32 = 0;
    let mut gnu: u32 = 0;
    let mut focus: u32 = 0;
    let mut unknown: Vec<UnknownDirective> = Vec::new();
    for (offset, raw) in source.lines().enumerate() {
        let line_no = offset.saturating_add(1);
        let effective = strip_sequence(raw);
        if !is_directive_shaped(effective) {
            continue;
        }
        let ((ibm_hint, gnu_hint, focus_hint), known) = score_line(effective);
        ibm = ibm.saturating_add(ibm_hint);
        gnu = gnu.saturating_add(gnu_hint);
        focus = focus.saturating_add(focus_hint);
        if !known {
            limits.check_nodes(unknown.len().saturating_add(1))?;
            unknown.push(UnknownDirective {
                text: String::from(effective.trim()),
                line: line_no,
            });
        }
    }
    let dialect = if ibm > gnu && ibm > focus {
        Dialect::IbmEnterprise
    } else if gnu > ibm && gnu > focus {
        Dialect::Gnu
    } else if focus > ibm && focus > gnu {
        Dialect::MicroFocus
    } else {
        Dialect::Unknown
    };
    Ok((dialect, unknown))
}
