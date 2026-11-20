//! A journal path of one test's own, gone when the test is.

use std::path::{Path, PathBuf};

/// A unique path in the temporary directory, removed on drop.
pub(crate) struct ScratchFile(PathBuf);

impl ScratchFile {
    /// A fresh path named for `name`, which no other run or test can share.
    pub(crate) fn new(name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let unique = lgwks_std::id::Uuid::new_v4()?;
        Ok(Self(
            std::env::temp_dir().join(format!("lgwks-{name}-{unique}")),
        ))
    }

    /// Where the file is, or will be.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchFile {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.0) {
            Ok(()) => {}
            // A test that never wrote the file leaves nothing to remove.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => lgwks_std::trace::debug!(
                error = %error,
                path = %self.0.display(),
                "scratch file: not removed, so the temporary directory keeps it"
            ),
        }
    }
}
