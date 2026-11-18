//! The rows a real file answers without a kill: a verdict that survives a
//! reopen, a torn or tampered tail, and parity with the in-memory journal.

use lgwks_bot::effect::{EffectKey, Id128};
use lgwks_bot::journal::{
    AttemptStatus, EffectEvent, EffectEvidence, EffectJournal, FileJournal, JournalError,
    Verification, VerificationResult,
};

use crate::journal_fixtures::{TempGuard, key_for as key, scratch_dir};

use super::{
    DIGEST_A, PREDICATE, RUNG_APPLIED, RUNG_PREPARED, RUNG_VERIFIED, TestResult, walk_ladder,
};

/// Append a partial frame to the journal's file, as an append interrupted
/// mid-write leaves it: bytes after the last acknowledged record.
fn tear_tail(
    journal_path: &std::path::Path,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(journal_path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// A scratch journal path with its cleanup guard, and the one key the verdict
/// tests climb, so each test states only what it appends and what it expects.
fn verdict_fixture(
    name: &str,
) -> Result<(TempGuard, std::path::PathBuf, EffectKey), Box<dyn std::error::Error>> {
    let dir = scratch_dir(name)?;
    let guard = TempGuard(dir.clone());
    Ok((guard, dir.join("journal.log"), key("1", DIGEST_A)?))
}

/// Append a `Verified` event for `key`, at the journal's own tail, saying that the
/// predicate at `version` over observations specific to that version `result`.
fn append_verdict(
    journal: &mut FileJournal,
    key: EffectKey,
    version: u64,
    result: VerificationResult,
) -> TestResult {
    journal.compare_and_append(
        journal.tail(),
        &EffectEvent::Verified {
            key,
            verification: Verification::new(
                Id128::from_hex(PREDICATE)?,
                version,
                lgwks_std::hash::blake3(format!("observed at version {version}").as_bytes()),
                result,
            ),
        },
    )?;
    Ok(())
}

// ── #257: a predicate that did not hold survives a reopen as its own state ──

/// A verification that did not hold is recovered from a real file, after the
/// handle that wrote it is gone, as a state of its own: not `Applied`, which an
/// attempt also reads as before any predicate ran, and not uncertainty, because
/// the effect is known to have landed. The answer comes from `Recovered`, with no
/// walk through the raw events.
#[test]
fn a_failed_verification_survives_a_reopen_as_its_own_state() -> TestResult {
    let (_guard, path, this_key) = verdict_fixture("failed-verification")?;
    {
        let mut journal = FileJournal::open(&path)?;
        walk_ladder(&mut journal, this_key, RUNG_APPLIED)?;
        append_verdict(&mut journal, this_key, 1, VerificationResult::NotSatisfied)?;
    }

    let reopened = FileJournal::open(&path)?.recover();
    assert_eq!(
        reopened.status(this_key),
        Some(AttemptStatus::VerificationFailed),
        "the failure is the recovered answer"
    );
    assert_ne!(reopened.status(this_key), Some(AttemptStatus::Applied));
    assert_eq!(
        reopened.uncertain().len(),
        0,
        "the effect landed, so a failed predicate is not an unknown outcome"
    );
    Ok(())
}

// ── #260: a verdict revised after a restart ────────────────────────────────

/// A `Verified` attempt is reopened after the world has moved, and a later
/// verification that no longer holds is recorded against the same key. The
/// recovered status follows it, names the predicate version it was decided at, and
/// keeps what it superseded, so an operator can read what changed after a restart.
#[test]
fn a_verdict_is_revised_by_a_later_verification_after_a_reopen() -> TestResult {
    let (_guard, path, this_key) = verdict_fixture("revised-verification")?;
    {
        let mut journal = FileJournal::open(&path)?;
        walk_ladder(&mut journal, this_key, RUNG_APPLIED)?;
        append_verdict(&mut journal, this_key, 1, VerificationResult::Satisfied)?;
    }
    {
        let mut journal = FileJournal::open(&path)?;
        assert_eq!(
            journal.recover().status(this_key),
            Some(AttemptStatus::Verified),
            "the first verdict survives the reopen"
        );
        append_verdict(&mut journal, this_key, 2, VerificationResult::NotSatisfied)?;
    }

    let recovered = FileJournal::open(&path)?.recover();
    assert_eq!(
        recovered.status(this_key),
        Some(AttemptStatus::VerificationFailed),
        "the later verdict is the recovered one"
    );
    assert_eq!(
        recovered
            .verification(this_key)
            .map(|found| found.predicate_version()),
        Some(2),
        "and it says which predicate version decided it"
    );
    let last = recovered
        .history(this_key)
        .last()
        .ok_or("a recovered attempt has a history")?;
    assert_eq!(
        (last.from(), last.to()),
        (
            Some(AttemptStatus::Verified),
            AttemptStatus::VerificationFailed
        ),
        "the history names what the verdict superseded"
    );
    assert_eq!(recovered.uncertain().len(), 0);
    Ok(())
}

// ── Row #102, recorded side: the failed append after a settlement ───────────

#[test]
fn a_settlement_followed_by_a_failed_recording_append_is_still_the_settlement() -> TestResult {
    let dir = scratch_dir("recording")?;
    let _guard = TempGuard(dir.clone());
    let journal_path = dir.join("journal.log");

    // The writer's handle is scoped: the fence is exclusive, so the reopen
    // below must not race a live first handle.
    let this_key = key("1", DIGEST_A)?;
    {
        let mut journal = FileJournal::open(&journal_path)?;
        walk_ladder(&mut journal, this_key, RUNG_APPLIED)?;

        // The recording failure: the next append dies mid-write. On a real
        // store that is a torn final frame — bytes after the last
        // acknowledged record.
        tear_tail(&journal_path, &[0x00, 0x00, 0x00])?;
    }

    let reopened = FileJournal::open(&journal_path)?;
    assert!(
        reopened.torn_tail_repaired(),
        "the interrupted append was never acknowledged; opening repairs it"
    );
    assert_eq!(
        reopened.recover().status(this_key),
        Some(AttemptStatus::Applied),
        "the settlement stands; a recording failure after it is not a refusal \
         and not an unknown"
    );
    assert_eq!(reopened.committed()?.len(), RUNG_APPLIED);
    Ok(())
}

// ── The mid-write kill, modelled at the byte level ──────────────────────────

#[test]
fn a_torn_final_frame_is_repaired_and_never_replayed() -> TestResult {
    let dir = scratch_dir("torn")?;
    let _guard = TempGuard(dir.clone());
    let journal_path = dir.join("journal.log");

    // The writer's handle is scoped: the fence is exclusive, so the reopen
    // below must not race a live first handle.
    let this_key = key("1", DIGEST_A)?;
    let acked_len;
    {
        let mut journal = FileJournal::open(&journal_path)?;
        walk_ladder(&mut journal, this_key, RUNG_PREPARED)?;
        acked_len = std::fs::metadata(&journal_path)?.len();

        // A kill mid-append leaves a partial frame. Both shapes a torn write
        // produces: a truncated frame body, and a truncated length prefix.
        tear_tail(&journal_path, &[0x00, 0x00, 0x01, 0x9f, 0xde, 0xad])?;
    }

    let mut reopened = FileJournal::open(&journal_path)?;
    assert!(reopened.torn_tail_repaired());
    assert_eq!(reopened.committed()?.len(), RUNG_PREPARED);
    assert_eq!(
        reopened.recover().status(this_key),
        Some(AttemptStatus::OutcomeUnknown),
        "the torn frame was never acknowledged, so it is not an answer"
    );
    assert_eq!(
        std::fs::metadata(&journal_path)?.len(),
        acked_len,
        "the repair truncates back to the acknowledged prefix"
    );
    // And the repaired journal still appends: the kill cost the un-acked
    // frame, nothing else. The continuation is the ladder's next rung for the
    // same key — the attempt it was for, not a new one.
    reopened.compare_and_append(
        reopened.tail(),
        &EffectEvent::OutcomeObserved {
            key: this_key,
            evidence: EffectEvidence::Applied,
        },
    )?;
    assert_eq!(reopened.committed()?.len(), RUNG_APPLIED);
    assert_eq!(
        reopened.recover().status(this_key),
        Some(AttemptStatus::Applied)
    );
    Ok(())
}

// ── Tamper: a well-framed but lying record is refused, never truncated ──────

#[test]
fn a_tampered_committed_frame_is_refused_rather_than_trimmed() -> TestResult {
    let dir = scratch_dir("tamper")?;
    let _guard = TempGuard(dir.clone());
    let journal_path = dir.join("journal.log");

    // The writer's handle is scoped: the fence is exclusive, so the refused
    // reopen below must not race a live first handle.
    {
        let mut journal = FileJournal::open(&journal_path)?;
        let this_key = key("1", DIGEST_A)?;
        walk_ladder(&mut journal, this_key, RUNG_APPLIED)?;

        // Flip one payload byte inside the last frame. This is not an
        // interrupted append; it is committed bytes that no longer mean what
        // the chain says.
        let mut bytes = std::fs::read(&journal_path)?;
        let first_len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert!(
            first_len > 16,
            "the first frame carries a real event, so the framing is real"
        );
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        std::fs::write(&journal_path, &bytes)?;
    }

    match FileJournal::open(&journal_path) {
        Err(JournalError::Corrupt(corruption)) => {
            // Three committed frames; the lying one is the third, named from
            // zero.
            assert_eq!(corruption.at(), 2, "the lying frame is the one named");
        }
        Err(other) => return Err(format!("expected a corruption refusal, got {other}").into()),
        Ok(_) => return Err("tampered committed bytes must not reopen as a journal".into()),
    }
    Ok(())
}

// ── Parity: the real store folds the same answers the in-memory one does ────

#[test]
fn the_file_journal_recovers_exactly_what_the_memory_journal_recovers() -> TestResult {
    let dir = scratch_dir("parity")?;
    let _guard = TempGuard(dir.clone());
    let journal_path = dir.join("journal.log");

    let mut file_journal = FileJournal::open(&journal_path)?;
    let this_key = key("1", DIGEST_A)?;
    walk_ladder(&mut file_journal, this_key, RUNG_VERIFIED)?;

    let mut memory_journal = lgwks_bot::journal::MemoryJournal::new();
    for event in file_journal.committed()? {
        memory_journal.compare_and_append(memory_journal.tail(), &event)?;
    }

    assert_eq!(
        file_journal.recover(),
        memory_journal.recover(),
        "one fold, one answer, whichever store carried the events"
    );
    assert_eq!(
        file_journal.recover().status(this_key),
        Some(AttemptStatus::Verified)
    );
    Ok(())
}
