//! The residue census: what one test leaves behind when it exits (ADR 0078,
//! INV-CI-RESIDUE-1, #1128 G2).
//!
//! The wrapper gives each test a `TMPDIR` of its own, made fresh under the one
//! it would have used. When the test exits, an entry still in that directory
//! is a leak: the test, or the code it drove, made it and nothing removed it.
//! The census then removes what it found, so the wrapper itself leaves nothing.
//! Processes a test orphans are not judged here: a subreaper that does not
//! reap while the test runs breaks init's semantics for it (#1128 G2b).
//!
//! It judges the outcome, not a proxy: a test passes it only by leaving
//! nothing, and no count of tests, lines or annotations moves it.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The prefix of one test's own temporary directory.
const PREFIX: &str = "lwc-test-";

/// Leftover entries a verdict names; the rest are counted.
const NAMED: usize = 8;

/// Entries one census reads before it stops counting and says so.
const COUNT_MAX: usize = 100_000;

/// One test's own temporary directory, judged and removed by [`Census::close`].
#[derive(Debug)]
pub(super) struct Census {
    /// The directory the test sees as `TMPDIR`.
    dir: PathBuf,
}

/// What one test left behind. Empty is clean.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Residue {
    /// The first [`NAMED`] leftover paths under the test's directory, sorted.
    named: BTreeSet<PathBuf>,
    /// Every leftover entry counted, at most [`COUNT_MAX`].
    total: usize,
    /// Whether the count stopped at [`COUNT_MAX`].
    capped: bool,
    /// Census steps the host refused. A refused step is not clean: the census
    /// fails closed.
    refusals: Vec<String>,
}

impl Census {
    /// A fresh directory for one test under `base`, named by 64 random bits.
    ///
    /// # Errors
    ///
    /// The draw's or the `create_dir` call's error: a test never runs without
    /// the census it was promised.
    pub(super) fn open(base: &Path) -> io::Result<Self> {
        let unique = lgwks_std::random::bytes::<8>().map_err(io::Error::other)?;
        let dir = base.join(format!("{PREFIX}{:016x}", u64::from_be_bytes(unique)));
        if let Err(error) = fs::create_dir(&dir) {
            lgwks_std::trace::debug!(%error, dir = %dir.display(), "census: the test's directory could not be made");
            return Err(error);
        }
        Ok(Self { dir })
    }

    /// The directory the test runs with.
    pub(super) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Judge the directory after the test exited into `residue`, then remove it.
    pub(super) fn close(self, residue: &mut Residue) {
        count(&self.dir, residue);
        if let Err(error) = fs::remove_dir_all(&self.dir) {
            residue.refuse(format!(
                "{} could not be removed: {error}",
                self.dir.display()
            ));
        }
    }
}

/// Count the entries under `dir` into `residue`, naming the first [`NAMED`]
/// relative paths in sorted order. Bounded by [`COUNT_MAX`] entries.
fn count(dir: &Path, residue: &mut Residue) {
    count_within(dir, residue, COUNT_MAX);
}

/// The entries of one directory, or the first error reading them.
fn entries_of(dir: &Path) -> io::Result<Vec<fs::DirEntry>> {
    fs::read_dir(dir)?.collect()
}

/// [`count`] bounded by `max` entries; past it the count is `capped`. A
/// directory that cannot be read is a refusal, and the walk goes on to the
/// rest, so every readable entry is still counted.
fn count_within(dir: &Path, residue: &mut Residue, max: usize) {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let entries = match entries_of(&next) {
            Ok(entries) => entries,
            Err(error) => {
                residue.refuse(format!("{} could not be read: {error}", next.display()));
                continue;
            }
        };
        for entry in entries {
            if residue.total >= max {
                residue.capped = true;
                return;
            }
            residue.total = residue.total.saturating_add(1);
            let path = entry.path();
            if let Ok(relative) = path.strip_prefix(dir) {
                residue.named.insert(relative.to_path_buf());
                if residue.named.len() > NAMED {
                    residue.named.pop_last();
                }
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(path),
                Ok(_) => {}
                Err(error) => {
                    residue.refuse(format!("{} has no readable type: {error}", path.display()))
                }
            }
        }
    }
}

impl Residue {
    /// Whether the test left nothing and every census step ran.
    pub(super) fn is_clean(&self) -> bool {
        self.total == 0 && self.refusals.is_empty()
    }

    /// Record a census step the host refused.
    pub(super) fn refuse(&mut self, why: String) {
        lgwks_std::trace::debug!(
            why,
            "census: a step was refused, so the test is not judged clean"
        );
        self.refusals.push(why);
    }
}

impl fmt::Display for Residue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.total > 0 {
            let names: Vec<String> = self
                .named
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            let more = if self.capped { "+" } else { "" };
            writeln!(
                formatter,
                "residue: the test left {}{more} entries in its TMPDIR: {} (ADR 0078)",
                self.total,
                names.join(", ")
            )?;
        }
        for refusal in &self.refusals {
            writeln!(formatter, "residue: the census could not judge: {refusal}")?;
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
#[path = "sim_residue.rs"]
mod sim_residue;
