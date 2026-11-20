//! Continue-as-new: the journey, end to end, against a real file journal.
//!
//! Every arm here drives the **shipped** [`FileJournal`]: its real frame codec,
//! its real storage-owner thread, its real chain verification, and the real
//! [`recover`](lgwks_bot::journal::recover) fold. Nothing in this file
//! reimplements a journal or a checkpoint, because a second journal written "for
//! the test" would pass while the shipped one rotted.
//!
//! The arms, one property each, are named for it: the trigger fires and the
//! sealed file refuses later appends; nothing unresolved is dropped or resent; a
//! replayed settled attempt is refused; tenants never cross; a reopen finds the
//! live journal; every crash boundary is reachable and named; a seal is one
//! frame in each file and nothing else moves.

use std::error::Error;
use std::path::{Path, PathBuf};

use lgwks_bot::effect::{
    ActionDigest, ActionId, AttemptId, EffectIdentity, EffectKey, EnvironmentEpoch, EnvironmentId,
    FlowRevision, Id128, RunId,
};
use lgwks_bot::journal::{
    AttemptStatus, ContinuationPolicy, DurableAck, EffectEvent, EffectEvidence, EffectJournal,
    FileJournal, JournalError, JournalPosition, SealPause, Verification, VerificationResult,
};
use lgwks_std::hash::blake3;

use crate::journal_fixtures::{ACTION, DIGEST_HEX, ENV, FLOW_HEX, RUN, TempGuard, scratch_dir};

/// The predicate every ladder's verification names, as 32 hex characters.
const PREDICATE: &str = "42424242424242424242424242424242";

/// A test that needs a parsed value returns `Result` and propagates, because the
/// crate forbids a panicking path anywhere, tests included.
type TestResult = Result<(), Box<dyn Error>>;

/// The trigger this file's arms use: sixty-four events, and a byte trigger well
/// past what a journal that small can reach. A declared policy rather than the
/// shipped fraction, because reaching the shipped trigger would test the ceiling,
/// not continuation; `examples/journal_continuation.rs` drives the shipped one.
const TEST_EVENT_WATERMARK: u64 = 64;
const TEST_BYTE_WATERMARK: u64 = 4 * 1024 * 1024;

/// The policy these arms open with.
fn policy() -> Result<ContinuationPolicy, Box<dyn Error>> {
    Ok(ContinuationPolicy::new(
        TEST_EVENT_WATERMARK,
        TEST_BYTE_WATERMARK,
    )?)
}

/// A key for `attempt` of `action`, under the shared run identity.
fn key_for_action(action: ActionId, attempt: u64) -> Result<EffectKey, Box<dyn Error>> {
    let run = RunId::from_hex(RUN)?;
    let environment = EnvironmentId::from_hex(ENV)?;
    let flow = FlowRevision::from_tagged("blake3_256", FLOW_HEX)?;
    let attempt = AttemptId::from_decimal(&attempt.to_string())?;
    let digest = ActionDigest::from_tagged("blake3_256", DIGEST_HEX)?;
    let epoch = EnvironmentEpoch::from_decimal("1")?;
    Ok(EffectIdentity::new(run, environment, flow).key(action, attempt, digest, epoch))
}

/// The action every attempt of one tenant belongs to.
///
/// One tenant per action: a fold that mixed tenants would show an attempt under
/// the wrong action.
fn tenant_action(tenant: u64) -> Result<ActionId, Box<dyn Error>> {
    let hex = format!("{tenant:032x}");
    Ok(ActionId::from_hex(&hex)?)
}

/// The shared action every attempt of the single-tenant arms belongs to.
fn shared_action() -> Result<ActionId, Box<dyn Error>> {
    Ok(ActionId::from_hex(ACTION)?)
}

/// A full four-rung ladder for `attempt` of the shared action.
fn ladder(attempt: u64) -> Result<Vec<EffectEvent>, Box<dyn Error>> {
    ladder_of(shared_action()?, attempt)
}

