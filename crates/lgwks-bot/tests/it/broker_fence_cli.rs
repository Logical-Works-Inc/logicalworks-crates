//! The shipped `broker-fence` binary, as a host lives it, under a real `kill -9`.
//!
//! Source: the Gap 2 issue's takeover requirement and a consumer's fence recovery
//! journey: a process that wrote its generation and died is fenced by whoever takes
//! over its journal, and the withdrawals and bound work through the public interface.

use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Command, Output, Stdio};

use crate::sim::scratch_file::ScratchFile;

/// A test's outcome: setup failures propagate instead of panicking.
type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The binary under test, wherever this run placed it.
///
/// Read at run time because an archived test run moves it between shards.
fn binary() -> std::path::PathBuf {
    for name in [
        "NEXTEST_BIN_EXE_broker-fence",
        "NEXTEST_BIN_EXE_broker_fence",
    ] {
        if let Some(found) = std::env::var_os(name) {
            return found.into();
        }
    }
    env!("CARGO_BIN_EXE_broker-fence").into()
}

/// Feed `script` to one process and read every line it answered.
fn session(scratch: &ScratchFile, script: &str) -> Outcome<Vec<String>> {
    let mut child = Command::new(binary())
        .arg(scratch.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("the child has a stdin")?
        .write_all(script.as_bytes())?;
    let done = child.wait_with_output()?;
    assert!(done.status.success(), "{}", stderr(&done));
    Ok(String::from_utf8(done.stdout)?
        .lines()
        .map(str::to_owned)
        .collect())
}

/// What a process wrote to standard error.
fn stderr(done: &Output) -> String {
    String::from_utf8_lossy(&done.stderr).into_owned()
}

/// Catches a takeover that loses the dead owner's generation, or restarts at one.
#[test]
fn a_takeover_after_kill_nine_moves_past_the_dead_owner() -> Outcome {
    let scratch = ScratchFile::new("broker-cli-kill")?;
    let mut owner = Command::new(binary())
        .arg(scratch.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = owner.stdin.take().ok_or("the owner has a stdin")?;
    input.write_all(b"record 7\nrecord 3\n")?;
    input.flush()?;
    let mut reader = BufReader::new(owner.stdout.take().ok_or("the owner has a stdout")?);
    let mut line = String::new();
    for _ in 0..2 {
        line.clear();
        reader.read_line(&mut line)?;
        assert!(line.contains("\"recorded\""), "{line}");
    }
    owner.kill()?;
    owner.wait()?;

    let answers = session(&scratch, "adopt\nauthorize 1 7\nauthorize 1 8\nstatus\n")?;
    assert_eq!(answers[0], "{\"ok\":true,\"generation\":8}");
    assert!(
        answers[1].contains("\"refused\":\"superseded\""),
        "{}",
        answers[1]
    );
    assert!(answers[2].contains("\"ok\":true"), "{}", answers[2]);
    assert!(answers[3].contains("\"generation\":8,"), "{}", answers[3]);
    Ok(())
}

/// Catches any door of the fence that the shipped binary cannot reach.
#[test]
fn a_host_withdraws_bounds_and_recharges_through_the_public_interface() -> Outcome {
    let scratch = ScratchFile::new("broker-cli-journey")?;
    let answers = session(
        &scratch,
        "register\nbound 1\nspend 2 1\nspend 2 1\nauthenticate 5\nspend 2 1\nspend 2 5\n\
         revoke 1\nrevoke 1\nauthenticate 5\nspend 1 5\nauthenticate 2\nreplace\nstatus\nclose\n\
         authorize 2 6\n",
    )?;
    let expected: [&str; 16] = [
        "\"generation\":1",
        "\"bound\":1",
        "\"generation\":1",
        "offline_spent",
        "\"generation\":5",
        "superseded",
        "\"generation\":5",
        "\"withdrew\":true",
        "\"withdrew\":false",
        "\"generation\":5",
        "\"refused\":\"revoked\"",
        "\"generation\":5",
        "\"generation\":6",
        "\"action_one_revoked\":true",
        "\"closed\":true",
        "\"refused\":\"closed\"",
    ];
    assert_eq!(answers.len(), expected.len(), "{answers:?}");
    for (answered, wanted) in answers.iter().zip(expected) {
        assert!(answered.contains(wanted), "{answered} should say {wanted}");
    }
    Ok(())
}

/// Catches a fat finger ending the session or reading as a generation.
#[test]
fn a_mistyped_command_is_answered_and_the_session_goes_on() -> Outcome {
    let scratch = ScratchFile::new("broker-cli-usage")?;
    let answers = session(
        &scratch,
        "fly\n\nauthenticate 0\nbound nine\nregister\nauthorize\n",
    )?;
    assert_eq!(answers.len(), 6, "{answers:?}");
    for index in [0_usize, 1, 2, 3, 5] {
        assert!(
            answers[index].contains("\"refused\":\"usage\""),
            "{}",
            answers[index]
        );
    }
    assert!(answers[4].contains("\"ok\":true"), "{}", answers[4]);
    let bare = Command::new(binary()).output()?;
    assert_eq!(bare.status.code(), Some(64));
    assert!(stderr(&bare).contains("usage: broker-fence"));
    Ok(())
}
