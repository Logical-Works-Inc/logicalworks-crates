//! The shipped `record-projection` binary under a real kill.
//!
//! Source: a consumer's restart journey (D04): after the process dies, a rebuild
//! from the journal is the view it had, down to the digest, and a poisoned view
//! refuses until it is rebuilt.

use std::path::PathBuf;
use std::process::Command;

use crate::sim::piped::{Outcome, Owner, located, session};
use crate::sim::scratch_file::ScratchFile;

/// The binary under test.
fn binary() -> PathBuf {
    located(
        [
            "NEXTEST_BIN_EXE_record-projection",
            "NEXTEST_BIN_EXE_record_projection",
        ],
        env!("CARGO_BIN_EXE_record-projection"),
    )
}

/// Each answer says what its place in `expected` wants, and there are no others.
fn says_each(answers: &[String], expected: &[&str]) {
    assert_eq!(answers.len(), expected.len(), "{answers:?}");
    for (answered, wanted) in answers.iter().zip(expected) {
        assert!(answered.contains(wanted), "{answered} should say {wanted}");
    }
}

/// Catches a rebuild that differs from the view a killed process held.
#[test]
fn a_view_rebuilt_after_a_kill_has_the_digest_the_dead_process_showed() -> Outcome {
    let scratch = ScratchFile::new("projection-cli-kill")?;
    let arguments = [scratch.path().as_os_str(), "16".as_ref()];
    let mut owner = Owner::start(&binary(), &arguments)?;
    let answered = owner.answers("fill 2 50\nfill 1 30\ncatch-up\nshow\n")?;
    owner.kill()?;
    let held = answered[3].trim();
    assert!(held.contains("\"rows\":2"), "{held}");
    assert!(held.contains("\"applied\":80"), "{held}");

    let after = session(&binary(), &arguments, "show\n")?;
    assert_eq!(after[0].trim(), held);
    Ok(())
}

/// Catches a poisoned view answering, and a rebuild past a record it must refuse.
#[test]
fn a_poisoned_view_refuses_until_a_rebuild_also_says_why() -> Outcome {
    let scratch = ScratchFile::new("projection-cli-poison")?;
    let answers = session(
        &binary(),
        &[scratch.path().as_os_str(), "16".as_ref()],
        "fill 1 3\ncatch-up\npoison 1\ncatch-up\ncatch-up\nshow\nrebuild\nshow\ncatch-up\nfly\n",
    )?;
    let expected: [&str; 10] = [
        "\"filled\":3",
        "\"folded\":3",
        "\"poisoned\":true",
        "\"refused\":\"fold\"",
        "\"refused\":\"poisoned\"",
        "\"applied\":3",
        "\"refused\":\"fold\"",
        "\"refused\":\"no_view\"",
        "\"refused\":\"no_view\"",
        "\"refused\":\"usage\"",
    ];
    says_each(&answers, &expected);
    Ok(())
}

/// Catches a staged record surviving a kill it was never synced through, and a synced
/// one being lost: the view after a restart is the file, whatever the dead one showed.
#[test]
fn a_kill_before_the_sync_loses_what_the_view_showed_and_a_kill_after_keeps_it() -> Outcome {
    let scratch = ScratchFile::new("projection-cli-stage")?;
    let arguments = [scratch.path().as_os_str(), "16".as_ref()];
    let mut owner = Owner::start(&binary(), &arguments)?;
    let answered = owner.answers("fill 1 2\nstage 2 3\nshow\nsync\nshow\nstage 3 4\nshow\n")?;
    owner.kill()?;
    assert!(answered[1].contains("\"staged\":3"), "{}", answered[1]);
    assert!(answered[2].contains("\"rows\":2"), "{}", answered[2]);
    assert!(answered[2].contains("\"applied\":5"), "{}", answered[2]);
    assert!(answered[3].contains("\"synced\":3"), "{}", answered[3]);
    assert!(answered[6].contains("\"applied\":9"), "{}", answered[6]);

    let after = session(&binary(), &arguments, "show\n")?;
    assert_eq!(
        after[0].trim(),
        answered[4].trim(),
        "the synced view, and not the one holding what the kill lost"
    );
    Ok(())
}

/// Catches a write jumping staged records, and a staged pile with no bound.
#[test]
fn staged_records_land_first_and_stop_at_their_limit() -> Outcome {
    let scratch = ScratchFile::new("projection-cli-limit")?;
    let answers = session(
        &binary(),
        &[scratch.path().as_os_str(), "16".as_ref()],
        "stage 1 4097\nstage 1 4096\nstage 1 1\nfill 1 1\npoison 1\nsync\nsync\nfill 1 1\nrebuild\nshow\n",
    )?;
    let expected: [&str; 10] = [
        "\"refused\":\"staged_limit\"",
        "\"staged\":4096",
        "\"refused\":\"staged_limit\"",
        "\"refused\":\"staged_first\"",
        "\"refused\":\"staged_first\"",
        "\"synced\":4096",
        "\"synced\":0",
        "\"filled\":1",
        "\"applied\":4097",
        "\"applied\":4097",
    ];
    says_each(&answers, &expected);
    Ok(())
}

/// Catches a staged record the fold refuses poisoning the view or leaving a row, and
/// a full view taking a new tenant (#905): each is refused before anything is staged.
#[test]
fn a_refused_stage_stages_nothing_and_the_view_keeps_answering() -> Outcome {
    let scratch = ScratchFile::new("projection-cli-admit")?;
    let answers = session(
        &binary(),
        &[scratch.path().as_os_str(), "2".as_ref()],
        "fill 1 2\nstage-poison 1\nstage-poison 5\nstage 1 2\nstage 2 1\nstage 3 1\nshow\nsync\nrebuild\nshow\n",
    )?;
    let expected: [&str; 10] = [
        "\"filled\":2",
        "\"refused\":\"fold\"",
        "\"refused\":\"fold\"",
        "\"staged\":2",
        "\"staged\":1",
        "\"refused\":\"row_limit\"",
        "\"rows\":2",
        "\"synced\":3",
        "\"applied\":5",
        "\"rows\":2",
    ];
    says_each(&answers, &expected);
    assert!(answers[6].contains("\"applied\":5"), "{}", answers[6]);
    assert_eq!(
        answers[6].trim(),
        answers[9].trim(),
        "the rebuilt view is the one that refused"
    );
    Ok(())
}

/// Catches a bad command line starting anything.
#[test]
fn a_missing_limit_is_a_usage_error() -> Outcome {
    let bare = Command::new(binary()).output()?;
    assert_eq!(bare.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&bare.stderr).contains("usage: record-projection"));
    Ok(())
}
