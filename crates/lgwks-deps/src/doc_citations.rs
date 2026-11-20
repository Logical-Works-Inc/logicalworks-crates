//! Checks that every `path:NNN` citation in the documentation still resolves.
//!
//! The guides cite the source file and line a claim came from, e.g.
//! `crates/lgwks-bot/src/ecs.rs:1496`. Those citations are maintained by hand
//! and every merge moves the lines under them, so they rot silently.
//!
//! The structural layer needs no state: the cited file exists, the cited line
//! exists and is not blank, and the cited line is not one that cannot support
//! any claim (a bare closing delimiter, an empty `///`, `//!` or `//`).
//!
//! The lock is the layer that needs state. `scripts/doc-citations.lock` records
//! the text of every line the documentation cites, and this fails when any of
//! them differs: when a cited file grows, the numbers below the insertion point
//! each land on some other plausible line, so the citation resolves while
//! pointing at unrelated code. `--update` writes the lock, and the diff is then
//! the review.
//!
//! Usage: `check-doc-citations [--update] [--limit N] [ROOT]`. Exits non-zero,
//! listing every failure, when any citation fails.
//!
//! The one implementation of this check in the estate (#695): this file is the
//! rule, the `check-doc-citations` binary reads its arguments with [`options`]
//! and runs [`execute`], and `logical_ci`'s `docs-refs` gate calls [`execute`]
//! as a library, so a changed document's dangling citations are NO-GO under
//! one rule with one owner.
//!
//! [`options`]: crate::doc_citations::options
//! [`execute`]: crate::doc_citations::execute

mod lock;
mod scan;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use lock::{Drift, clip, drift, read_lock, spell, text_of, write_lock};
use scan::{collect, inventory_failures};

/// Where the reviewed text of every cited line is recorded.
const LOCK_NAME: &str = "scripts/doc-citations.lock";

/// How deep a documentation walk may go.
const MAX_DEPTH: usize = 32;

/// Why the check could not run.
#[doc(hidden)]
#[derive(Debug)]
#[non_exhaustive]
pub enum CiteError {
    /// A file or directory could not be read or written.
    Io {
        /// What was being touched.
        path: PathBuf,
        /// What the operating system said.
        cause: io::Error,
    },
    /// The verdict could not be written to the caller's handle.
    Report {
        /// What the handle said.
        cause: io::Error,
    },
    /// A lock row's key is not `<path>:<line>`.
    BadLockKey {
        /// The key as written.
        key: String,
    },
    /// A walk went deeper than [`MAX_DEPTH`].
    TooDeep {
        /// Where the walk stopped.
        path: PathBuf,
    },
}

impl fmt::Display for CiteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Io {
                ref path,
                ref cause,
            } => write!(formatter, "{}: {cause}", path.display()),
            Self::Report { ref cause } => {
                write!(formatter, "the verdict could not be written: {cause}")
            }
            Self::BadLockKey { ref key } => {
                write!(
                    formatter,
                    "{LOCK_NAME}: bad key {key:?}, expected <path>:<line>"
                )
            }
            Self::TooDeep { ref path } => {
                write!(
                    formatter,
                    "{} is nested past {MAX_DEPTH} levels",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for CiteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Io { ref cause, .. } | Self::Report { ref cause } => Some(cause),
            Self::BadLockKey { .. } | Self::TooDeep { .. } => None,
        }
    }
}

/// A write to the caller's output handle failed: every `writeln!` in this
/// module targets one, so the failure is the verdict's, not a file's.
impl From<io::Error> for CiteError {
    fn from(cause: io::Error) -> Self {
        Self::Report { cause }
    }
}

/// Logs `error` at the site that produced it, then hands it back as the `Err`.
fn fail<T>(error: CiteError, site: &'static str) -> Result<T, CiteError> {
    lgwks_std::trace::debug!(error = ?error, site, "returning an error to the caller");
    Err(error)
}

/// A cited line: its file and its one-based number.
type Key = (String, u64);

