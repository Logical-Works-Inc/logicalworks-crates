//! T14: the boundaries the ladder has, each against a real kill.
//!
//! The row names five crashes and one property: crash before intent ack, after
//! ack before dispatch, after dispatch before response, after response before
//! receipt, and during recovery; and no unknown effect is blindly replayed.
//!
//! The rows in the parent cover two of them under a real kill. The rows below
//! are the remaining boundaries, and each is *modelled at the point the crash
//! names* rather than by waiting for a window to open:
//!
//! - **after ack before dispatch** is the probe that acknowledges `IntentAdmitted`
//!   and is killed before it appends the preparation. The window between those two
//!   appends is where a controller decides whether to hand over, and the honest
//!   recovered answer is `Prepared` — the intent is on the disk and nothing has
//!   left the process, so the attempt is eligible for an attempt again and not
//!   unknown.
//! - **after response before receipt** is the probe that walks the whole ladder
//!   and is killed after the outcome is on the disk but before anything verified
//!   it. The outcome is the fact; `Verified` is the attestation. A kill between
//!   them must recover the outcome, because the effect landed and a recovery path
//!   that re-dispatched on the missing verification would duplicate it.
//! - **during recovery** is the probe that is killed while it is *reading* a
//!   journal a previous process wrote — the restart itself. Nothing new is
//!   appended, so the file is unchanged, and the third restart reads the same
//!   history: recovery that mutates is a recovery that can lose.
//! - **before the intent ack** is the probe killed while its first append is in
//!   flight on a store that is not answering.
//!
//! Each row is one function that is both the test and the probe it spawns. The
//! spawners live here, beside their rows, because a probe names its test by this
//! module's own path.

use lgwks_bot::effect::Id128;
use lgwks_bot::journal::{
    AttemptStatus, DurabilityPromise, EffectEvent, EffectEvidence, EffectJournal, FileJournal,
    JournalError, Verification, VerificationResult,
};

use crate::journal_fixtures::{ProbeGuard, TempGuard, key_for as key, pause, scratch_dir};

use super::{
    DIGEST_A, PREDICATE, PROBE_JOURNAL, PROBE_MARKER, RUNG_APPLIED, TestResult, kill_after_marker,
    walk_ladder,
};

/// Turns this test binary into a probe child that parks mid-append, with its
/// storage device held closed, and waits to be killed with nothing written.
const PROBE_STALLED: &str = "LGWKS_PROBE_STALLED";

/// What one probe child did before it was killed, for the parent to read.
///
/// A report file rather than a marker, because these rows assert *which* events
/// reached the disk and a marker can only say that the child got that far. The
/// child writes it after its last acknowledged append and before it parks, so
/// the report is itself evidence that those appends were acknowledged before the
/// kill.
const PROBE_REPORT: &str = "LGWKS_PROBE_REPORT";

/// The environment variable naming which ladder boundary a probe is killed at.
const PROBE_RUNG_ENV: &str = "LGWKS_PROBE_RUNG";

/// What the probe child was told to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeRungs {
    /// Acknowledge `IntentAdmitted`, park, and be killed before the preparation.
    AckBeforeDispatch,
    /// Walk the whole ladder, park, and be killed before the verification.
    ResponseBeforeReceipt,
    /// Append nothing at all: park holding the journal, and be killed while a
    /// restart is reading it.
    DuringRecovery,
}

impl ProbeRungs {
    /// The environment-variable spelling the child reads.
    const fn as_str(self) -> &'static str {
        match self {
            Self::AckBeforeDispatch => "ack-before-dispatch",
            Self::ResponseBeforeReceipt => "response-before-receipt",
            Self::DuringRecovery => "during-recovery",
        }
    }

    /// The spelling this probe writes.
    fn from_str(text: &str) -> Result<Self, String> {
        match text {
            "ack-before-dispatch" => Ok(Self::AckBeforeDispatch),
            "response-before-receipt" => Ok(Self::ResponseBeforeReceipt),
            "during-recovery" => Ok(Self::DuringRecovery),
            other => Err(format!("{other:?} is not a probe rung this file knows")),
        }
    }
}

