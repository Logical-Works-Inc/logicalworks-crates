//! The shipped `check-doc-citations` binary, driven on a scratch tree.
//!
//! Source: run 45089113500 (PR #1125). ADR 0075 cited `AGENTS.md:14`, nobody
//! ran `--update`, and the `doc-citations` lane failed with "1 citation(s) are
//! not in the lock". Requirement proved (INV-DOC-2): a citation a page adds is
//! refused by name until `--update` pins it, and then the check passes.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

type TestResult = Result<(), Box<dyn Error>>;

/// Runs the binary on `root`, with `--update` when `update` is set.
fn check(root: &Path, update: bool) -> Result<Output, Box<dyn Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_check-doc-citations"));
    command.arg(root);
    if update {
        command.arg("--update");
    }
    Ok(command.output()?)
}

#[test]
fn a_new_citation_fails_by_name_until_pinned() -> TestResult {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("a-new-citation-fails-by-name");
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(root.join("docs/adr"))?;
    fs::create_dir_all(root.join("scripts"))?;
    fs::create_dir_all(root.join("crates/lgwks-std/src"))?;
    // The std inventory compares these two; empty, they agree on nothing.
    fs::write(root.join("crates/lgwks-std/src/lib.rs"), "")?;
    fs::write(root.join("docs/std-ast-deps-closure-matrix-std.md"), "")?;
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"subject\"\n")?;
    fs::write(root.join("docs/census.md"), "The name is `Cargo.toml:1`.\n")?;
    let pinned = check(&root, true)?;
    fs::write(
        root.join("docs/adr/0001-a-record.md"),
        "The finding was `Cargo.toml:2`.\n",
    )?;
    let unpinned = check(&root, false)?;
    let updated = check(&root, true)?;
    let verified = check(&root, false)?;
    fs::remove_dir_all(&root)?;
    let stderr = String::from_utf8_lossy(&unpinned.stderr);
    assert!(pinned.status.success(), "the first pin is written");
    assert_eq!(unpinned.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("1 citation(s) are not in the lock"),
        "{stderr}"
    );
    assert!(
        stderr.contains("Cargo.toml:2"),
        "the citation is named: {stderr}"
    );
    assert!(updated.status.success(), "--update pins it");
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    Ok(())
}