/// What a walk of the documentation found.
struct Collected {
    /// The cited line's text by key.
    entries: BTreeMap<Key, String>,
    /// The pages citing each key.
    sites: BTreeMap<Key, Vec<String>>,
    /// Pages whose citations are not checked.
    skipped: Vec<String>,
    /// Structural failures.
    failures: Vec<String>,
}

/// What the command line asked for.
#[derive(Debug)]
#[doc(hidden)]
#[non_exhaustive]
pub struct Options {
    /// The repository root.
    #[doc(hidden)]
    pub root: PathBuf,
    /// Whether to rewrite the lock.
    #[doc(hidden)]
    pub update: bool,
    /// How many failures to print.
    #[doc(hidden)]
    pub limit: usize,
}

#[doc(hidden)]
impl Options {
    /// What a caller asked for: the root to read, whether to rewrite the
    /// lock, and how many failures to print.
    #[must_use]
    pub fn new(root: PathBuf, update: bool, limit: usize) -> Self {
        Self {
            root,
            update,
            limit,
        }
    }
}

/// Reads a file whole, naming it when it cannot be read.
fn read(path: &Path) -> Result<String, CiteError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(cause) => fail(
            CiteError::Io {
                path: path.to_path_buf(),
                cause,
            },
            "read",
        ),
    }
}

/// The summary suffix naming pages whose citations are not checked, if any.
fn skipped_note(collected: &Collected) -> String {
    if collected.skipped.is_empty() {
        String::new()
    } else {
        format!("; not checked: {}", collected.skipped.join(", "))
    }
}