/// The probe child for the three boundary rows.
///
/// Each rung shape is the real thing: a `FileJournal` over a real file, appends
/// that are `fsync`-ed before their acknowledgments, and a real park the parent
/// decides when to end with a `SIGKILL`. The difference between the shapes is
/// only *where* in the ladder the kill lands, which is the thing each row is
/// about.
fn boundary_probe_body() -> TestResult {
    let path = std::env::var_os(PROBE_JOURNAL)
        .ok_or("the boundary probe was started without a journal path")?;
    let marker = std::env::var_os(PROBE_MARKER)
        .ok_or("the boundary probe was started without a marker path")?;
    let report = std::env::var_os(PROBE_REPORT)
        .ok_or("the boundary probe was started without a report path")?;
    let rung = ProbeRungs::from_str(
        &std::env::var(PROBE_RUNG_ENV)
            .map_err(|error| format!("the boundary probe was started without a rung: {error}"))?,
    )
    .map_err(std::io::Error::other)?;

    let key = key("1", DIGEST_A)?;
    let mut journal = FileJournal::open(&path)?;
    let handoff = journal.admit_external_handoff()?;
    assert_eq!(
        handoff,
        DurabilityPromise::ProcessCrash,
        "the file journal must grant exactly the promise its appends earn"
    );

    match rung {
        // Acknowledge the intent and stop. Nothing is prepared, so nothing has
        // left the process, and this is the boundary the row calls "after ack
        // before dispatch".
        ProbeRungs::AckBeforeDispatch => {
            journal.compare_and_append(journal.tail(), &EffectEvent::IntentAdmitted { key })?;
        }
        // Walk the ladder to the outcome and stop one rung short of the
        // verification: the response is recorded, its receipt is not.
        ProbeRungs::ResponseBeforeReceipt => {
            walk_ladder(&mut journal, key, RUNG_APPLIED)?;
        }
        // Fold a journal a previous process wrote and hold it. This probe
        // appends nothing at all, so the file is exactly what the parent left.
        ProbeRungs::DuringRecovery => {
            // A full recovery fold, not just a read: this is the restart doing
            // the work a restart does, and the parent's assertion is that the
            // fold moved nothing.
            let folded = journal.recover();
            assert!(
                !folded.is_empty(),
                "the journal this process was told to recover holds an attempt"
            );
        }
    }

    std::fs::write(&report, rung.as_str())?;
    std::fs::write(&marker, b"acked")?;

    // Park until the parent kills us. Bounded, so a parent that never kills
    // cannot leave a stray process behind.
    for _ in 0..600 {
        pause(100);
    }
    Err("the boundary probe parked for its whole bound and was never killed".into())
}

/// Run the boundary probe when this process is the child.
fn dispatch_to_boundary_probe() -> Result<bool, Box<dyn std::error::Error>> {
    if std::env::var_os(PROBE_RUNG_ENV).is_some() {
        boundary_probe_body()?;
        return Ok(true);
    }
    Ok(false)
}

/// Spawn one boundary probe and kill it once it has acknowledged its last append.
fn spawn_boundary_probe(
    test_name: &str,
    dir: &std::path::Path,
    rung: ProbeRungs,
) -> Result<(std::path::PathBuf, TempGuard), Box<dyn std::error::Error>> {
    let journal_path = dir.join("journal.log");
    let marker = dir.join("marker");
    let report = dir.join("report");
    let child = crate::probe_command(&crate::probe_test(module_path!(), test_name))?
        .env(PROBE_RUNG_ENV, rung.as_str())
        .env(PROBE_JOURNAL, &journal_path)
        .env(PROBE_MARKER, &marker)
        .env(PROBE_REPORT, &report)
        .spawn()?;
    kill_after_marker(ProbeGuard(Some(child)), &marker)?;
    Ok((journal_path, TempGuard(dir.to_path_buf())))
}

/// Spawn this test binary as a probe child that parks mid-append.
fn spawn_stalled_probe(
    test_name: &str,
    journal_path: &std::path::Path,
    marker_path: &std::path::Path,
) -> Result<ProbeGuard, Box<dyn std::error::Error>> {
    Ok(ProbeGuard(Some(
        crate::probe_command(&crate::probe_test(module_path!(), test_name))?
            .env(PROBE_STALLED, "1")
            .env(PROBE_JOURNAL, journal_path)
            .env(PROBE_MARKER, marker_path)
            .spawn()?,
    )))
}

// ── T14 row 1: killed after the intent ack, before the dispatch ─────────────

