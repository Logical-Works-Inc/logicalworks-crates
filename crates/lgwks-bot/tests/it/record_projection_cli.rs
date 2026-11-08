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
    assert_eq!(answers.len(), expected.len(), "{answers:?}");
    for (answered, wanted) in answers.iter().zip(expected) {
        assert!(answered.contains(wanted), "{answered} should say {wanted}");
    }
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
