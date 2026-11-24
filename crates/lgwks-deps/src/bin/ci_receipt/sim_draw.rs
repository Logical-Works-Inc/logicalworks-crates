//! The seeded draws the `ci-receipt` sims share: one `Seeded` stream drives
//! every choice, so a seed names one log and one run.

use std::error::Error;
use std::fmt;
use std::num::TryFromIntError;

use lgwks_std::seeded::{DrawError, Seeded};

/// What a sim reports when a precondition did not hold.
pub(crate) type SimResult = Result<(), Box<dyn Error>>;

/// The seeds each sim runs.
pub(crate) const SEEDS: u64 = 24;

/// Why a draw could not be made.
#[derive(Debug)]
pub(crate) enum DrawFailed {
    /// The stream refused the range.
    Range(DrawError),
    /// A drawn number did not fit the type asked for.
    Width(TryFromIntError),
    /// A drawn index was past its list.
    Past,
}

impl fmt::Display for DrawFailed {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Range(ref error) => write!(out, "the stream refused the range: {error}"),
            Self::Width(ref error) => write!(out, "a drawn number did not fit: {error}"),
            Self::Past => out.write_str("a drawn index is past the list"),
        }
    }
}

impl Error for DrawFailed {}

impl From<DrawError> for DrawFailed {
    fn from(error: DrawError) -> Self {
        Self::Range(error)
    }
}

impl From<TryFromIntError> for DrawFailed {
    fn from(error: TryFromIntError) -> Self {
        Self::Width(error)
    }
}

/// A drawn element of `items`.
pub(crate) fn pick<'items, T>(
    draw: &mut Seeded,
    items: &'items [T],
) -> Result<&'items T, DrawFailed> {
    let at = draw.index(items.len())?;
    items.get(at).ok_or(DrawFailed::Past)
}

/// A drawn number below `bound`, which is not 0.
pub(crate) fn below(draw: &mut Seeded, bound: usize) -> Result<usize, DrawFailed> {
    Ok(usize::try_from(draw.below(u64::try_from(bound)?)?)?)
}

/// `length` characters drawn from `alphabet`.
pub(crate) fn word(
    draw: &mut Seeded,
    alphabet: &[char],
    length: usize,
) -> Result<String, DrawFailed> {
    let mut text = String::new();
    for _ in 0..length {
        text.push(*pick(draw, alphabet)?);
    }
    Ok(text)
}

/// `text` behind a drawn prefix form: raw API, `gh run view --log`, or raw with a
/// byte order mark.
pub(crate) fn stamp(draw: &mut Seeded, text: &str) -> Result<String, DrawFailed> {
    let time = "2026-11-24T15:22:46.9285151Z";
    Ok(match below(draw, 3)? {
        0 => format!("{time} {text}"),
        1 => format!("Tests on Linux (workspace, slice:4/10)\tUNKNOWN STEP\t{time} {text}"),
        _ => format!("\u{feff}{time} {text}"),
    })
}

/// `lines` as a log, one drawn prefix form to a line.
pub(crate) fn log_of(draw: &mut Seeded, lines: &[String]) -> Result<String, DrawFailed> {
    let mut log = String::new();
    for text in lines {
        log.push_str(&stamp(draw, text)?);
        log.push('\n');
    }
    Ok(log)
}

/// A colour spelling drawn for `label`: none, the `^[` spelling or the real byte.
pub(crate) fn paint(draw: &mut Seeded, label: &str) -> Result<String, DrawFailed> {
    Ok(match below(draw, 3)? {
        0 => label.to_owned(),
        1 => format!("^[[31;1m{label}^[[0m"),
        _ => format!("\u{1b}[31;1m{label}\u{1b}[0m"),
    })
}