/// A kill after the intent is acknowledged and before anything is dispatched
/// recovers as `Prepared`, and the attempt may be dispatched because nothing has
/// left the process.
///
/// The half of the row that a "recover as unknown" answer would break. A kill
/// between the admission and the preparation has *not* handed anything over:
/// there is no `DispatchPrepared`, so no bytes are in flight, and a recovery path
/// that answered `OutcomeUnknown` here would make an effect that provably never
/// left the process sit behind a barrier forever. The control is the row below
/// this one, which kills the same child one rung later and gets the opposite
/// answer — so this is not a blanket "everything recovers as Prepared".
#[test]
fn a_kill_after_the_intent_ack_and_before_the_dispatch_recovers_as_prepared_t14() -> TestResult {
    if dispatch_to_boundary_probe()? {
        return Ok(());
    }
    let dir = scratch_dir("ack-before-dispatch")?;
    let (journal_path, _guard) = spawn_boundary_probe(
        "a_kill_after_the_intent_ack_and_before_the_dispatch_recovers_as_prepared_t14",
        &dir,
        ProbeRungs::AckBeforeDispatch,
    )?;

    let journal = FileJournal::open(&journal_path)?;
    let this_key = key("1", DIGEST_A)?;
    let committed = journal.committed()?;
    assert_eq!(
        committed.len(),
        1,
        "the acknowledged intent is the only event the kill could leave"
    );
    assert!(
        matches!(committed.first(), Some(EffectEvent::IntentAdmitted { .. })),
        "and it is the admission, not a preparation: {committed:?}"
    );

    let recovered = journal.recover();
    assert_eq!(
        recovered.status(this_key),
        Some(AttemptStatus::Prepared),
        "nothing was handed over, so the recovered answer is Prepared"
    );
    assert!(
        recovered.uncertain().is_empty(),
        "an attempt that was never dispatched is not an unknown one: {:?}",
        recovered.uncertain()
    );

    // And it is genuinely dispatchable: the next rung for the same key is the
    // preparation, and a recovery path that only *thought* it was prepared would
    // be refused here.
    let mut journal = journal;
    journal.compare_and_append(
        journal.tail(),
        &EffectEvent::DispatchPrepared { key: this_key },
    )?;
    assert_eq!(
        journal.recover().status(this_key),
        Some(AttemptStatus::OutcomeUnknown),
        "dispatching it moves the attempt to the barrier, which is the other \
         row's answer"
    );
    Ok(())
}

// ── T14 row 2: killed after the response, before the receipt ───────────────

/// A kill after the outcome is recorded and before it is verified recovers the
/// outcome, and the effect is not dispatched again.
///
/// The boundary that decides whether a verified-but-unrecorded effect is
/// duplicated. The outcome append is the *fact* — the bytes reached the world —
/// and the verification is an attestation about that fact made afterwards. A kill
/// between them must therefore recover `Applied` and leave nothing uncertain: a
/// recovery path that treated the missing verification as an unknown would offer
/// to dispatch the effect a second time, and the journal is what refuses it.
///
/// The ladder is the mechanism, and it is asserted rather than assumed: the
/// `Verified` rung for this key is now the only one left, so the duplicate
/// settlement the row forbids is not merely discouraged but unrepresentable.
#[test]
fn a_kill_after_the_response_and_before_the_receipt_recovers_the_outcome() -> TestResult {
    if dispatch_to_boundary_probe()? {
        return Ok(());
    }
    let dir = scratch_dir("response-before-receipt")?;
    let (journal_path, _guard) = spawn_boundary_probe(
        "a_kill_after_the_response_and_before_the_receipt_recovers_the_outcome",
        &dir,
        ProbeRungs::ResponseBeforeReceipt,
    )?;

    let mut journal = FileJournal::open(&journal_path)?;
    let this_key = key("1", DIGEST_A)?;
    assert_eq!(
        journal.committed()?.len(),
        RUNG_APPLIED,
        "the outcome is on the disk; the verification is not"
    );
    assert!(
        !journal.torn_tail_repaired(),
        "the kill lost no acknowledged frame, so there is nothing to repair"
    );

    let recovered = journal.recover();
    assert_eq!(
        recovered.status(this_key),
        Some(AttemptStatus::Applied),
        "the effect landed and its outcome was acknowledged; a kill before the \
         verification cannot un-apply it"
    );
    assert!(
        recovered.uncertain().is_empty(),
        "an acknowledged outcome is not an unknown, whatever happened to its \
         verification: {:?}",
        recovered.uncertain()
    );
    assert!(
        !recovered
            .status(this_key)
            .is_some_and(AttemptStatus::is_uncertain),
        "and the status an external system is asked about is the settled one"
    );

    // The duplicate the row forbids is not representable: the ladder for this
    // key has exactly one rung left, and it is the verification.
    match journal.compare_and_append(
        journal.tail(),
        &EffectEvent::OutcomeObserved {
            key: this_key,
            evidence: EffectEvidence::Applied,
        },
    ) {
        Err(JournalError::OutOfOrder {
            expected: Some(lgwks_bot::journal::EventKind::Verified),
            ..
        }) => {}
        Err(other) => return Err(format!("expected an out-of-order refusal, got {other}").into()),
        Ok(_) => return Err("a settled effect accepted a second settlement".into()),
    }

    // The verification is what the restart adds, and adding it changes the status
    // without changing the fact.
    journal.compare_and_append(
        journal.tail(),
        &EffectEvent::Verified {
            key: this_key,
            verification: Verification::new(
                Id128::from_hex(PREDICATE)?,
                1,
                lgwks_std::hash::blake3(b"the postcondition the restart observed"),
                VerificationResult::Satisfied,
            ),
        },
    )?;
    let verified = journal.recover();
    assert_eq!(
        verified.status(this_key),
        Some(AttemptStatus::Verified),
        "the restart's verification is what moves the attempt from settled to verified"
    );
    assert_eq!(
        verified.uncertain().len(),
        0,
        "and verifying never reintroduces uncertainty"
    );
    Ok(())
}

