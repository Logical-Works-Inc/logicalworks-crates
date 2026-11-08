//! The shipped `durable-stop` binary under a real kill.
//!
//! Source: a consumer's cancellation and cursor journeys (E30, E35): a stop asked for
//! before a kill stops the next process, and its saved place comes back.

use std::path::PathBuf;
use std::process::Command;

use crate::sim::piped::{Outcome, Owner, located, session};
use crate::sim::scratch_file::ScratchFile;

/// The binary under test.
fn binary() -> PathBuf {
    located(
        [
            "NEXTEST_BIN_EXE_durable-stop",
            "NEXTEST_BIN_EXE_durable_stop",
        ],
        env!("CARGO_BIN_EXE_durable-stop"),
    )
}

/// A journal, a store and the arguments naming them, for one tenant and consumer.
fn arguments(scratch: &ScratchFile, tenant: &str) -> Outcome<Vec<String>> {
    let journal = scratch.path().display().to_string();
    let store = format!("{journal}-store");
    std::fs::create_dir_all(&store)?;
    Ok(vec![journal, store, tenant.to_owned(), "mailer".to_owned()])
}

/// Catches a stop or a place that dies with the process that recorded it.
#[test]
fn a_stop_and_a_place_recorded_before_a_kill_hold_in_the_next_process() -> Outcome {
    let scratch = ScratchFile::new("stop-cli-kill")?;
    let words = arguments(&scratch, "acme")?;
    let mut owner = Owner::start(&binary(), &words)?;
    let first = owner.answers("status\nwork 1\nadvance 40\nrequest\n")?;
    owner.kill()?;
    assert!(first[0].contains("\"may_start\":true"), "{}", first[0]);
    assert!(first[3].contains("\"stage\":\"Requested\""), "{}", first[3]);

    let after = session(
        &binary(),
        &words,
        "status\ncursor\nadvance 39\nbegin\nfinish\nstatus\n",
    )?;
    let expected: [&str; 6] = [
        "\"stage\":\"Requested\",\"fired\":true,\"may_start\":false",
        "\"cursor\":40",
        "\"refused\":\"behind\"",
        "\"stage\":\"Cancelling\"",
        "\"stage\":\"Finished\"",
        "\"stage\":\"Finished\",\"fired\":true,\"may_start\":false",
    ];
    assert_eq!(after.len(), expected.len(), "{after:?}");
    for (answered, wanted) in after.iter().zip(expected) {
        assert!(answered.contains(wanted), "{answered} should say {wanted}");
    }
    Ok(())
}

/// Catches a stage out of order and a fat finger ending the session.
#[test]
fn a_misused_session_is_answered_and_goes_on() -> Outcome {
    let scratch = ScratchFile::new("stop-cli-misuse")?;
    let words = arguments(&scratch, "acme")?;
    let answers = session(&binary(), &words, "finish\nfly\nadvance\nrequest\n")?;
    assert!(
        answers[0].contains("\"refused\":\"out_of_order\""),
        "{}",
        answers[0]
    );
    assert!(
        answers[1].contains("\"refused\":\"usage\""),
        "{}",
        answers[1]
    );
    assert!(
        answers[2].contains("\"refused\":\"usage\""),
        "{}",
        answers[2]
    );
    assert!(
        answers[3].contains("\"stage\":\"Requested\""),
        "{}",
        answers[3]
    );
    Ok(())
}

/// Catches a tenant the store would refuse starting anything, and a short command line.
#[test]
fn a_bad_command_line_says_so() -> Outcome {
    let bare = Command::new(binary()).output()?;
    assert_eq!(bare.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&bare.stderr).contains("usage: durable-stop"));

    let scratch = ScratchFile::new("stop-cli-tenant")?;
    let words = arguments(&scratch, "not a tenant!")?;
    let refused = Command::new(binary()).args(&words).output()?;
    assert_eq!(refused.status.code(), Some(2));
    Ok(())
}