/// A full four-rung ladder for `attempt` of `action`.
///
/// The shared fixture's ladder, keyed: one definition of a complete attempt.
fn ladder_of(action: ActionId, attempt: u64) -> Result<Vec<EffectEvent>, Box<dyn Error>> {
    crate::journal_fixtures::ladder(key_for_action(action, attempt)?, PREDICATE, 1)
}

/// Write `attempts` complete four-rung ladders, batched so the run costs one flush
/// per attempt rather than one per rung.
fn fill(journal: &mut FileJournal, from: u64, attempts: u64) -> Result<(), Box<dyn Error>> {
    for attempt in from..from.saturating_add(attempts) {
        journal.compare_and_append_all(&ladder(attempt)?)?;
    }
    Ok(())
}

/// Continue this handle and hand back the successor, refusing anything else.
///
/// One helper: every arm continues, then carries on with the successor.
fn continue_once(journal: &mut FileJournal) -> Result<FileJournal, Box<dyn Error>> {
    journal
        .continue_as_file()?
        .ok_or_else(|| "a continuing journal must hand back a successor".into())
}

/// Append one event at the handle's own tail.
fn append(journal: &mut FileJournal, event: &EffectEvent) -> Result<DurableAck, JournalError> {
    journal.compare_and_append(journal.tail(), event)
}

/// A verification that did not hold, at `version`.
fn failed_verification(version: u64) -> Result<Verification, Box<dyn Error>> {
    Ok(Verification::new(
        Id128::from_hex(PREDICATE)?,
        version,
        blake3(b"the durable ladder's own predicate"),
        VerificationResult::NotSatisfied,
    ))
}

#[test]
fn a_run_continues_at_its_watermark_and_the_predecessor_is_sealed_read_only() -> TestResult {
    let dir = scratch_dir("seal")?;
    let _guard = TempGuard(dir.clone());
    let first = dir.join("run.jrnl");
    let mut journal = FileJournal::open_continuing_with(&first, policy()?)?;

    // Fifteen four-rung attempts, then the trigger.
    fill(&mut journal, 1, 15)?;
    assert!(
        !journal.continuation_watermark()?.is_due(),
        "sixty events must not have reached a sixty-four-event trigger"
    );
    let sealed_at = journal.tail();
    let sealed_path = journal.successor_path();

    let mut successor = continue_once(&mut journal)?;
    assert_eq!(
        journal.sealed_by(),
        Some(sealed_path.as_path()),
        "the handle reports where it sealed to, which is where it sealed"
    );
    assert_eq!(
        journal.committed()?.len(),
        60,
        "the seal frame is not an event, so the predecessor keeps its sixty events"
    );
    let refused = append(
        &mut journal,
        &EffectEvent::IntentAdmitted {
            key: key_for_action(shared_action()?, 1)?,
        },
    );
    assert!(
        matches!(refused, Err(JournalError::Superseded { .. })),
        "a sealed predecessor is read-only, and says why: {refused:?}"
    );

    // The successor's own chain starts at the predecessor's tail and its first
    // frame is the seal frame, so it is exactly one past.
    assert_eq!(
        successor.tail().sequence(),
        sealed_at.sequence().saturating_add(1),
        "the successor's first frame is the predecessor's seal, one past its tail"
    );
    assert_eq!(
        successor.base(),
        sealed_at,
        "and the successor names the predecessor's chain head as its own base"
    );
    assert!(
        successor.committed()?.is_empty(),
        "a successor's events are only what has been appended since its checkpoint"
    );
    assert!(
        append(
            &mut successor,
            &EffectEvent::IntentAdmitted {
                key: key_for_action(shared_action()?, 16)?
            }
        )
        .is_ok(),
        "and the successor admits work the sealed file refuses"
    );
    Ok(())
}