// ── T14 row 3: killed while recovering ─────────────────────────────────────

/// A kill while a restart is reading the journal leaves the file exactly as the
/// process that wrote it left it, and the next restart reads the same history.
///
/// The boundary that is easy to skip and expensive to get wrong: recovery itself
/// is a window in which the crashing process can do damage. A replay that
/// repairs a torn tail is *writing*, and a reader that wrote would turn an
/// interrupted append — which was never anyone's answer — into a committed one.
///
/// Three opens in a row, with a real kill in the middle of the second, is what
/// makes this an observation rather than an assertion about one process: the
/// bytes after the kill are compared against the bytes before it, and the history
/// the third reader folds is the history the first wrote.
#[test]
fn a_kill_during_recovery_leaves_the_journal_exactly_as_it_was() -> TestResult {
    if dispatch_to_boundary_probe()? {
        return Ok(());
    }
    // The first process writes the ladder and dies; its bytes are the subject.
    // A recovery window needs something to recover, so the parent writes the
    // ladder itself and then hands the finished file to a child that only reads.
    let dir = scratch_dir("during-recovery")?;
    let journal_path = dir.join("journal.log");
    {
        let mut journal = FileJournal::open(&journal_path)?;
        walk_ladder(&mut journal, key("1", DIGEST_A)?, RUNG_APPLIED)?;
    }
    // The bytes this process wrote, kept so the assertion after the kill is a
    // comparison against a known subject rather than against whatever is left.
    let after_seed = std::fs::read(&journal_path)?;

    let (journal_path, _guard) = spawn_boundary_probe(
        "a_kill_during_recovery_leaves_the_journal_exactly_as_it_was",
        &dir,
        ProbeRungs::DuringRecovery,
    )?;

    // The reading child appended nothing and repaired nothing: the file holds
    // exactly the events the writer acknowledged, byte for byte.
    let before = std::fs::read(&journal_path)?;
    assert_eq!(
        before, after_seed,
        "a kill while a journal was being read must leave the file exactly as \
         the process that wrote it left it"
    );

    let reader = FileJournal::open(&journal_path)?;
    let history = reader.committed()?;
    assert_eq!(
        history.len(),
        RUNG_APPLIED,
        "a reader that appends nothing sees the ladder it was left"
    );
    assert!(
        !reader.torn_tail_repaired(),
        "a complete file is not torn, so a reader repaired nothing"
    );
    let this_key = key("1", DIGEST_A)?;
    assert_eq!(
        reader.recover().status(this_key),
        Some(AttemptStatus::Applied),
        "and the recovery it folds is the settled answer, not a barrier"
    );
    drop(reader);

    assert_eq!(
        std::fs::read(&journal_path)?,
        before,
        "reading a journal must not move a byte: the file before the recovery \
         and the file after it are the same bytes"
    );

    // The same history again, from a reader that never saw the first: recovery is
    // a fold over committed evidence, so it is repeatable.
    let again = FileJournal::open(&journal_path)?;
    assert_eq!(
        again.committed()?,
        history,
        "a second recovery reads the same history, not a repaired one"
    );
    assert_eq!(
        std::fs::read(&journal_path)?,
        before,
        "and still moves no byte"
    );
    Ok(())
}

