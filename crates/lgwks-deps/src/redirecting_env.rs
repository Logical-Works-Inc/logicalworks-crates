//! The variables that send `git` to another repository than the one it names.
//!
//! A hook runs with some of these set (githooks(5)), absolute in a worktree.
//! `git` removes them from each run, the `churn-slot` wrapper from each test
//! it starts, and the test fixtures from their own runs, so no fixture's
//! `git -C <dir>` writes the repository a hook runs in. One file, loaded by
//! path in each, so the three lists cannot drift.

/// Environment Git reads that binds it to one repository, as `git rev-parse
/// --local-env-vars` prints it (git 2.54). githooks(5) has a hook unset these
/// before it runs git in another repository; a hook run under `git -c` also
/// exports `GIT_CONFIG_PARAMETERS`, so the list is git's own, not only paths.
pub(crate) const REDIRECTING_ENV: [&str; 15] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];