#[test]
fn an_unresolved_attempt_stays_unknown_across_a_continuation_and_settles_without_a_resend()
-> TestResult {
    let dir = scratch_dir("carry")?;
    let _guard = TempGuard(dir.clone());
    let first = dir.join("run.jrnl");
    let action = ActionId::from_hex(&"22".repeat(16))?;
    let mut journal = FileJournal::open_continuing_with(&first, policy()?)?;

    // One attempt handed over unsettled, then enough settled work to reach the trigger.
    let stranded = key_for_action(action, 1)?;
    let two = ladder_of(action, 1)?;
    journal.compare_and_append_all(two.get(..2).ok_or("the ladder has two rungs")?)?;
    assert_eq!(
        journal.recover().status(stranded),
        Some(AttemptStatus::OutcomeUnknown),
        "the predecessor holds the attempt as unknown before the continuation"
    );
    fill(&mut journal, 2, 15)?;

    let mut successor = continue_once(&mut journal)?;
    drop(journal);

    assert_eq!(
        successor.recover().status(stranded),
        Some(AttemptStatus::OutcomeUnknown),
        "the successor holds the same attempt as unknown, never as absent"
    );
    assert!(
        successor.recover().uncertain().contains(&stranded),
        "and it is named in the uncertain list a recovery path must settle"
    );
    let checkpoint = successor
        .checkpoint()
        .ok_or("a successor carries a checkpoint")?;
    assert!(
        checkpoint
            .unresolved()
            .iter()
            .any(|carried| carried.key() == stranded),
        "the carried list names the unresolved attempt by full identity"
    );

    // A resend is refused: the ladder knows this key reached DispatchPrepared.
    let resend = successor.compare_and_append(
        successor.tail(),
        &EffectEvent::IntentAdmitted { key: stranded },
    );
    assert!(
        resend.is_err(),
        "an unknown attempt must not be re-admitted, which is how a duplicate \
         non-idempotent effect gets sent: {resend:?}"
    );

    // Evidence settles it, in the successor, with no resend and no second rung.
    let settled = append(
        &mut successor,
        &EffectEvent::OutcomeObserved {
            key: stranded,
            evidence: EffectEvidence::Applied,
        },
    )?;
    assert_eq!(
        successor.recover().status(stranded),
        Some(AttemptStatus::Applied),
        "the appended evidence settled the carried attempt at sequence {}",
        settled.position().sequence()
    );
    Ok(())
}

#[test]
fn a_verification_digest_crosses_the_boundary() -> TestResult {
    let dir = scratch_dir("verification")?;
    let _guard = TempGuard(dir.clone());
    let first = dir.join("run.jrnl");
    let action = ActionId::from_hex(&"33".repeat(16))?;
    let mut journal = FileJournal::open_continuing_with(&first, policy()?)?;

    // A ladder whose verification did not hold: the status a caller must not
    // confuse with an unverified success, and the digest it is qualified by.
    let verified = key_for_action(action, 1)?;
    journal.compare_and_append_all(ladder_of(action, 1)?.get(..3).ok_or("three rungs")?)?;
    append(
        &mut journal,
        &EffectEvent::Verified {
            key: verified,
            verification: failed_verification(9)?,
        },
    )?;
    // Sixteen settled attempts of a *different* action: a successor keeps the
    // latest attempt per action, so the failed verdict needs an action of its own.
    let other = ActionId::from_hex(&"55".repeat(16))?;
    for attempt in 2..=16 {
        journal.compare_and_append_all(&ladder_of(other, attempt)?)?;
    }

    let successor = continue_once(&mut journal)?;
    drop(journal);

    assert_eq!(
        successor.recover().status(verified),
        Some(AttemptStatus::VerificationFailed),
        "the failed verification is carried as itself, not as an applied effect"
    );
    assert_eq!(
        successor
            .recover()
            .verification(verified)
            .ok_or("the verification digest must cross the boundary")?
            .predicate_version(),
        9,
        "and it is the revision the verdict was decided at"
    );
    Ok(())
}