/// The pages citing `key`, sorted and joined.
fn cited_by(collected: &Collected, key: &Key) -> String {
    let pages: BTreeSet<&String> = match collected.sites.get(key) {
        Some(pages) => pages.iter().collect(),
        None => BTreeSet::new(),
    };
    pages
        .into_iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Prints structural failures and the reason the lock was left alone.
fn report_failures(options: &Options, failures: &[String], err: &mut impl Write) -> io::Result<()> {
    writeln!(err, "{} unresolvable citation(s):", failures.len())?;
    for failure in failures.iter().take(options.limit) {
        writeln!(err, "  {failure}")?;
    }
    if failures.len() > options.limit {
        writeln!(
            err,
            "  … and {} more",
            failures.len().saturating_sub(options.limit)
        )?;
    }
    writeln!(
        err,
        "\nThe lock was not touched: a citation that does not resolve is fixed by re-anchoring \
         it, not by recording where it landed."
    )
}

/// Rewrites the lock and says what moved.
fn update_lock(
    options: &Options,
    collected: &Collected,
    locked: &BTreeMap<Key, String>,
    found: &Drift,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<u8, CiteError> {
    let lock_path = options.root.join(LOCK_NAME);
    if !found.moved.is_empty() {
        writeln!(err, "{} cited line(s) changed text:", found.moved.len())?;
        for key in found.moved.iter().take(options.limit) {
            writeln!(
                err,
                "  {}  (cited by {})",
                spell(key),
                cited_by(collected, key)
            )?;
            writeln!(err, "    was: {}", clip(&text_of(locked, key), 100))?;
            writeln!(
                err,
                "    now: {}",
                clip(&text_of(&collected.entries, key), 100)
            )?;
        }
    }
    if !found.stale.is_empty() {
        writeln!(err, "{} entry(ies) are no longer cited:", found.stale.len())?;
        for key in found.stale.iter().take(options.limit) {
            writeln!(err, "  {}", spell(key))?;
        }
    }
    write_lock(&lock_path, &collected.entries)?;
    writeln!(
        out,
        "lock rewritten: {} citation(s) pinned, {} new, {} re-anchored, {} dropped{}",
        collected.entries.len(),
        found.unpinned.len(),
        found.moved.len(),
        found.stale.len(),
        skipped_note(collected)
    )?;
    Ok(if read_lock(&lock_path)? == collected.entries {
        0
    } else {
        1
    })
}

/// Prints how the citations drifted from the lock.
fn report_drift(
    options: &Options,
    collected: &Collected,
    locked: &BTreeMap<Key, String>,
    found: &Drift,
    err: &mut impl Write,
) -> io::Result<()> {
    if !found.unpinned.is_empty() {
        writeln!(
            err,
            "{} citation(s) are not in the lock. Add them with `--update` once the line each one \
             names is confirmed to support the sentence citing it:",
            found.unpinned.len()
        )?;
        for key in found.unpinned.iter().take(options.limit) {
            writeln!(
                err,
                "  {}  {}",
                spell(key),
                clip(&text_of(&collected.entries, key), 80)
            )?;
        }
    }
    if !found.moved.is_empty() {
        writeln!(
            err,
            "{} cited line(s) no longer hold the text they were pinned to. The number resolves, \
             which is exactly why this is the check that catches a citation that moved under a \
             grown file:",
            found.moved.len()
        )?;
        for key in found.moved.iter().take(options.limit) {
            writeln!(
                err,
                "  {}  (cited by {})",
                spell(key),
                cited_by(collected, key)
            )?;
            writeln!(err, "    pinned: {}", clip(&text_of(locked, key), 100))?;
            writeln!(
                err,
                "    found:  {}",
                clip(&text_of(&collected.entries, key), 100)
            )?;
        }
    }
    if !found.stale.is_empty() {
        writeln!(
            err,
            "{} lock entry(ies) are cited by nothing. A citation was removed or renumbered \
             without the lock being regenerated:",
            found.stale.len()
        )?;
        for key in found.stale.iter().take(options.limit) {
            writeln!(err, "  {}  {}", spell(key), clip(&text_of(locked, key), 80))?;
        }
    }
    writeln!(
        err,
        "\nRe-anchor by content: find where the cited text lives now and point the citation at \
         it, rather than shifting the number by the line delta. Then run `--update`."
    )
}

/// Runs the check, writing its verdict to the two handles.
#[doc(hidden)]
pub fn execute(
    options: &Options,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<u8, CiteError> {
    let mut collected = collect(&options.root)?;
    collected
        .failures
        .extend(inventory_failures(&options.root)?);
    if !collected.failures.is_empty() {
        report_failures(options, &collected.failures, err)?;
        return Ok(1);
    }
    let locked = read_lock(&options.root.join(LOCK_NAME))?;
    let found = drift(&collected.entries, &locked);
    if options.update {
        return update_lock(options, &collected, &locked, &found, out, err);
    }
    if locked.is_empty() {
        writeln!(
            err,
            "no lock at {LOCK_NAME}: every citation is unpinned. Generate it with \
             `check-doc-citations --update`, and review the result before committing it."
        )?;
        return Ok(1);
    }
    if !(found.unpinned.is_empty() && found.moved.is_empty() && found.stale.is_empty()) {
        report_drift(options, &collected, &locked, &found, err)?;
        return Ok(1);
    }
    let pages: BTreeSet<&String> = collected.sites.values().flatten().collect();
    writeln!(
        out,
        "doc citations resolve: {} line(s) pinned across {} page(s), all unchanged{}",
        collected.entries.len(),
        pages.len(),
        skipped_note(&collected)
    )?;
    Ok(0)
}

/// Reads the command line: the binary's arguments as an [`Options`].
///
/// Kept beside [`execute`] so the rule and its invocation stay one
/// implementation: the binary calls this, then [`execute`], and a library
/// caller builds [`Options`] directly.
#[doc(hidden)]
#[must_use]
pub fn options() -> Options {
    let mut options = Options {
        root: PathBuf::from("."),
        update: false,
        limit: 40,
    };
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--update" {
            options.update = true;
        } else if argument == "--limit" {
            let Some(value) = arguments.next() else {
                continue;
            };
            let Some(text) = value.to_str() else {
                continue;
            };
            if let Ok(parsed) = text.parse::<usize>() {
                options.limit = parsed;
            }
        } else {
            options.root = PathBuf::from(argument);
        }
    }
    options
}

#[cfg(test)]
mod tests {
    use super::{Options, execute};
    use std::error::Error;
    use std::path::PathBuf;

    type TestResult = Result<(), Box<dyn Error>>;

    /// The committed lock verifies against the committed documentation: the
    /// citations this binary finds are exactly the ones the Python script pinned.
    #[test]
    fn the_committed_lock_verifies() -> TestResult {
        let options = Options {
            root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
            update: false,
            limit: 40,
        };
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = execute(&options, &mut out, &mut err)?;
        assert_eq!(
            code,
            0,
            "the lock must verify: {}",
            String::from_utf8_lossy(&err)
        );
        Ok(())
    }

    /// A scratch repository the checker accepts: its own temp tree, an empty
    /// std inventory on both sides, and a `Cargo.toml` to cite.
    fn scratch_root() -> Result<PathBuf, Box<dyn Error>> {
        // A process id is reused by the OS; the estate's entropy names the tree.
        let nonce = lgwks_std::hex::encode(lgwks_std::random::bytes::<8>()?);
        let root = std::env::temp_dir().join(format!("lgwks-doc-cite-{nonce}"));
        std::fs::create_dir_all(root.join("docs/adr"))?;
        std::fs::create_dir_all(root.join("scripts"))?;
        std::fs::create_dir_all(root.join("crates/lgwks-std/src"))?;
        // The std inventory compares these two; empty, they agree on nothing.
        std::fs::write(root.join("crates/lgwks-std/src/lib.rs"), "")?;
        std::fs::write(root.join("docs/std-ast-deps-closure-matrix-std.md"), "")?;
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"subject\"\n")?;
        Ok(root)
    }

    /// Runs the checker on `root`, returning its code and both handles.
    fn run_on(
        root: &std::path::Path,
        update: bool,
    ) -> Result<(u8, String, String), Box<dyn Error>> {
        let options = Options {
            root: root.to_path_buf(),
            update,
            limit: 40,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = execute(&options, &mut out, &mut err)?;
        Ok((
            code,
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
        ))
    }

    /// Run 43622999959 (PR #1046): the budget lines grew `app.rs` and moved
    /// `auto_workers` from line 430 to 477 under the census's citation. The
    /// number still resolves, so only the pinned text catches it: the check
    /// names the page and both texts, then a re-anchor and `--update` verify.
    #[test]
    fn a_citation_moved_under_a_grown_file_fails_until_re_anchored() -> TestResult {
        let body = "fn auto_workers() -> usize {\n    1\n}\n";
        for grown in [1_usize, 3, 47] {
            let root = scratch_root()?;
            std::fs::create_dir_all(root.join("crates/demo/src"))?;
            let source = root.join("crates/demo/src/lib.rs");
            let page = root.join("docs/census.md");
            std::fs::write(&source, format!("//! demo\n{body}"))?;
            std::fs::write(&page, "One copy is `crates/demo/src/lib.rs:2`.\n")?;
            let pinned = run_on(&root, true)?;
            std::fs::write(
                &source,
                format!("//! demo\n{}{body}", "fn budget() {}\n".repeat(grown)),
            )?;
            let moved = run_on(&root, false)?;
            let line = 2 + grown;
            std::fs::write(
                &page,
                format!("One copy is `crates/demo/src/lib.rs:{line}`.\n"),
            )?;
            let anchored = run_on(&root, true)?;
            let verified = run_on(&root, false)?;
            std::fs::remove_dir_all(&root)?;
            assert_eq!(pinned.0, 0, "the first pin is written: {}", pinned.2);
            assert_eq!(
                moved.0, 1,
                "a grown file fails the check by {grown}: {}",
                moved.2
            );
            assert!(
                moved
                    .2
                    .contains("1 cited line(s) no longer hold the text they were pinned to"),
                "the moved pin is counted: {}",
                moved.2
            );
            assert!(
                moved
                    .2
                    .contains("crates/demo/src/lib.rs:2  (cited by docs/census.md)"),
                "the refusal names the line and the page: {}",
                moved.2
            );
            assert!(
                moved.2.contains("pinned: fn auto_workers() -> usize {")
                    && moved.2.contains("found:  fn budget() {}"),
                "both texts are printed: {}",
                moved.2
            );
            assert_eq!(anchored.0, 0, "the re-anchor is recorded: {}", anchored.2);
            assert!(
                anchored.2.contains("1 entry(ies) are no longer cited:"),
                "the old pin is dropped: {}",
                anchored.2
            );
            assert_eq!(
                verified.0, 0,
                "the re-anchored lock verifies: {}",
                verified.2
            );
            assert!(
                verified.1.contains("doc citations resolve:"),
                "the check says so: {}",
                verified.1
            );
        }
        Ok(())
    }

    /// Run 43715176360 (PR #1057): one import line moved `block_on` onto the
    /// census's pin at `task/admit.rs:42`. `--update` with no doc edit takes
    /// the move, so it must print the line, the page and both texts: that
    /// printout is the review AGENTS.md promises, and without it a pin slides
    /// onto a line its claim never meant.
    #[test]
    fn an_update_over_a_moved_pin_prints_both_texts_for_review() -> TestResult {
        let root = scratch_root()?;
        std::fs::create_dir_all(root.join("crates/demo/src"))?;
        let source = root.join("crates/demo/src/lib.rs");
        std::fs::write(&source, "use a;\npub fn block_on()\nwhere\n")?;
        std::fs::write(
            root.join("docs/census.md"),
            "The block is `crates/demo/src/lib.rs:3`.\n",
        )?;
        let pinned = run_on(&root, true)?;
        std::fs::write(&source, "use a;\nuse b;\npub fn block_on()\nwhere\n")?;
        let updated = run_on(&root, true)?;
        let verified = run_on(&root, false)?;
        std::fs::remove_dir_all(&root)?;
        assert_eq!(pinned.0, 0, "the first pin is written: {}", pinned.2);
        assert_eq!(updated.0, 0, "an update takes the move: {}", updated.2);
        assert!(
            updated.2.contains("1 cited line(s) changed text:")
                && updated
                    .2
                    .contains("crates/demo/src/lib.rs:3  (cited by docs/census.md)"),
            "the update names the line and the page: {}",
            updated.2
        );
        assert!(
            updated.2.contains("was: where") && updated.2.contains("now: pub fn block_on()"),
            "the update prints both texts for review: {}",
            updated.2
        );
        assert!(
            updated.1.contains("1 re-anchored"),
            "the summary counts the move: {}",
            updated.1
        );
        assert_eq!(verified.0, 0, "the updated lock verifies: {}", verified.2);
        Ok(())
    }

    /// Run 43191943628: ADR 0030 cited `src/property.rs:152`, a file only
    /// logical_ci has. A citation into another repository resolves nowhere
    /// here, so it is refused by name and the lock is left alone, while a
    /// citation of this repository's own file beside it still resolves.
    #[test]
    fn a_citation_of_a_file_only_another_repository_has_is_refused() -> TestResult {
        let root = scratch_root()?;
        std::fs::write(
            root.join("docs/adr/0001-foreign.md"),
            "The name is `Cargo.toml:2`; the receipt reader is `src/property.rs:152`.\n",
        )?;
        let (code, _, printed) = run_on(&root, true)?;
        let lock_written = root.join(super::LOCK_NAME).exists();
        std::fs::remove_dir_all(&root)?;
        assert_eq!(code, 1, "a foreign citation fails the check: {printed}");
        assert!(
            printed.contains("1 unresolvable citation(s):"),
            "only the foreign citation fails, the local one resolves: {printed}"
        );
        assert!(
            printed.contains("docs/adr/0001-foreign.md: src/property.rs:152 — no such file"),
            "the refusal names the page and the citation: {printed}"
        );
        assert!(
            !lock_written,
            "--update never pins a citation that does not resolve"
        );
        Ok(())
    }
}
