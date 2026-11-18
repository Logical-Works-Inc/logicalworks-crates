//! External observations for issue #109's release-gate register.
//!
//! The register's rows were repaired in code and unit-tested against
//! `MemoryJournal`, whose whole storage is a `Vec` in the process that would
//! have to survive the crash. What every row still needed was the *external*
//! observation: a real store on a real disk, a real process kill, a restart,
//! and an assertion that the recovered answer is the one the design names.
//! This file runs those observations.
//!
//! The kill is a real `SIGKILL` of a real child process (the test binary
//! re-executing itself in probe mode, the pattern `ecs_tick.rs` uses for its
//! watchdog). The store is [`FileJournal`] over a real file. What makes the
//! observation discriminating is the order the child writes: it appends
//! through the journal — whose append is `fsync`-ed before the acknowledgment
//! is minted — and only then touches a marker file. A marker therefore proves
//! the appends were acknowledged, and a kill after the marker proves the
//! acknowledgments were either kept or were lies.
//!
//! Rows observed here: #100 (external handoff admits on a durable record, and
//! the external marker never precedes the ack), #101 (a restart with a new
//! digest cannot fold into a completed attempt), #102 (a settlement followed
//! by a failed recording append is still the settlement), #104 (an unknown
//! stays a barrier across the kill), #106 (a live settlement survives the
//! kill; a duplicate stays idempotent).
//!
//! Rows still unobserved after this file: #99 (needs the ECS poll path under
//! a real store), #107 T21/T22 (needs a real descendant tree), #108 (needs a
//! real frame). They are named so they cannot quietly count as done.

use lgwks_bot::effect::EffectKey;
// The scratch-path and cleanup-guard fixtures this file shares with the
// journal liveness and scale families.
//
// One definition of "a unique scratch path" and of "remove it when the test
// ends", so this observation cannot drift into asserting a different cleanup
// discipline than the families that also observe a real kill.
use crate::journal_fixtures as shared;

use shared::{ProbeGuard, TempGuard, key_for as key, pause, scratch_dir};

use lgwks_bot::journal::{
    AttemptStatus, DurabilityPromise, EffectEvent, EffectEvidence, EffectJournal, FileJournal,
    JournalError,
};

/// T14: a real kill at each boundary of the ladder, the restart's reading
/// included, and the append killed while it is in flight.
mod boundary;

/// The rows a real file answers without a kill: a verdict across a reopen, a
/// torn or tampered tail, and parity with the in-memory journal.
mod on_disk;

const DIGEST_A: &str = "f0f1f2f3f4f5f6f7f8f9fafbfcfdfeffe0e1e2e3e4e5e6e7e8e9eaebecedeeef";
const DIGEST_B: &str = "efeeedecebeae9e8e7e6e5e4e3e2e1e0dfdedddcdbdad9d8d7d6d5d4d3d2d1d0";
const PREDICATE: &str = "4142434445464748494a4b4c4d4e4f50";

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// One ladder rung, counted from one, in the order the journal enforces.
const RUNG_PREPARED: usize = 2;
const RUNG_APPLIED: usize = 3;
const RUNG_VERIFIED: usize = 4;

/// Environment variable that turns this test binary into a probe child.
const PROBE_ENV: &str = "LGWKS_JOURNAL_PROBE";
/// Environment variables carrying the probe's orders.
const PROBE_JOURNAL: &str = "LGWKS_PROBE_JOURNAL";
const PROBE_EVENTS: &str = "LGWKS_PROBE_EVENTS";
const PROBE_MARKER: &str = "LGWKS_PROBE_MARKER";

/// Append the first `rungs` events of the standard ladder for `key`, each
/// through `compare_and_append` at the journal's own tail.
///
/// A prefix of the shared support module's ladder rather than a second copy of
/// it. Spelled out again here, the four rungs would be a second claim about what
/// an attempt looks like, and the two could come to disagree about which rung a
/// given attempt is on — which would make "the kill landed at this boundary" a
/// statement about the fixture rather than about the ladder.
fn walk_ladder(
    journal: &mut FileJournal,
    key: EffectKey,
    rungs: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    shared::walk_ladder(journal, key, PREDICATE, 1, rungs)
}

// ── The probe child ─────────────────────────────────────────────────────────