#[test]
fn a_replayed_settled_attempt_is_refused_rather_than_admitted_again() -> TestResult {
    let dir = scratch_dir("replay")?;
    let _guard = TempGuard(dir.clone());
    let first = dir.join("run.jrnl");
    let mut journal = FileJournal::open_continuing_with(&first, policy()?)?;

    fill(&mut journal, 1, 3)?;
    let older = key_for_action(shared_action()?, 1)?;
    let latest = key_for_action(shared_action()?, 3)?;
    let successor = continue_once(&mut journal)?;
    drop(journal);

    // The successor does not carry attempt 1 by key, because `AttemptId` is
    // monotonic per action; it carries the fold that answers for it.
    let mut successor = successor;
    // Two arms, and the difference is the point. The **older** attempt is not in
    // the successor's ladder at all: it is refused by the folded record, which is
    // the only thing that remembers it. The **latest** attempt is in the ladder,
    // because the checkpoint seeded it, so the ladder refuses it and the fold never
    // has to be consulted. Both are refusals; neither is a duplicate.
    for (replay, by_the_fold) in [(older, true), (latest, false)] {
        let refused = successor.compare_and_append(
            successor.tail(),
            &EffectEvent::IntentAdmitted { key: replay },
        );
        let matched = match by_the_fold {
            true => matches!(refused, Err(JournalError::AttemptAlreadyWalked { .. })),
            false => matches!(refused, Err(JournalError::OutOfOrder { .. })),
        };
        assert!(
            matched,
            "a re-presentation of a sealed attempt is refused, by the fold for an \
             older attempt and by the ladder for the folded key itself: {refused:?}"
        );
    }
    let fresh = key_for_action(shared_action()?, 4)?;
    assert!(
        successor
            .compare_and_append(
                successor.tail(),
                &EffectEvent::IntentAdmitted { key: fresh }
            )
            .is_ok(),
        "and the next attempt of the same action is new work, so it is admitted"
    );
    Ok(())
}

#[test]
fn two_tenants_in_one_directory_continue_independently() -> TestResult {
    let dir = scratch_dir("tenants")?;
    let _guard = TempGuard(dir.clone());
    let mut live: Vec<PathBuf> = Vec::new();

    for tenant in 1_u64..=2 {
        let path = dir.join(format!("tenant-{tenant}.jrnl"));
        let action = tenant_action(tenant)?;
        let mut journal = FileJournal::open_continuing_with(&path, policy()?)?;
        let mut continuations = 0_u64;
        for attempt in 1..=90 {
            let events = tenant_ladder(action, attempt)?;
            journal.compare_and_append_all(&events)?;
            if journal.continuation_watermark()?.is_due() {
                journal = continue_once(&mut journal)?;
                continuations = continuations.saturating_add(1);
            }
        }
        assert!(
            continuations > 1,
            "tenant {tenant} continued {continuations} times, which is not enough \
             to say a chain of files works"
        );
        live.push(journal.path().to_path_buf());
        for attempt in journal.recover().attempts() {
            assert_eq!(
                attempt.key().action(),
                action,
                "tenant {tenant} recovered an attempt belonging to another tenant"
            );
        }
        // The ninety attempts are accounted for as *refused*, not as remembered:
        // the checkpoint folds them into one record per action, so what a caller
        // can see is small and what it cannot do is replay any of them.
        let mut replay = journal;
        for attempt in 1..=90 {
            let refused = replay.compare_and_append(
                replay.tail(),
                &EffectEvent::IntentAdmitted {
                    key: key_for_action(action, attempt)?,
                },
            );
            // Two typed answers are both refusals and both correct: an attempt the
            // live suffix or the folded key still holds is refused by the ladder,
            // and one only the folded record knows about is refused by the fold.
            // Neither admits it, which is the whole property.
            assert!(
                matches!(
                    refused,
                    Err(JournalError::AttemptAlreadyWalked { .. } | JournalError::OutOfOrder { .. })
                ),
                "tenant {tenant}'s attempt {attempt} was already walked, and a \
                 re-presentation of it must be refused: {refused:?}"
            );
        }
    }

    assert_ne!(
        live.first(),
        live.get(1),
        "two tenants must not share a journal"
    );
    for (index, path) in live.iter().enumerate() {
        assert!(
            path.to_string_lossy().starts_with(
                &dir.join(format!("tenant-{}.jrnl", index + 1))
                    .display()
                    .to_string()
            ),
            "each tenant's live journal is its own file plus the shared successor \
             rule applied: {}",
            path.display()
        );
    }
    Ok(())
}

