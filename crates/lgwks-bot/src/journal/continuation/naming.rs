//! Where a continuation lives: the successor path and generation a journal's file name derives.

use std::path::{Path, PathBuf};

/// The suffix that turns one journal's file name into the directory its
/// successors live in.
const GENERATION_DIRECTORY_SUFFIX: &str = ".cont";

/// The fixed width of a generation's file name inside that directory.
///
/// Fixed rather than appended, and that is the whole reason the naming uses a
/// directory at all: a chain that grew its name by one token per generation passes
/// 255 characters after fifty continuations, so a run meant to last weeks would
/// stop for a reason that has nothing to do with its journal. A fixed-width ordinal
/// inside a per-journal directory costs the same six characters at generation one
/// and at generation 999,999.
pub(super) const GENERATION_DIGITS: usize = 6;

/// The highest generation a name can hold.
///
/// The width above, so it is a stated bound rather than a wrap: a chain that
/// reaches it is refused by the open rather than pointed at a name that already
/// exists, because "the successor is this file" is not an answer a caller can act on.
const MAX_GENERATION_NAME: u64 = 999_999;

/// Where one journal's continuation lives.
///
/// A successor path is a *property of the file*, not of the run that happens to be
/// holding it: a restarting controller that only has the original path has to be
/// able to find the live journal by following it, and a name derived from anything
/// the process was doing would make that a guess.
///
/// So the rule is two fixed shapes and no counter anybody keeps: a base journal
/// continues into `<name>.cont/000001`, and a generation continues into the next
/// six-digit name beside it. Two tenants sharing one directory therefore keep two
/// directories apart, and a chain of any length keeps names of the same width.
#[must_use]
pub(crate) fn successor_path(path: &Path) -> PathBuf {
    // A path with a file name always has a parent (`""` for a bare name), so the
    // one shape that has neither is a root or an empty path, which names no
    // journal and continues into itself.
    let (Some(name), Some(parent)) = (path.file_name(), path.parent()) else {
        return path.to_path_buf();
    };
    if let Some(number) = name_number(name) {
        let next = number.saturating_add(1).min(MAX_GENERATION_NAME);
        return parent.join(format!("{next:0GENERATION_DIGITS$}"));
    }
    let mut directory = path.as_os_str().to_os_string();
    directory.push(GENERATION_DIRECTORY_SUFFIX);
    PathBuf::from(directory).join(format!("{:0GENERATION_DIGITS$}", 1))
}

/// The journal generation a path names, counted from the base journal.
///
/// One for a base journal. Derived from the name rather than stored, so there is no
/// second place a generation is written down and nothing for two continuations to
/// disagree about.
#[must_use]
pub(crate) fn generation_of(path: &Path) -> u64 {
    // The base journal is generation one and its first successor, `000001`, is
    // generation two: the name counts continuations, the generation counts files.
    // Read from the name alone, `000001` would be generation one, and its own
    // successor's checkpoint would claim the generation it already holds.
    match path.file_name().and_then(name_number) {
        Some(number) if is_generation_path(path) => number.saturating_add(1),
        Some(_) | None => 1,
    }
}

/// The number a generation name spells, or `None` for any other name.
///
/// A fold rather than a parse: [`is_generation_name`] admitted exactly six ASCII
/// digits, so there is no failure left for a parse to report.
fn name_number(name: &std::ffi::OsStr) -> Option<u64> {
    if !is_generation_name(name) {
        return None;
    }
    let number = name.to_string_lossy().bytes().fold(0_u64, |number, digit| {
        number
            .saturating_mul(10)
            .saturating_add(u64::from(digit.saturating_sub(b'0')))
    });
    Some(number)
}

/// Whether a name is one this scheme generates.
///
/// Six ASCII digits, exactly. A base journal whose own file name happens to look
/// like a generation is refused by the continuing constructors rather than
/// tolerated, because a name that can be read two ways is a name whose successor
/// cannot be derived.
fn is_generation_name(name: &std::ffi::OsStr) -> bool {
    let text = name.to_string_lossy();
    text.len() == GENERATION_DIGITS && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether a path is one this scheme generated rather than a base journal.
///
/// A generation is a six-digit name **inside a `.cont` directory**, and both halves
/// are load-bearing: the directory is what a base journal cannot already be, so a
/// base named `000123` beside its journal's files is still a base and is refused,
/// while the successor `bot.jrnl.cont/000001` is this scheme's own.
#[must_use]
pub(crate) fn is_generation_path(path: &Path) -> bool {
    let named = path.file_name().is_some_and(is_generation_name);
    let in_directory = path.parent().and_then(Path::file_name).is_some_and(|name| {
        name.to_string_lossy()
            .ends_with(GENERATION_DIRECTORY_SUFFIX)
    });
    named && in_directory
}

/// Refuse a base journal whose name could be read as a generation.
///
/// The alternative is a chain that quietly continues into a neighbouring number,
/// and "which file is authoritative?" must have exactly one answer.
#[cfg(feature = "wire")]
pub(crate) fn refuse_ambiguous_base(path: &Path) -> Result<(), crate::journal::JournalError> {
    let Some(number) = path.file_name().and_then(name_number) else {
        return Ok(());
    };
    if is_generation_path(path) {
        return Ok(());
    }
    let refusal = Err(crate::journal::JournalError::CapacityExceeded {
        resource: crate::journal::JournalLimitKind::Generation,
        limit: MAX_GENERATION_NAME,
        requested: number,
    });
    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "refuse_ambiguous_base: this name could be read as a generation");
    refusal
}
