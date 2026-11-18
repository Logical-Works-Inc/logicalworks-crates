//! `lgwks-deps check` held to its fail-closed verdict at the report layer
//! (`check_report.rs`), through the shipped binary, in both output modes.
//!
//! The fool here points the gate at roots it cannot read: a path that does not
//! exist and a directory with no workspace in it. Neither can reach a verdict, so
//! both modes must refuse with exit 2, and `--json` must say `admitted: false`
//! and carry the error, never the empty refusal list a skipped audit would
//! leave. The repository's own root is the control: it reaches a verdict and
//! passes in both modes, so a gate that refused everything fails here too.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

/// One run of the shipped `check` command: its exit code, its stdout, and its
/// stderr so a failed assertion shows what the binary said.
fn check(root: &Path, json: bool) -> Result<(Option<i32>, String, String), Box<dyn Error>> {
    let binary = std::env::var_os("CARGO_BIN_EXE_lgwks-deps")
        .ok_or("Cargo did not provide the lgwks-deps test binary")?;
    let mut command = Command::new(binary);
    command.arg("check").arg(root);
    if json {
        command.arg("--json");
    }
    let output = command.output()?;
    Ok((
        output.status.code(),
        String::from_utf8(output.stdout)?,
        String::from_utf8(output.stderr)?,
    ))
}

/// Roots no audit can read: a path directly under `/` that only root could
/// create, and the filesystem root, which exists and is readable but holds no
/// workspace and no `Cargo.lock`. (A member's `Cargo.toml` is not one: it
/// resolves to its workspace and passes.)
fn hostile_roots() -> [PathBuf; 2] {
    [
        PathBuf::from("/lgwks-sim-check-report-absent"),
        PathBuf::from("/"),
    ]
}

#[test]
fn sim_check_report_refuses_an_unreadable_root_in_both_modes() -> TestResult {
    for root in hostile_roots() {
        let (human, said, complained) = check(&root, false)?;
        assert_eq!(
            human,
            Some(2),
            "{}: no verdict is a refusal: {said}{complained}",
            root.display()
        );
        let (code, payload, _) = check(&root, true)?;
        assert_eq!(
            code,
            Some(2),
            "{}: --json refuses alike: {payload}",
            root.display()
        );
        assert!(
            payload.contains("\"admitted\": false"),
            "{}: the payload must not read as admitted: {payload}",
            root.display()
        );
        assert!(
            payload.contains("\"error\": \""),
            "{}: the unreached verdict is rendered as an error: {payload}",
            root.display()
        );
    }
    Ok(())
}

#[test]
fn sim_check_report_admits_the_repository_root_in_both_modes() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (human, said, complained) = check(&root, false)?;
    assert_eq!(
        human,
        Some(0),
        "the tree's own registers hold: {said}{complained}"
    );
    assert!(!said.is_empty(), "a pass says what it read");
    let (code, payload, _) = check(&root, true)?;
    assert_eq!(code, Some(0), "--json passes alike: {payload}");
    assert!(
        payload.contains("\"admitted\": true"),
        "a pass reads as admitted: {payload}"
    );
    Ok(())
}
