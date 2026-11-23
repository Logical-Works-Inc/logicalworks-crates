//! Holds a page for people to one statement per sentence, with nothing sold.
//!
//! The Director's order of 2026-11-22 ("EVERY BYTE OF DOCUMENTATION IS FRONTIER AND
//! UPDATED AND TOKEN EFFICIENT") ended the sales voice: the README sold "Why
//! Developers & AI Choose" beside a table that said it once. ASD-STE100 keeps one
//! statement to a sentence, so the shipped `check-docs` refuses an em dash and a
//! sales phrase in prose, while the same text as inline code is a literal.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

/// One page with an em dash on line 3, a sales phrase on line 5, the same words
/// as inline code on line 7 and inside a fence on line 10: the binary names
/// lines 3 and 5, never 7 or 10.
#[test]
fn a_page_for_people_sells_nothing() -> TestResult {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("a-page-for-people-sells-nothing");
    fs::create_dir_all(root.join("docs/adr"))?;
    fs::create_dir_all(root.join("docs/prd"))?;
    fs::write(
        root.join("docs/adr/0001-a-record.md"),
        "# ADR 0001: A record\n",
    )?;
    let page = "# T\n\nOne thing \u{2014} then another.\n\nWhy Developers & AI Choose it.\n\nThe `seamless \u{2014}` literal.\n\n```text\nseamless \u{2014} best-in-class\n```\n";
    fs::write(root.join("README.md"), page)?;
    let run = Command::new(env!("CARGO_BIN_EXE_check-docs"))
        .arg(&root)
        .output()?;
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let on = |line: usize| {
        let prefix = format!("README.md:{line}:");
        printed
            .lines()
            .filter(|finding| finding.trim_start().starts_with(&prefix))
            .count()
    };
    assert_eq!((on(3), on(5), on(7), on(10)), (1, 1, 0, 0), "{printed}");
    Ok(())
}

/// The Director's order of 2026-11-22 ("THERE SHOULD BE 0 LINT FINDINGS",
/// "ALWAYS"): the shipped `check-docs` audits every page of this repository,
/// records included, `AGENTS.md` too, and exits 0 with nothing on stderr. A
/// crash or a single finding fails, so neither can read as clean.
#[test]
fn the_whole_tree_reads_zero_findings() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let run = Command::new(env!("CARGO_BIN_EXE_check-docs"))
        .arg(&root)
        .output()?;
    assert_eq!(String::from_utf8_lossy(&run.stderr), "", "findings named");
    assert!(run.status.success(), "check-docs exited {}", run.status);
    Ok(())
}
