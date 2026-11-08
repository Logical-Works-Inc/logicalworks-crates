//! The shipped `recorded-instant` binary, as a consumer lives it, under a real `kill -9`.
//!
//! Source: #845, a consumer's runtime records a clock reading, dies and restarts on a
//! host whose system clock may have gone backwards, and replays a seeded run elsewhere.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

use crate::sim::piped::{self, Outcome, Owner, located};
use crate::sim::scratch_file::ScratchFile;

/// The binary under test.
fn binary() -> PathBuf {
    located(
        [
            "NEXTEST_BIN_EXE_recorded-instant",
            "NEXTEST_BIN_EXE_recorded_instant",
        ],
        env!("CARGO_BIN_EXE_recorded-instant"),
    )
}

/// Feed `script` to one seeded process and read every line it answered.
fn seeded(scratch: &ScratchFile, seed: &str, script: &str) -> Outcome<Vec<String>> {
    let arguments = [scratch.path().as_os_str(), OsStr::new(seed)];
    piped::session(&binary(), &arguments, script)
}

/// The number a line carries under `name`.
fn field(line: &str, name: &str) -> Outcome<u64> {
    let (_, after) = line
        .split_once(&format!("\"{name}\":"))
        .ok_or_else(|| format!("{line} has no {name}"))?;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    Ok(digits.parse()?)
}

/// Catches a restart that re-reads the system clock with no floor: the record is far
/// ahead of any real clock, which is what a clock stepped backwards looks like.
#[test]
fn a_live_restart_after_kill_nine_never_reads_before_the_last_record() -> Outcome {
    let scratch = ScratchFile::new("instant-cli-kill")?;
    let ahead = format!("{}", u64::MAX);
    let written = seeded(&scratch, &ahead, "record\n")?;
    let floor = field(&written[0], "nanos")?;

    let mut owner = Owner::start(&binary(), &[scratch.path()])?;
    let answers = owner.answers("last\nnow\nrecord\n")?;
    owner.kill()?;
    assert_eq!(field(&answers[0], "nanos")?, floor);
    assert!(field(&answers[1], "nanos")? >= floor, "{}", answers[1]);
    let second = field(&answers[2], "nanos")?;
    assert!(second >= floor, "{}", answers[2]);
    assert_eq!(field(&answers[2], "sequence")?, 2, "{}", answers[2]);

    let reopened = piped::session(&binary(), &[scratch.path()], "last\nnow\n")?;
    assert_eq!(field(&reopened[0], "nanos")?, second);
    assert!(field(&reopened[1], "nanos")? >= second, "{}", reopened[1]);
    Ok(())
}

/// Catches a seeded clock that reads real time anywhere on its path.
#[test]
fn one_seed_and_one_script_print_the_same_bytes_in_two_processes() -> Outcome {
    let script = "now\nadvance 5\nrecord\nsince 1\nelapsed\nlast\n";
    let first = seeded(&ScratchFile::new("instant-cli-seed-a")?, "1000", script)?;
    let second = seeded(&ScratchFile::new("instant-cli-seed-b")?, "1000", script)?;
    let other = seeded(&ScratchFile::new("instant-cli-seed-c")?, "2000", script)?;
    assert_eq!(first, second);
    assert_ne!(first, other);
    assert_eq!(field(&first[0], "nanos")?, 500);
    assert_eq!(field(&first[1], "nanos")?, 505);
    assert_eq!(field(&first[3], "since")?, 504);
    assert_eq!(field(&first[4], "elapsed")?, 5);
    assert_eq!(field(&first[5], "nanos")?, 505);
    Ok(())
}

/// Catches a restored clock that drifts, or one that a live journal cannot stand on.
#[test]
fn a_restored_clock_stands_at_the_record_and_moves_only_when_told() -> Outcome {
    let scratch = ScratchFile::new("instant-cli-restore")?;
    let written = seeded(&scratch, "90", "advance 7\nrecord\n")?;
    assert_eq!(field(&written[1], "nanos")?, 52);

    let answers = piped::session(
        &binary(),
        &[scratch.path()],
        "advance 1\nrestore\nnow\nnow\nadvance 8\nsince 999\nsince 50\n",
    )?;
    assert!(
        answers[0].contains("\"refused\":\"not_virtual\""),
        "{}",
        answers[0]
    );
    assert_eq!(field(&answers[1], "nanos")?, 52);
    assert_eq!(field(&answers[2], "nanos")?, 52);
    assert_eq!(field(&answers[3], "nanos")?, 52);
    assert_eq!(field(&answers[4], "nanos")?, 60);
    assert_eq!(field(&answers[5], "since")?, 0, "a later moment saturates");
    assert_eq!(field(&answers[6], "since")?, 10);
    Ok(())
}

/// Catches a fat finger ending the session, and a restore with nothing to stand on.
#[test]
fn a_mistyped_command_is_answered_and_the_session_goes_on() -> Outcome {
    let scratch = ScratchFile::new("instant-cli-usage")?;
    let answers = seeded(
        &scratch,
        "4",
        "fly\n\nadvance nine\nsince\nrestore\nlast\nadvance 18446744073709551615\nadvance 1\nnow\n",
    )?;
    assert_eq!(answers.len(), 9, "{answers:?}");
    for line in &answers[..4] {
        assert!(line.contains("\"refused\":\"usage\""), "{line}");
    }
    assert!(
        answers[4].contains("\"refused\":\"nothing_recorded\""),
        "{}",
        answers[4]
    );
    assert!(answers[5].contains("\"nanos\":null"), "{}", answers[5]);
    assert_eq!(
        field(&answers[6], "nanos")?,
        u64::MAX,
        "saturates, never wraps"
    );
    assert!(
        answers[7].contains("\"refused\":\"out_of_range\""),
        "{}",
        answers[7]
    );
    assert_eq!(field(&answers[8], "nanos")?, u64::MAX);

    let bare = Command::new(binary()).output()?;
    assert_eq!(bare.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&bare.stderr).contains("usage: recorded-instant"));
    let bad_seed = Command::new(binary())
        .args(["journal", "twelve"])
        .output()?;
    assert_eq!(bad_seed.status.code(), Some(64));
    Ok(())
}
