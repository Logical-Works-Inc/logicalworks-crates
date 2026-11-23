//! A journal path of one test's own, gone when the test is.
//!
//! The path sits alone in a directory of its own, and the drop removes that
//! directory. A journal writes siblings beside its path (`-store`, `.cont`,
//! successors), and removing only the path left them in the temp root: the
//! residue census (ADR 0078) named the tests that did.

use std::path::{Path, PathBuf};

/// A unique path in a fresh temporary directory, removed with it on drop.
pub(crate) struct ScratchFile {
    /// The directory only this path lives in.
    dir: PathBuf,
    /// The path a test writes.
    file: PathBuf,
}

impl ScratchFile {
    /// A fresh path named for `name`, which no other run or test can share.
    pub(crate) fn new(name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let unique = lgwks_std::id::Uuid::new_v4()?;
        let dir = std::env::temp_dir().join(format!("lgwks-{name}-{unique}"));
        std::fs::create_dir(&dir)?;
        let file = dir.join(name);
        Ok(Self { dir, file })
    }

    /// Where the file is, or will be.
    pub(crate) fn path(&self) -> &Path {
        &self.file
    }
}

impl Drop for ScratchFile {
    /// Remove the directory, and every sibling the journal wrote in it.
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => lgwks_std::trace::debug!(
                error = %error,
                path = %self.dir.display(),
                "scratch file: not removed, so the temporary directory keeps it"
            ),
        }
    }
}
