//! The shipped `check-gate-parity` binary keeps the doc tests off the one
//! critical path, driven on a scratch copy of the workflow tree.
//!
//! Source: #1095. As a step of the evidence job the doc tests ran after the
//! journey, about 200 s, and the run walled at 313 s with every test green
//! (run 45086616792). Under the shared `workspace` cache key another job's
//! `dev` build owned the entry, so every run compiled 249 crates again (run
//! 45056931513). Requirement proved (INV-CI-DOC-PATH-1): the committed tree
//! passes, and each way back onto the path is refused by name.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

/// Every file the binary reads, relative to the repository root.
const READS: [&str; 8] = [
    "scripts/gate-lanes.toml",
    ".github/workflows/ci.yml",
    ".github/workflows/pages.yml",
    ".github/workflows/logical_ci.yml",
    ".github/workflows/consumer-gate.yml",
    ".github/workflows/cache-budget.yml",
    ".github/actions/hosted-rust/action.yml",
    ".cargo/config.toml",
];

/// The workflow the cases change.
const WORKFLOW: &str = ".github/workflows/ci.yml";

/// Copies the tree into `case` with the workflow's first `from` replaced by
/// `to`, runs the binary on it, and returns its exit code and stderr.
fn judge(case: &str, from: &str, to: &str) -> Result<(Option<i32>, String), Box<dyn Error>> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(case);
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    for rel in READS {
        let target = root.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = fs::read_to_string(repo.join(rel))?;
        if rel == WORKFLOW {
            assert!(
                text.contains(from),
                "{case}: the text it changes must exist: {from:?}"
            );
            fs::write(&target, text.replacen(from, to, 1))?;
        } else {
            fs::write(&target, text)?;
        }
    }
    let output = Command::new(env!("CARGO_BIN_EXE_check-gate-parity"))
        .arg(&root)
        .output()?;
    fs::remove_dir_all(&root)?;
    Ok((
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

#[test]
fn the_doc_tests_hold_no_job_but_the_verdict() -> TestResult {
    let (code, stderr) = judge("doc-path-committed", "", "")?;
    assert_eq!(code, Some(0), "the committed tree passes: {stderr}");
    let cases = [
        (
            "doc-path-journey-waits",
            "    needs: workspace_tail\n",
            "    needs: [workspace_tail, doc_tests]\n",
            "debug_e2e waits on doc_tests, which runs the doc tests: only build_verdict may (#1095)",
        ),
        (
            "doc-path-shared-key",
            "cache-key: doc-tests\n",
            "cache-key: workspace\n",
            "doc_tests's cache key workspace is also workspace_all_targets's: the first saver owns the entry (#1095)",
        ),
        (
            "doc-path-no-key",
            "          cache-key: doc-tests\n",
            "",
            "doc_tests restores no dependency build (#1095)",
        ),
        (
            "doc-path-no-step",
            "      - name: Doc tests\n",
            "      - name: Doc test run\n",
            "ci.yml runs no `Doc tests` step",
        ),
    ];
    for (case, from, to, named) in cases {
        let (code, stderr) = judge(case, from, to)?;
        assert_eq!(code, Some(1), "{case}: {stderr}");
        assert!(stderr.contains(named), "{case}: {named}: {stderr}");
    }
    Ok(())
}
