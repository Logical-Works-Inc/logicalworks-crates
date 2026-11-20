//! A real `SIGKILL` at every seal boundary: a child process stops its
//! continuation at each declared boundary and is killed there, and the parent
//! reopens what the kill left.

use std::path::{Path, PathBuf};

use lgwks_bot::journal::{AttemptStatus, EffectEvent, FileJournal, JournalError, SealPause};

use super::{
    PREDICATE, TestResult, append, continue_once, describe, fill, key_for_action, policy,
    shared_action, tenant_action,
};
use crate::journal_fixtures::{TempGuard, scratch_dir};

/// Turns this binary into the seal-boundary probe child.
const SEAL_PROBE_ENV: &str = "LGWKS_SEAL_PROBE";
/// The directory the probe child's journal lives in.
const SEAL_PROBE_DIR: &str = "LGWKS_SEAL_PROBE_DIR";
/// Which of [`SealPause::all`] the probe child stops its continuation at.
const SEAL_PROBE_BOUNDARY: &str = "LGWKS_SEAL_PROBE_BOUNDARY";
/// The libtest name the probe child is started under.
const TEST_SEAL_KILL: &str =
    "a_kill_at_every_seal_boundary_leaves_exactly_one_authoritative_journal";
/// Settled attempts the probe child writes before it continues.
///
/// Well under the sixty-four-event trigger, so the only continuation in the run is
/// the armed one the kill lands inside.
const SEAL_PROBE_SETTLED: u64 = 9;
/// The attempt the probe child leaves unknown, so the kill is also a test of the
/// carry: an attempt with no outcome must be unknown in whichever file survives.
const SEAL_PROBE_UNKNOWN: u64 = SEAL_PROBE_SETTLED + 1;

/// The file the probe child writes once its continuation has stopped.
fn seal_marker(dir: &Path) -> PathBuf {
    dir.join("stopped")
}

/// The probe child: settle some attempts, leave one unknown, stop the seal at the
/// ordered boundary, announce it, and park until killed.
///
/// The stop is the shipped fault door, so the bytes on the disk at the kill are
/// exactly the ones a process that died at that instruction would leave: the
/// handle refuses every later write, and the kill runs no destructor that could
/// write one anyway.
fn seal_probe_body() -> TestResult {
    let dir = PathBuf::from(
        std::env::var_os(SEAL_PROBE_DIR).ok_or("the seal probe was started without a directory")?,
    );
    let index: usize = std::env::var(SEAL_PROBE_BOUNDARY)?.parse()?;
    let boundary = *SealPause::all()
        .get(index)
        .ok_or("the seal probe was given a boundary past the declared set")?;
    let mut journal = FileJournal::open_continuing_with(dir.join("run.jrnl"), policy()?)?;
    fill(&mut journal, 1, SEAL_PROBE_SETTLED)?;
    let unknown = key_for_action(shared_action()?, SEAL_PROBE_UNKNOWN)?;
    crate::journal_fixtures::walk_ladder(&mut journal, unknown, PREDICATE, 1, 2)?;
    journal.arm_continuation_pause(boundary);
    match journal.continue_as_file() {
        Err(JournalError::ContinuationPaused { boundary: at }) if at == boundary => {}
        other => {
            let refusal: TestResult =
                Err(format!("the seal did not stop at {boundary}: {}", describe(&other)).into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "seal_probe_body: the armed seal ran past its boundary");
            return refusal;
        }
    }
    let mut marker = std::fs::File::create(seal_marker(&dir))?;
    std::io::Write::write_all(&mut marker, b"stopped")?;
    marker.sync_all()?;
    for _ in 0..600 {
        crate::journal_fixtures::pause(100);
    }
    Err("the seal probe parked for its whole bound and was never killed".into())
}

/// A continuation killed by `SIGKILL` at each of its write and sync boundaries
/// leaves exactly one authoritative journal, holding exactly what was written.
///
/// For every boundary the child stops at, a reopen through the original path must:
///
/// - find one live journal, and the other file must refuse to be appended to:
///   either the predecessor is sealed and names its successor, or the successor
///   never became a journal;
/// - hold every settled attempt as settled and refuse each one a second time,
///   which is "nothing lost and nothing duplicated" at the journal's boundary;
/// - hold the attempt the child left unknown as unknown, wherever it survived;
/// - and continue again on demand, so a crash mid-continuation is a delay rather
///   than a journal that can never continue.
#[test]
fn a_kill_at_every_seal_boundary_leaves_exactly_one_authoritative_journal() -> TestResult {
    if std::env::var_os(SEAL_PROBE_ENV).is_some() {
        return seal_probe_body();
    }
    for (index, boundary) in SealPause::all().into_iter().enumerate() {
        let dir = scratch_dir("seal-kill")?;
        let _guard = TempGuard(dir.clone());
        let mut command = crate::probe_command(&crate::probe_test(module_path!(), TEST_SEAL_KILL))?;
        command
            .env(SEAL_PROBE_ENV, "1")
            .env(SEAL_PROBE_DIR, &dir)
            .env(SEAL_PROBE_BOUNDARY, index.to_string());
        let mut child = crate::journal_fixtures::ProbeGuard(Some(command.spawn()?));
        child.kill_after_marker(&seal_marker(&dir), TEST_SEAL_KILL)?;
        reopen_after_seal_kill(&dir, boundary)?;
    }
    Ok(())
}

/// Every assertion a reopen after a kill at `boundary` must satisfy.
fn reopen_after_seal_kill(dir: &Path, boundary: SealPause) -> TestResult {
    let base = dir.join("run.jrnl");
    let successor = dir.join("run.jrnl.cont").join("000001");
    let mut live = FileJournal::open_active(&base)?;
    let other = if live.path() == base.as_path() {
        &successor
    } else {
        &base
    };
    if other.exists() {
        let foreign = EffectEvent::IntentAdmitted {
            key: key_for_action(tenant_action(77)?, 1)?,
        };
        let refused =
            FileJournal::open(other).and_then(|mut journal| append(&mut journal, &foreign));
        assert!(
            refused.is_err(),
            "after a kill at {boundary}, {} is live and {} still took an append: two \
             journals are authoritative",
            live.path().display(),
            other.display()
        );
    }

    let latest = key_for_action(shared_action()?, SEAL_PROBE_SETTLED)?;
    let unknown = key_for_action(shared_action()?, SEAL_PROBE_UNKNOWN)?;
    let recovered = live.recover();
    assert_eq!(
        recovered.status(unknown),
        Some(AttemptStatus::OutcomeUnknown),
        "after a kill at {boundary}, the attempt left unknown is still unknown in {}",
        live.path().display()
    );
    assert_eq!(
        recovered.status(latest),
        Some(AttemptStatus::Verified),
        "after a kill at {boundary}, the last settled attempt is settled in {}",
        live.path().display()
    );
    for attempt in 1..=SEAL_PROBE_SETTLED {
        let again = EffectEvent::IntentAdmitted {
            key: key_for_action(shared_action()?, attempt)?,
        };
        assert!(
            append(&mut live, &again).is_err(),
            "after a kill at {boundary}, settled attempt {attempt} was admitted a second time"
        );
    }

    if live.generation() == 1 {
        let continued = continue_once(&mut live)?;
        assert_eq!(
            continued.recover().status(unknown),
            Some(AttemptStatus::OutcomeUnknown),
            "a continuation completed after a kill at {boundary} still carries the unknown"
        );
    }
    Ok(())
}