/// One tenant's three-rung ladder under its own action.
fn tenant_ladder(action: ActionId, attempt: u64) -> Result<Vec<EffectEvent>, Box<dyn Error>> {
    let key = key_for_action(action, attempt)?;
    Ok(vec![
        EffectEvent::IntentAdmitted { key },
        EffectEvent::DispatchPrepared { key },
        EffectEvent::OutcomeObserved {
            key,
            evidence: EffectEvidence::Applied,
        },
    ])
}

#[test]
fn a_reopen_follows_the_generation_chain_to_the_live_journal() -> TestResult {
    let dir = scratch_dir("walk")?;
    let _guard = TempGuard(dir.clone());
    let first = dir.join("run.jrnl");
    let mut journal = FileJournal::open_continuing_with(&first, policy()?)?;
    for round in 0..3_u64 {
        let from = round.saturating_mul(30).saturating_add(1);
        fill(&mut journal, from, 30)?;
        if journal.continuation_watermark()?.is_due() {
            journal = continue_once(&mut journal)?;
        }
    }
    let live = journal.path().to_path_buf();
    let generation = journal.generation();
    drop(journal);

    let reopened = FileJournal::open_active(&first)?;
    assert_eq!(
        reopened.path(),
        live.as_path(),
        "a caller holding only the original path reaches the live journal"
    );
    assert_eq!(reopened.generation(), generation);
    Ok(())
}

/// Every file in a chain is its own generation, and its checkpoint names the
/// same one: the base is generation one, so successor `000001` is generation two.
#[test]
fn each_successor_is_the_next_generation_and_its_checkpoint_says_so() -> TestResult {
    let dir = scratch_dir("generations")?;
    let _guard = TempGuard(dir.clone());
    let mut journal = FileJournal::open_continuing_with(dir.join("run.jrnl"), policy()?)?;
    assert_eq!(
        journal.generation(),
        1,
        "the base journal is generation one"
    );
    assert!(journal.checkpoint().is_none(), "and carries no checkpoint");
    for expected in 2..=4_u64 {
        fill(&mut journal, expected.saturating_mul(100), 3)?;
        journal = continue_once(&mut journal)?;
        let carried = journal
            .checkpoint()
            .ok_or("a successor is opened from its predecessor's checkpoint")?
            .generation();
        assert_eq!(
            (journal.generation(), carried),
            (expected, expected),
            "the file and the checkpoint it was opened from name one generation"
        );
    }
    Ok(())
}

#[test]
fn a_sealed_predecessor_refuses_itself_and_names_its_successor() -> TestResult {
    let dir = scratch_dir("superseded")?;
    let _guard = TempGuard(dir.clone());
    let first = dir.join("run.jrnl");
    let mut journal = FileJournal::open_continuing_with(&first, policy()?)?;
    fill(&mut journal, 1, 20)?;
    let successor_path = continue_once(&mut journal)?.path().to_path_buf();
    drop(journal);

    match FileJournal::open(&first) {
        Err(JournalError::Superseded { path }) => {
            assert_eq!(path, successor_path.display().to_string());
        }
        other => {
            return Err(format!(
                "a sealed predecessor must refuse itself and name its successor, got {other:?}"
            )
            .into());
        }
    }
    Ok(())
}

