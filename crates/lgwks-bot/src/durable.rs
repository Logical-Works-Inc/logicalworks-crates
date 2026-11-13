//! The one durable whole-document replace, and the directory sync it ends with.
//!
//! A store that persists a whole document at a time writes it beside the real
//! name and renames it over. That is atomic against a process crash on its own,
//! and it is not durable against a power loss: the rename can reach the disk
//! before the bytes do, and the next open then reads an empty or torn file under
//! the real name, which the store refuses whole. So the bytes are synced before
//! the rename, and the directory after it (#910 defect 8).

use std::io::{self, Write as _};
use std::path::Path;

/// Replace `path` with `bytes`, so that a crash or a power loss leaves the old
/// document or the new one, never a prefix and never neither.
///
/// The scratch file is the full name plus [`SCRATCH_SUFFIX`], so `x.json` and
/// `x.db` in one directory never share one, and a document named `x.tmp` is not
/// the scratch of `x`. A failure anywhere before the rename leaves the old
/// document in place, and the error is the device's own.
pub(crate) fn replace_durably(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let scratch = scratch_path(path)?;
    let mut file = std::fs::File::create(&scratch)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&scratch, path)?;
    sync_directory(parent_of(path))
}

/// The name a replace of `path` writes before it renames.
///
/// # Errors
///
/// `InvalidInput` when `path` names no file (`/`, `..`, an empty path).
fn scratch_path(path: &Path) -> io::Result<std::path::PathBuf> {
    let Some(name) = path.file_name() else {
        let refusal = Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a durable replace needs a path that names a file",
        ));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "scratch_path: returning an error to the caller");
        return refusal;
    };
    let mut scratch = name.to_os_string();
    scratch.push(SCRATCH_SUFFIX);
    Ok(path.with_file_name(scratch))
}

/// The suffix a replace's scratch file carries: reserved beside a store, so no
/// document a caller names can be another document's scratch.
const SCRATCH_SUFFIX: &str = ".lgwks-scratch";

/// The directory `path` sits in, `.` for a bare relative name.
fn parent_of(path: &Path) -> &Path {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    }
}

/// Make the names in `dir` durable, where the platform has such a call.
///
/// `#[cfg(unix)]` rather than best effort: elsewhere a directory cannot be
/// opened as a file at all, and a swallowed error would be a bound nothing
/// observes. What the absence costs is power-loss durability only; a name is in
/// the running system's cache across any process death.
#[cfg(unix)]
pub(crate) fn sync_directory(dir: &Path) -> io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

/// [`sync_directory`] where the platform has no directory sync to make.
#[cfg(not(unix))]
pub(crate) fn sync_directory(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod sim_durable;
