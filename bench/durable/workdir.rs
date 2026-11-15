//! The directory a run works in: made fresh under a parent that must already
//! exist, and removed when the run ends.
//!
//! The parent is the one place the user's `--work-dir` reaches. It is never
//! created, so a typo names a missing directory and is refused instead of
//! growing a tree where nobody looked.

use std::io;
use std::path::{Path, PathBuf};

use crate::error::BenchError;

/// How many names one process tries under one parent before it gives up.
const NAME_ATTEMPTS: u32 = 16;

/// A directory this run works in.
#[derive(Debug)]
pub(crate) struct Workdir {
    /// Where it is.
    path: PathBuf,
    /// Whether this value made it, and so removes it when dropped.
    owned: bool,
}

impl Workdir {
    /// A fresh directory under `parent`, or under the OS temp directory when the
    /// user named none.
    ///
    /// # Errors
    ///
    /// [`BenchError::UnreadablePath`] when `parent` does not exist, is not a
    /// directory, or cannot be written to.
    pub(crate) fn create(parent: Option<&Path>) -> Result<Self, BenchError> {
        let base = match parent {
            Some(given) => given.to_path_buf(),
            None => std::env::temp_dir(),
        };
        let unreadable = |source| BenchError::UnreadablePath {
            path: base.clone(),
            source,
        };
        let meta = std::fs::metadata(&base).map_err(unreadable)?;
        if !meta.is_dir() {
            lgwks_std::trace::debug!(path = ?base, "the work directory's parent is not a directory");
            return Err(unreadable(io::Error::new(
                io::ErrorKind::InvalidInput,
                "it is not a directory",
            )));
        }
        let pid = std::process::id();
        for attempt in 0..NAME_ATTEMPTS {
            let path = base.join(format!("lgwks-bench-durable-{pid}-{attempt}"));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path, owned: true }),
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    lgwks_std::trace::debug!(%source, path = ?path, "a work directory could not be made");
                    return Err(unreadable(source));
                }
            }
        }
        Err(unreadable(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "every name this process would use is taken",
        )))
    }

    /// A directory another process made for this one, used as is and left alone.
    ///
    /// # Errors
    ///
    /// [`BenchError::UnreadablePath`] when it is not an existing directory.
    pub(crate) fn adopt(path: &Path) -> Result<Self, BenchError> {
        let meta = std::fs::metadata(path).map_err(|source| BenchError::UnreadablePath {
            path: path.to_path_buf(),
            source,
        })?;
        if !meta.is_dir() {
            lgwks_std::trace::debug!(path = ?path, "the adopted work directory is not a directory");
            return Err(BenchError::UnreadablePath {
                path: path.to_path_buf(),
                source: io::Error::new(io::ErrorKind::InvalidInput, "it is not a directory"),
            });
        }
        Ok(Self {
            path: path.to_path_buf(),
            owned: false,
        })
    }

    /// Where the directory is.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Workdir {
    /// Remove what this run made. A failure cannot be returned from a drop, so
    /// it is traced: the directory is litter under the temp root, not a wrong
    /// answer.
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            lgwks_std::trace::warn!(%error, "the work directory was not removed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test's result.
    type TestResult = Result<(), String>;

    /// A parent that is not there is refused and named, and nothing is made.
    #[test]
    fn a_missing_parent_is_unreadable_not_created() -> TestResult {
        let made = Workdir::create(None).map_err(|error| error.line())?;
        let absent = made.path().join("absent");
        let refused = Workdir::create(Some(&absent));
        assert!(
            matches!(refused, Err(BenchError::UnreadablePath { .. })),
            "{refused:?}"
        );
        assert!(!absent.exists(), "the refused parent was created");
        Ok(())
    }

    /// A parent that is a file is refused.
    #[test]
    fn a_file_as_parent_is_unreadable() -> TestResult {
        let made = Workdir::create(None).map_err(|error| error.line())?;
        let file = made.path().join("plain");
        std::fs::write(&file, b"x").map_err(|error| error.to_string())?;
        let refused = Workdir::create(Some(&file));
        assert!(matches!(refused, Err(BenchError::UnreadablePath { .. })));
        Ok(())
    }

    /// The double-clicker: two directories in one process under one parent get
    /// two names, and each is removed with its own value.
    #[test]
    fn sim_two_work_dirs_in_one_parent_are_distinct_and_removed_on_drop() -> TestResult {
        let outer = Workdir::create(None).map_err(|error| error.line())?;
        let first = Workdir::create(Some(outer.path())).map_err(|error| error.line())?;
        let second = Workdir::create(Some(outer.path())).map_err(|error| error.line())?;
        assert_ne!(first.path(), second.path());
        let (kept, gone) = (second.path().to_path_buf(), first.path().to_path_buf());
        drop(first);
        assert!(!gone.exists(), "a dropped directory is removed");
        assert!(kept.exists(), "the other directory is not touched");
        drop(second);
        assert!(!kept.exists());
        Ok(())
    }

    /// An adopted directory outlives its value: the parent owns it.
    #[test]
    fn an_adopted_directory_is_not_removed() -> TestResult {
        let outer = Workdir::create(None).map_err(|error| error.line())?;
        let adopted = Workdir::adopt(outer.path()).map_err(|error| error.line())?;
        drop(adopted);
        assert!(outer.path().exists());
        let refused = Workdir::adopt(&outer.path().join("absent"));
        assert!(matches!(refused, Err(BenchError::UnreadablePath { .. })));
        Ok(())
    }
}