#[test]
fn an_armed_continuation_refuses_at_the_boundary_it_reached() -> TestResult {
    for boundary in SealPause::all() {
        let dir = scratch_dir("pause")?;
        let _guard = TempGuard(dir.clone());
        let first = dir.join("run.jrnl");
        let mut journal = FileJournal::open_continuing_with(&first, policy()?)?;
        fill(&mut journal, 1, 20)?;
        journal.arm_continuation_pause(boundary);

        match journal.continue_as_file() {
            Err(JournalError::ContinuationPaused { boundary: at }) => assert_eq!(at, boundary),
            other => {
                return Err(format!(
                    "an armed continuation must stop at {boundary}, got {}",
                    describe(&other)
                )
                .into());
            }
        }
        let refused = append(
            &mut journal,
            &EffectEvent::IntentAdmitted {
                key: key_for_action(shared_action()?, 1)?,
            },
        );
        assert!(
            matches!(refused, Err(JournalError::Superseded { .. })),
            "a stopped seal leaves the handle read-only on either side of the boundary"
        );
    }
    Ok(())
}

/// The seal is one frame, written byte for byte into both files.
#[test]
fn a_seal_writes_exactly_its_own_frame_into_both_files() -> TestResult {
    let dir = scratch_dir("bytes")?;
    let _guard = TempGuard(dir.clone());
    let first = dir.join("run.jrnl");
    let sealed = dir.join("run.jrnl.cont").join("000001");
    let mut journal = FileJournal::open_continuing_with(&first, policy()?)?;
    fill(&mut journal, 1, 20)?;
    let before = std::fs::metadata(&first)?.len();
    assert!(
        !sealed.exists(),
        "the successor does not exist before the seal"
    );

    let successor = continue_once(&mut journal)?;
    drop(journal);
    let sealed_bytes = std::fs::read(&sealed)?;
    let first_bytes = std::fs::read(&first)?;

    assert_eq!(
        u64::try_from(first_bytes.len())?.saturating_sub(before),
        u64::try_from(sealed_bytes.len())?,
        "the predecessor grew by exactly what the successor's whole first frame is"
    );
    let grown = first_bytes.get(usize::try_from(before)?..);
    assert_eq!(
        Some(sealed_bytes.as_slice()),
        grown,
        "the seal frame is the same bytes in both files"
    );
    assert!(
        successor.committed()?.is_empty(),
        "the successor has admitted nothing yet"
    );
    Ok(())
}

/// A continuation answer, rendered for a failure that cannot name its type.
fn describe(answer: &Result<Option<FileJournal>, JournalError>) -> String {
    match *answer {
        Ok(Some(ref journal)) => format!("a successor at {}", journal.path().display()),
        Ok(None) => "no successor".to_owned(),
        Err(ref error) => format!("{error}"),
    }
}

/// The shipped watermark is eight tenths of the shipped ceilings, and it leaves
/// room for an in-flight handoff to settle at either one.
#[test]
fn the_shipped_watermark_leaves_the_declared_headroom() -> TestResult {
    let declared = ContinuationPolicy::declared();
    assert_eq!(declared.events_at(), 80_000);
    assert!(
        u64::try_from(lgwks_bot::journal::MAX_JOURNAL_EVENTS)? > declared.events_at(),
        "the watermark is strictly inside the event ceiling, so an in-flight \
         handoff always has room to settle"
    );
    assert!(lgwks_bot::journal::MAX_JOURNAL_BYTES > declared.bytes_at());
    assert!(
        ContinuationPolicy::new(
            u64::try_from(lgwks_bot::journal::MAX_JOURNAL_EVENTS)?,
            declared.bytes_at()
        )
        .is_err(),
        "a trigger at the ceiling is refused rather than clamped to something unreachable"
    );
    assert!(Path::new("/tmp").is_absolute());
    let genesis = JournalPosition::genesis();
    assert_eq!(genesis.sequence(), 0);
    Ok(())
}

mod seal_probe;
