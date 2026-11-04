//! The lines a change adds, file by file: the one reader of `git diff
//! --unified=0` that the estate's change-scoped gates share.
//!
//! A gate that holds a change to a bar the older text does not meet yet judges
//! only the lines the change wrote. `check-docs --base` does it for prose and
//! `lgwks-ast scan --base` for the missing-example rule; both read the change
//! here, so the two cannot disagree about which lines it added.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::process::Command;

/// The inclusive 1-based line ranges each file of a unified diff adds, keyed by
/// the path after `+++ b/`, in hunk order.
///
/// A hunk that only deletes adds no range, and a deleted file (`+++
/// /dev/null`) adds no entry. A header it cannot read is skipped rather than
/// guessed at, so a malformed diff narrows what is judged and never widens it.
fn added_lines(diff: &str) -> BTreeMap<String, Vec<(usize, usize)>> {
    let mut found: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            current = path.strip_prefix("b/").map(str::to_owned);
        } else if let (Some(hunk), Some(name)) = (line.strip_prefix("@@ "), current.as_ref())
            && let Some(range) = hunk_range(hunk)
        {
            found.entry(name.clone()).or_default().push(range);
        }
    }
    found
}

/// The lines each file matching `pathspec` added since the revision `base`, in
/// the work tree at `root`, keyed by its path relative to `root`. A file git
/// does not track yet is added whole, as the range `(1, usize::MAX)`.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
///
/// let added = lgwks_ast::hunks::added_since(Path::new("."), "origin/main", "*.rs")?;
/// for (path, ranges) in &added {
///     println!("{path}: {ranges:?}");
/// }
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// The [`io::Error`] of a `git` that could not start, or one naming the git
/// command and what it printed when it exits unsuccessfully: an unknown `base`
/// or a `root` outside a repository.
pub fn added_since(
    root: &Path,
    base: &str,
    pathspec: &str,
) -> io::Result<BTreeMap<String, Vec<(usize, usize)>>> {
    let diff = git(
        root,
        &[
            "diff",
            "--unified=0",
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            "--relative",
            base,
            "--",
            pathspec,
        ],
    )?;
    let mut found = added_lines(&diff);
    let untracked = git(
        root,
        &["ls-files", "--others", "--exclude-standard", "--", pathspec],
    )?;
    for name in untracked.lines() {
        found.insert(name.to_owned(), vec![(1, usize::MAX)]);
    }
    Ok(found)
}

/// Runs `git` in `root` and returns what it printed, refusing a failed run.
fn git(root: &Path, arguments: &[&str]) -> io::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()?;
    if !output.status.success() {
        tracing::warn!(?arguments, status = %output.status, "git refused the change base");
        return Err(io::Error::other(format!(
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The added range of one `@@ -a,b +c,d @@` hunk header, or `None` for a hunk
/// that only deletes.
fn hunk_range(hunk: &str) -> Option<(usize, usize)> {
    let added = hunk
        .split_whitespace()
        .find_map(|part| part.strip_prefix('+'))?;
    let (start, count): (usize, usize) = match added.split_once(',') {
        Some((start, count)) => (start.parse().ok()?, count.parse().ok()?),
        None => (added.parse().ok()?, 1),
    };
    if count == 0 {
        return None;
    }
    let last = start.checked_add(count)?.checked_sub(1)?;
    Some((start, last))
}

#[cfg(test)]
mod tests {
    use super::{added_lines, hunk_range};

    #[test]
    fn every_added_range_of_a_file_is_read_in_hunk_order() {
        let diff = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -3,0 +4,2 @@ fn f() {}\n+/// Adds one.\n+pub fn bump(x: u8) -> u8 { x + 1 }\n@@ -9 +11 @@\n-old\n+new\n";
        assert_eq!(
            added_lines(diff).get("src/lib.rs"),
            Some(&vec![(4, 5), (11, 11)])
        );
    }

    #[test]
    fn a_header_reads_to_its_added_range() {
        assert_eq!(hunk_range("-4,2 +7,3 @@ heading"), Some((7, 9)));
        assert_eq!(hunk_range("-4 +7 @@"), Some((7, 7)));
        assert_eq!(hunk_range("-4,2 +6,0 @@"), None);
    }

    #[test]
    fn a_deleting_hunk_and_a_deleted_file_add_nothing() {
        let diff = "+++ b/a.rs\n@@ -4,2 +3,0 @@\n+++ /dev/null\n@@ -1,3 +0,0 @@\n";
        let added = added_lines(diff);
        assert!(added.is_empty(), "{added:?}");
    }

    #[test]
    fn a_malformed_header_is_skipped() {
        let diff = "+++ b/a.rs\n@@ -1 +x,2 @@\n@@ -1 +7,99999999999999999999 @@\n@@ -1 +2,3 @@\n";
        assert_eq!(added_lines(diff).get("a.rs"), Some(&vec![(2, 4)]));
    }

    #[test]
    fn a_hunk_before_any_file_is_ignored() {
        assert!(added_lines("@@ -1 +1 @@\n").is_empty());
    }
}
