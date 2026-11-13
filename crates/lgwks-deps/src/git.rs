//! The one way this crate runs `git`: pointed at one repository, with no stdin
//! and none of the environment that would redirect it to another.
//!
//! `invariants::binding` checks recorded runs against history with it, and
//! `advisories` reads the advisory database's `HEAD` with it.

use std::io;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Environment Git reads that would redirect it away from `root`. A hook runs
/// with some of these set, and a gate that answered about the wrong repository
/// would be worse than one that refused.
const REDIRECTING_ENV: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

/// Runs `git -C root args…` with no stdin, no optional locks and no
/// redirecting environment.
///
/// # Errors
///
/// The [`io::Error`] when `git` cannot be started.
pub(crate) fn git(root: &Path, args: &[&str]) -> io::Result<Output> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .env("GIT_OPTIONAL_LOCKS", "0");
    for name in REDIRECTING_ENV {
        command.env_remove(name);
    }
    command.output()
}