// ── A real kill while an append is in flight, on a store that is not answering

/// The child that dies mid-append.
///
/// It opens a journal whose storage device is held closed, hands the owner one
/// append — which cannot complete until the device is released — writes the
/// marker, and waits to be killed. Nothing is on the disk when the kill lands,
/// so the reopen must show an empty, clean journal and the retry must land
/// exactly once: no duplicate, no lost receipt.
fn stalled_probe_body() -> TestResult {
    use std::task::{Context, Waker};

    let path = std::env::var_os(PROBE_JOURNAL)
        .ok_or("the stalled probe was started without a journal path")?;
    let marker = std::env::var_os(PROBE_MARKER)
        .ok_or("the stalled probe was started without a marker path")?;

    let mut journal = FileJournal::open_with_stalled_storage(&path)?;
    let event = EffectEvent::IntentAdmitted {
        key: key("1", DIGEST_A)?,
    };
    let tail = journal.tail();
    let mut append = std::pin::pin!(journal.compare_and_append_async(tail, &event));
    // One poll: the owner is handed the request and parks on the closed device.
    // The append is now genuinely in flight, and no byte has reached the disk.
    // A single poll needs no runtime, so this kill test runs under every feature
    // set rather than only where `rt` is compiled in.
    let mut cx = Context::from_waker(Waker::noop());
    if append.as_mut().poll(&mut cx).is_ready() {
        let refusal = Err("the stalled append completed before the kill".into());
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "stalled_probe_body: returning an error to the caller");
        return refusal;
    }
    std::fs::write(&marker, b"in-flight")?;

    // Park until the parent kills us. Bounded, so a parent that never kills
    // cannot leave a stray process behind.
    for _ in 0..600 {
        pause(100);
    }
    Err("the stalled probe parked for its whole bound and was never killed".into())
}

#[test]
fn a_real_kill_mid_append_leaves_no_duplicate_and_no_lost_receipt() -> TestResult {
    if std::env::var_os(PROBE_STALLED).is_some() {
        return stalled_probe_body();
    }
    let dir = scratch_dir("midappend")?;
    let journal_path = dir.join("journal.log");
    let marker = dir.join("marker");
    let _guard = TempGuard(dir);
    let child = spawn_stalled_probe(
        "a_real_kill_mid_append_leaves_no_duplicate_and_no_lost_receipt",
        &journal_path,
        &marker,
    )?;
    kill_after_marker(child, &marker)?;

    // The append never completed: the owner was parked, so nothing reached the
    // disk. A clean, empty journal is the only honest answer — not a torn tail
    // and not a phantom committed event.
    let mut journal = FileJournal::open(&journal_path)?;
    assert!(
        !journal.torn_tail_repaired(),
        "an append that wrote nothing cannot leave a torn tail"
    );
    assert_eq!(
        journal.committed()?.len(),
        0,
        "an in-flight append killed before its device answered must not commit"
    );
    // T14 row 0: an attempt nobody acknowledged was never promised to anyone,
    // so it recovers as no attempt at all. Not `Prepared`, which would claim
    // the intent was durable, and not an unknown barrier, which would demand a
    // reconciliation for an effect that cannot have left the process. Row 1,
    // killed one acknowledgment later, recovers `Prepared` for the same key.
    let this_key = key("1", DIGEST_A)?;
    let recovered = journal.recover();
    assert_eq!(
        recovered.status(this_key),
        None,
        "an unacknowledged admission recovers as no attempt, never as Prepared"
    );
    assert!(
        recovered.uncertain().is_empty(),
        "nothing was dispatched, so nothing is unknown and nothing is resent: {:?}",
        recovered.uncertain()
    );

    // The retry lands exactly once: there was no acknowledgment to lose, and
    // the fact on the disk is the first and only one for the attempt.
    let ack = journal.compare_and_append(
        journal.tail(),
        &EffectEvent::IntentAdmitted { key: this_key },
    )?;
    assert_eq!(
        ack.promise(),
        DurabilityPromise::ProcessCrash,
        "the retry earns the same durable promise the killed append would have"
    );
    drop(journal);
    let reopened = FileJournal::open(&journal_path)?;
    assert_eq!(
        reopened.committed()?.len(),
        1,
        "the retry must land exactly once, never a duplicate"
    );
    assert_eq!(
        reopened.recover().status(this_key),
        Some(AttemptStatus::Prepared),
        "the retry's own admission is the recovered answer"
    );
    Ok(())
}
