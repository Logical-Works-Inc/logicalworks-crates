//! The invariant register's files, and the size law each one is held to.
//!
//! The register is the root `INVARIANTS.md` plus each crate's `INVARIANTS.md` and
//! `INVARIANTS-<n>.md` parts. Each part keeps to 120 lines counted at 120 columns,
//! so a rule that grows past one line is charged for every line it fills.

use std::path::{Path, PathBuf};

use super::all_of;
use crate::builtins::crate_directories;

/// The width a reader's terminal wraps a register line at.
const COLUMNS: usize = 120;

/// The `.md` size law: at most this many lines, each counted at [`COLUMNS`].
const MAX_LINES: usize = 120;

/// The register's files: the root `INVARIANTS.md`, then each crate's parts,
/// sorted by name. A part the lane did not read would be a rule nobody checks.
pub(super) fn files(root: &Path) -> Vec<PathBuf> {
    let mut found = vec![root.join("INVARIANTS.md")];
    for directory in crate_directories(root) {
        let mut parts: Vec<PathBuf> = match std::fs::read_dir(&directory) {
            Ok(reader) => reader
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_file() && is_register_name(path))
                .collect(),
            Err(error) => {
                lgwks_std::trace::debug!(%error, "a crate directory could not be listed");
                Vec::new()
            }
        };
        parts.sort();
        found.extend(parts);
    }
    found
}

/// Whether `path` is named `INVARIANTS.md` or `INVARIANTS-<digits>.md`.
fn is_register_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    match name
        .strip_prefix("INVARIANTS")
        .and_then(|rest| rest.strip_suffix(".md"))
    {
        Some("") => true,
        Some(part) => part
            .strip_prefix('-')
            .is_some_and(|digits| all_of(digits, |ch| ch.is_ascii_digit())),
        None => false,
    }
}

/// The lines `text` fills at [`COLUMNS`] characters: an empty line is one, and a
/// line of `n` characters is `n / COLUMNS` rounded up.
///
/// A register once held 102 newlines and 76,842 characters, so a law that counted
/// newlines passed a file a reader saw as 688 lines (Director, 2026-11-13).
fn visual_lines(text: &str) -> usize {
    text.lines()
        .map(|line| line.chars().count().div_ceil(COLUMNS).max(1))
        .sum()
}

/// The finding for the register file at `path` when `text` is past the law.
pub(super) fn past_the_law(path: &Path, text: &str) -> Option<String> {
    let height = visual_lines(text);
    (height > MAX_LINES).then(|| {
        format!(
            "{}: {height} lines at {COLUMNS} columns, past the {MAX_LINES}-line law",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::{lane, tree_of, write_fixture};
    use super::{is_register_name, visual_lines};
    use std::error::Error;
    use std::path::Path;

    type TestResult = Result<(), Box<dyn Error>>;

    /// A one-bullet register whose single line is exactly `width` characters.
    fn register_of_width(width: usize) -> String {
        let rule = "- **INV-DEMO-1** A rule. · enforced by: `real_case` ";
        let fill = width.saturating_sub(rule.chars().count());
        format!("{rule}{}\n", "x".repeat(fill))
    }

    #[test]
    fn visual_lines_charge_each_line_its_wrapped_height() {
        let wide = |count: usize, ch: char| ch.to_string().repeat(count);
        assert_eq!(visual_lines(""), 0, "no text fills no line");
        assert_eq!(visual_lines("\n"), 1, "an empty line is still a line");
        assert_eq!(visual_lines("a"), 1, "one character is one line");
        assert_eq!(visual_lines(&wide(120, 'a')), 1, "a full line is one line");
        assert_eq!(visual_lines(&wide(121, 'a')), 2, "one past the width wraps");
        assert_eq!(
            visual_lines(&wide(240, 'a')),
            2,
            "two full widths are two lines"
        );
        assert_eq!(
            visual_lines(&wide(120, 'é')),
            1,
            "characters, not bytes, are counted"
        );
        assert_eq!(
            visual_lines("a\n\nb"),
            3,
            "a blank line between two is counted"
        );
    }

    #[test]
    fn only_invariants_and_its_numbered_parts_are_register_names() {
        for (name, expected) in [
            ("INVARIANTS.md", true),
            ("INVARIANTS-2.md", true),
            ("INVARIANTS-12.md", true),
            ("INVARIANTS-.md", false),
            ("INVARIANTS-x.md", false),
            ("INVARIANTS.txt", false),
            ("invariants.md", false),
            ("INVARIANTS-2.md.bak", false),
            ("README.md", false),
        ] {
            assert_eq!(is_register_name(Path::new(name)), expected, "{name}");
        }
    }

    #[test]
    fn a_register_past_the_law_is_refused_by_its_wrapped_height() -> TestResult {
        let crate_source = ("crates/demo/src/lib.rs", "fn real_case() {}\n");
        let fixture = tree_of(&register_of_width(120 * 120), &[crate_source])?;
        let at_the_law = lane(fixture.path())?;
        assert_eq!(
            at_the_law.code, 0,
            "120 lines is the law: {}",
            at_the_law.detail
        );
        write_fixture(
            fixture.path(),
            "INVARIANTS.md",
            &register_of_width(120 * 120 + 1),
        )?;
        let past = lane(fixture.path())?;
        assert_eq!(past.code, 1, "{}", past.detail);
        assert!(
            past.detail
                .contains("INVARIANTS.md: 121 lines at 120 columns"),
            "{}",
            past.detail
        );
        Ok(())
    }

    #[test]
    fn a_second_crate_register_file_is_read() -> TestResult {
        let part = "crates/demo/INVARIANTS-2.md";
        let fixture = tree_of(
            "",
            &[
                ("crates/demo/src/lib.rs", "fn real_case() {}\n"),
                (
                    part,
                    "- **INV-DEMO-2** A rule. · enforced by: `no_such_case`\n",
                ),
            ],
        )?;
        let unresolved = lane(fixture.path())?;
        assert_eq!(unresolved.code, 1, "{}", unresolved.detail);
        assert!(
            unresolved.detail.contains("INV-DEMO-2: `no_such_case`"),
            "{}",
            unresolved.detail
        );
        write_fixture(fixture.path(), part, &register_of_width(121 * 120))?;
        let oversized = lane(fixture.path())?;
        assert_eq!(oversized.code, 1, "{}", oversized.detail);
        assert!(
            oversized.detail.contains("INVARIANTS-2.md: 121 lines"),
            "{}",
            oversized.detail
        );
        Ok(())
    }
}
