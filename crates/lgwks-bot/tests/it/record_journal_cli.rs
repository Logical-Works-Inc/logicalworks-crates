//! The shipped `record-journal` binary under a real `kill -9`.
//!
//! Source: the Gap 1 issue's durability claim, and a consumer's crash-restore
//! journey: what a process printed as acknowledged is on the disk after a kill, and
//! a restart keeps working from it. The process is really killed mid-append, at
//! whatever byte it had reached, so no line of this test can stage a clean stop.

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A test's outcome: setup failures propagate instead of panicking.
type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The binary under test, wherever this run placed it.
///
/// Read at run time because an archived test run moves it between shards.
fn binary() -> Outcome<PathBuf> {
    for name in [
        "NEXTEST_BIN_EXE_record-journal",
        "NEXTEST_BIN_EXE_record_journal",
    ] {
        if let Some(found) = std::env::var_os(name) {
            return Ok(PathBuf::from(found));
        }
    }
    Ok(PathBuf::from(env!("CARGO_BIN_EXE_record-journal")))
}

/// Run one command to its end.
fn run(args: &[&str]) -> Outcome<Output> {
    Ok(Command::new(binary()?).args(args).output()?)
}

/// The digits after `"key":` in a line of the binary's output.
fn field<'line>(line: &'line str, key: &str) -> Option<&'line str> {
    let (_, rest) = line.split_once(&format!("\"{key}\":"))?;
    rest.split([',', '}'])
        .next()
        .map(|value| value.trim_matches('"'))
}

/// A directory of this test's own, gone when the test is.
struct Scratch(PathBuf);

impl Scratch {
    /// A fresh directory named for this test, unique per call.
    fn new(name: &str) -> Outcome<Self> {
        let unique = lgwks_std::id::Uuid::new_v4()?;
        let dir = std::env::temp_dir().join(format!("lgwks-record-cli-{name}-{unique}"));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    /// The journal in this directory.
    fn journal(&self) -> PathBuf {
        self.0.join("journal")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(cause) = std::fs::remove_dir_all(&self.0) {
            lgwks_std::trace::warn!(error = %cause, "record_journal_cli: the scratch stayed");
        }
    }
}

/// Start an append, read `seen` acknowledgments, kill it mid-write, and return every
/// line it managed to print.
fn killed_after(path: &Path, seen: usize) -> Outcome<Vec<String>> {
    let text = path.to_str().ok_or("path")?;
    let mut child = Command::new(binary()?)
        .args(["append", text, "1000000", "256"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("stdout")?;
    let mut reader = BufReader::new(stdout);
    let mut lines = Vec::new();
    for _ in 0..seen {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        lines.push(line);
    }
    child.kill()?;
    child.wait()?;
    let mut rest = String::new();
    while reader.read_line(&mut rest)? > 0 {
        lines.push(core::mem::take(&mut rest));
    }
    Ok(lines)
}

/// Requirement: every printed acknowledgment survives a kill, three times over, and
/// the restarted process appends after them on one chain.
#[test]
fn a_killed_writer_keeps_every_acknowledgment_it_printed() -> Outcome {
    let scratch = Scratch::new("kill")?;
    let path = scratch.journal();
    let text = path.to_str().ok_or("path")?;
    for round in 0..3 {
        let printed = killed_after(&path, 50)?;
        let last = printed.last().ok_or("no acknowledgment was printed")?;
        let sequence: u64 = field(last, "ack").ok_or("ack")?.parse()?;
        let head = field(last, "head").ok_or("head")?;
        assert_eq!(field(last, "promise"), Some("process_crash"));

        let verified = run(&["page", text, &sequence.saturating_sub(1).to_string(), "1"])?;
        let entry = String::from_utf8(verified.stdout)?;
        assert_eq!(
            field(&entry, "head"),
            Some(head),
            "round {round}: acknowledged position {sequence} is not on the disk"
        );
        let report = run(&["verify", text])?;
        assert!(report.status.success(), "round {round}: reopen refused");
        let body = String::from_utf8(report.stdout)?;
        let tail: u64 = field(&body, "tail").ok_or("tail")?.parse()?;
        assert!(
            tail >= sequence,
            "round {round}: tail {tail} before ack {sequence}"
        );
        assert_eq!(field(&body, "committed"), field(&body, "replayed"));
    }
    Ok(())
}

/// Requirement: a seal stopped at each boundary resolves to one authority, across
/// real process exits, and the journal keeps taking appends.
#[test]
fn a_seal_stopped_at_each_boundary_leaves_one_writable_journal() -> Outcome {
    let boundaries = [
        "after_seal_write",
        "after_successor_write",
        "after_successor_sync",
        "after_directory_sync",
    ];
    for boundary in boundaries {
        let scratch = Scratch::new(boundary)?;
        let path = scratch.journal();
        let text = path.to_str().ok_or("path")?;
        assert!(run(&["append", text, "4", "32"])?.status.success());
        let stopped = run(&["continue", text, "checkpoint", boundary])?;
        assert_eq!(stopped.status.code(), Some(2), "{boundary}: not paused");
        assert!(
            run(&["append", text, "2", "32"])?.status.success(),
            "{boundary}"
        );
        let body = String::from_utf8(run(&["verify", text])?.stdout)?;
        assert_eq!(
            field(&body, "committed"),
            field(&body, "replayed"),
            "{boundary}"
        );
    }
    Ok(())
}

/// Requirement: a first-timer who types nothing, or something wrong, is told what
/// the tool takes and nothing is created.
#[test]
fn a_wrong_command_line_says_what_the_tool_takes() -> Outcome {
    let scratch = Scratch::new("usage")?;
    let path = scratch.journal();
    let text = path.to_str().ok_or("path")?;
    for args in [
        &[][..],
        &["nonsense", text][..],
        &["append", text, "many", "1"][..],
    ] {
        let refused = run(args)?;
        assert_eq!(refused.status.code(), Some(2));
        assert!(String::from_utf8(refused.stderr)?.contains("usage:"));
    }
    assert!(!path.exists() || std::fs::metadata(&path)?.len() == 0);
    Ok(())
}
