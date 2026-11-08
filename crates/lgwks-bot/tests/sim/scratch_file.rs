//! A journal path of one test's own, gone when the test is.

use std::path::{Path, PathBuf};

/// A unique path in the temporary directory, removed on drop.
pub struct ScratchFile(PathBuf);

impl ScratchFile {
    /// A fresh path named for `name`, which no other run or test can share.
    pub fn new(name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let unique = lgwks_std::id::Uuid::new_v4()?;
        Ok(Self(
            std::env::temp_dir().join(format!("lgwks-{name}-{unique}")),
        ))
    }

    /// Where the file is, or will be.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchFile {
    fn drop(&mut self) {
        drop(std::fs::remove_file(&self.0));
    }
}
