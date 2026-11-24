//! The variables that send `git` to another repository than the one it names.
//!
//! A hook runs with some of these set (githooks(5)), absolute in a worktree.
//! `git` removes them from each run, the `churn-slot` wrapper from each test
//! it starts, and the test fixtures from their own runs, so no fixture's
//! `git -C <dir>` writes the repository a hook runs in. One file, loaded by
//! path in each, so the three lists cannot drift.

/// Environment Git reads that would redirect it away from the repository it
/// is pointed at.
pub(crate) const REDIRECTING_ENV: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];