/// Run the probe body when this process is the child.
///
/// The child appends `PROBE_EVENTS` ladder events for attempt `"1"` to the
/// journal at `PROBE_JOURNAL` — each append fsync-ed before its acknowledgment
/// — then writes `PROBE_MARKER`, then parks. Parking is what makes the kill
/// land mid-flight: the parent decides when the process dies, and the only
/// facts the child has produced are the ones already on the disk.
fn probe_body() -> TestResult {
    let path = std::env::var_os(PROBE_JOURNAL)
        .ok_or("the probe child was started without a journal path")?;
    let events: usize = std::env::var(PROBE_EVENTS)
        .map_err(|error| format!("the probe child was started without an event count: {error}"))?
        .parse()
        .map_err(|error| format!("the probe event count was not a number: {error}"))?;
    let marker = std::env::var_os(PROBE_MARKER)
        .ok_or("the probe child was started without a marker path")?;

    let mut journal = FileJournal::open(&path)?;
    // The handoff gate on a real store: the admission half of row #100.
    let granted = journal.admit_external_handoff()?;
    assert_eq!(
        granted,
        DurabilityPromise::ProcessCrash,
        "the file journal must grant exactly the promise its appends earn"
    );

    walk_ladder(&mut journal, key("1", DIGEST_A)?, events)?;
    // The marker is written only after every append was acknowledged, so its
    // existence is the parent's proof that the acknowledgments were minted
    // before the process died.
    std::fs::write(&marker, b"acked")?;

    // Park until the parent kills us. Bounded, so a parent that never kills
    // cannot leave a stray process behind: the loop exits and the probe fails
    // loudly instead.
    for _ in 0..600 {
        pause(100);
    }
    Err("the probe child parked for its whole bound and was never killed".into())
}

/// Spawn this test binary as a probe child ordered to append `events` ladder
/// rungs and then park.
fn spawn_probe(
    test_name: &str,
    journal_path: &std::path::Path,
    marker_path: &std::path::Path,
    events: usize,
) -> Result<ProbeGuard, Box<dyn std::error::Error>> {
    Ok(ProbeGuard(Some(
        crate::probe_command(&crate::probe_test(module_path!(), test_name))?
            .env(PROBE_ENV, "1")
            .env(PROBE_JOURNAL, journal_path)
            .env(PROBE_EVENTS, events.to_string())
            .env(PROBE_MARKER, marker_path)
            .spawn()?,
    )))
}

/// Run the probe body instead of the test when this process is the child.
///
/// Every observation row is one function that is both the test and the probe it
/// spawns, which is what keeps a row's pre-kill appends and its parent's
/// post-restart reads in the same file. The dispatch is stated once here: a
/// row that forgot it would run its real body in the child, whose environment
/// names a journal it must instead park in, and the parent would hang waiting
/// for a marker no child ever writes.
fn dispatch_to_probe() -> Result<bool, Box<dyn std::error::Error>> {
    if std::env::var_os(PROBE_ENV).is_some() {
        probe_body()?;
        return Ok(true);
    }
    Ok(false)
}

/// One kill scenario's setup: a scratch directory, its guard, and the two paths
/// the probe child and its parent both need.
///
/// Three of the rows below are the same observation with a different rung count
/// and a different assertion after the restart, so the part before the
/// assertion is what they share. Naming it means the probe's contract — a
/// directory removed when the test ends, a journal under `journal.log`, a
/// liveness marker under `marker` — is stated once instead of being re-spelled
/// per row, and a row that forgets the guard fails the same way as the others
/// instead of silently leaking a directory.
///
/// The guard is returned rather than bound inside, because a `_guard` bound by
/// the caller is dropped at the end of that caller's scope, which is what makes
/// it live as long as the test.
fn kill_scenario(
    tag: &str,
    row: &str,
    events: usize,
) -> Result<(TempGuard, std::path::PathBuf), Box<dyn std::error::Error>> {
    let dir = scratch_dir(tag)?;
    let journal_path = dir.join("journal.log");
    let marker = dir.join("marker");
    let child = spawn_probe(row, &journal_path, &marker, events)?;
    kill_after_marker(child, &marker)?;
    Ok((TempGuard(dir), journal_path))
}

/// Wait until the probe's marker exists — its proof that every append was
/// acknowledged — then kill the child with a real `SIGKILL` and reap it.
///
/// `Child::kill` sends `SIGKILL` on Unix: no cleanup, no destructors, no
/// flushing. Everything the child wrote that is not on the disk is gone, and
/// everything that claimed to be durable had better be there.
fn kill_after_marker(
    mut guard: ProbeGuard,
    marker: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    guard.kill_after_marker(marker, "durable_crash_observation")
}

#[test]
fn a_settlement_recorded_before_a_real_kill_is_the_recovered_answer() -> TestResult {
    if dispatch_to_probe()? {
        return Ok(());
    }
    let (_guard, journal_path) = kill_scenario(
        "settlement",
        "a_settlement_recorded_before_a_real_kill_is_the_recovered_answer",
        RUNG_APPLIED,
    )?;

    // The restart: a fresh controller opens what is on the disk.
    let mut journal = FileJournal::open(&journal_path)?;
    let recovered = journal.recover();
    let settled = key("1", DIGEST_A)?;
    assert_eq!(
        recovered.status(settled),
        Some(AttemptStatus::Applied),
        "the kill happened after the outcome was acknowledged; \
         the recovered answer must be that outcome"
    );

    // The duplicate: a second OutcomeObserved for the settled key is not a
    // second settlement, it is a refused append. Idempotent by ladder.
    let duplicate = journal.compare_and_append(
        journal.tail(),
        &EffectEvent::OutcomeObserved {
            key: settled,
            evidence: EffectEvidence::Applied,
        },
    );
    match duplicate {
        Err(JournalError::OutOfOrder {
            expected: Some(lgwks_bot::journal::EventKind::Verified),
            ..
        }) => {}
        Err(other) => return Err(format!("expected an out-of-order refusal, got {other}").into()),
        Ok(_) => return Err("a duplicate settlement must not be appendable".into()),
    }

    // The journal is still a journal after the kill: a new key climbs fresh.
    let next = key("2", DIGEST_A)?;
    journal.compare_and_append(journal.tail(), &EffectEvent::IntentAdmitted { key: next })?;
    assert_eq!(journal.recover().len(), 2);
    Ok(())
}

// ── Rows #102 and #104: the unknown is a barrier across the kill ────────────

#[test]
fn a_kill_between_prepare_and_outcome_recovers_a_barrier_not_an_answer() -> TestResult {
    if dispatch_to_probe()? {
        return Ok(());
    }
    let (_guard, journal_path) = kill_scenario(
        "barrier",
        "a_kill_between_prepare_and_outcome_recovers_a_barrier_not_an_answer",
        RUNG_PREPARED,
    )?;

    let journal = FileJournal::open(&journal_path)?;
    let this_key = key("1", DIGEST_A)?;
    let recovered = journal.recover();
    assert_eq!(
        recovered.status(this_key),
        Some(AttemptStatus::OutcomeUnknown),
        "the bytes may have left; the only honest answer is unknown"
    );
    assert_ne!(
        recovered.status(this_key),
        Some(AttemptStatus::NotApplied),
        "nothing established that the bytes did not arrive"
    );
    assert_eq!(
        recovered.uncertain(),
        vec![this_key],
        "the barrier is named"
    );

    // The barrier settles by evidence, appended — never by a resend. The
    // restart appends the outcome and the attempt leaves uncertainty.
    let mut journal = journal;
    journal.compare_and_append(
        journal.tail(),
        &EffectEvent::OutcomeObserved {
            key: this_key,
            evidence: EffectEvidence::Applied,
        },
    )?;
    let settled = journal.recover();
    assert_eq!(settled.status(this_key), Some(AttemptStatus::Applied));
    assert!(settled.uncertain().is_empty());
    Ok(())
}

// ── Row #101: a new digest after the restart is a new identity ──────────────

#[test]
fn a_restart_with_a_new_digest_cannot_fold_into_a_completed_attempt() -> TestResult {
    if dispatch_to_probe()? {
        return Ok(());
    }
    let (_guard, journal_path) = kill_scenario(
        "identity",
        "a_restart_with_a_new_digest_cannot_fold_into_a_completed_attempt",
        RUNG_APPLIED,
    )?;

    // The restart re-admits the same logical action under a changed input:
    // attempt `"2"`, digest `B`. The binding is to the digest, so this is a
    // different attempt with its own fresh ladder, not a continuation of the
    // one the disk says already applied.
    let mut journal = FileJournal::open(&journal_path)?;
    let first = key("1", DIGEST_A)?;
    let rebound = key("2", DIGEST_B)?;
    assert_ne!(first, rebound);
    journal.compare_and_append(
        journal.tail(),
        &EffectEvent::IntentAdmitted { key: rebound },
    )?;

    let recovered = journal.recover();
    assert_eq!(recovered.len(), 2, "two identities, never one");
    assert_eq!(recovered.status(first), Some(AttemptStatus::Applied));
    assert_eq!(recovered.status(rebound), Some(AttemptStatus::Prepared));
    Ok(())
}

// ── Row #100: the external marker never precedes the durable ack ────────────

#[test]
fn an_external_marker_appears_only_after_the_durable_ack() -> TestResult {
    if dispatch_to_probe()? {
        return Ok(());
    }
    let dir = scratch_dir("handoff")?;
    let _guard = TempGuard(dir.clone());
    let journal_path = dir.join("journal.log");
    // For this row the marker is not scaffolding: it is the external effect
    // the row is about — an artifact on the disk, outside the dying process.
    // The child writes it only after its appends were acknowledged by a
    // journal that promises to survive the kill, so its existence plus the
    // kill is the observation.
    let marker = dir.join("external-effect");

    let child = spawn_probe(
        "an_external_marker_appears_only_after_the_durable_ack",
        &journal_path,
        &marker,
        RUNG_PREPARED,
    )?;
    kill_after_marker(child, &marker)?;

    let journal = FileJournal::open(&journal_path)?;
    let granted = journal.admit_external_handoff()?;
    assert!(
        granted.survives_process_crash(),
        "a real file journal must be the record a handoff may leave behind"
    );
    let this_key = key("1", DIGEST_A)?;
    assert_eq!(
        journal.recover().status(this_key),
        Some(AttemptStatus::OutcomeUnknown)
    );

    // The ordering the observation pins: the child wrote its external marker
    // only after the DispatchPrepared append was acknowledged, so both the
    // marker and the prepared record are here, and the record is on the disk
    // the kill did not touch.
    let committed = journal.committed()?;
    assert_eq!(committed.len(), RUNG_PREPARED);
    assert!(matches!(
        committed.last(),
        Some(EffectEvent::DispatchPrepared { .. })
    ));
    Ok(())
}
