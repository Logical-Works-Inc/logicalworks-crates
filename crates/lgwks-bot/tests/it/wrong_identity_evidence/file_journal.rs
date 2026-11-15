//! The byte-identical half, on a real file.

/// The scratch and cleanup fixtures, shared with the journal families.
///
/// One home for "a path no other run will use" and "remove it when the test
/// ends", so this file cannot assert a different cleanup discipline than the
/// families that also observe real kills.
use crate::journal_fixtures as shared;

use shared::{TempGuard, scratch_dir};

use lgwks_bot::effect::EffectKey;
use lgwks_bot::journal::{EffectEvent, EffectJournal, EventKind, FileJournal, JournalError};

use super::{TestResult, WrongField};

/// The predicate this file's fixture verifications are written under.
const PREDICATE: &str = "4142434445464748494a4b4c4d4e4f50";
/// The whole ladder, which the file-journal row walks end to end.
const WHOLE_LADDER: usize = 4;

/// A refused settlement moves no byte of a real journal.
///
/// The other half of "without mutation". An in-memory `Vec` cannot show that a
/// refusal left a *device* alone, and the claim that matters is about the file a
/// crash would later replay: a refusal that wrote a partial frame, repaired a
/// torn tail, or moved the tail would change what a restart recovers even though
/// it "refused".
///
/// What refuses, and what does not, is worth stating because a test written
/// against the wrong expectation would be asserting the ladder does not do what
/// it does. The ordering ladder is **per key**: a key that differs in one
/// identity field is not on the ladder of the key the journal holds — it is at
/// the foot of its own, and admitting it is correct, because a distinct identity
/// is distinct work. What the journal refuses is a rung that cannot follow what
/// is committed *for that key*: a repeat, and anything at all on a completed
/// ladder. Both are asserted here, and both must leave the file byte-identical.
///
/// The ladder is not the settlement path the ledger rows above exercise; it is
/// the device's own ordering rule, and this row is about the device.
#[test]
fn a_refused_settlement_leaves_a_real_file_journal_byte_identical() -> TestResult {
    let dir = scratch_dir("t12-file")?;
    let _guard = TempGuard(dir.clone());
    let path = dir.join("journal.log");
    let mut journal = FileJournal::open(&path)?;
    let key = recorded_key()?;

    // A whole attempt on the real file, so the journal holds four committed
    // frames and the wrong-field deliveries have a completed ladder to sit
    // beside. The ladder itself is the shared support module's — the journal's
    // ordering rule, spelled once for every harness that walks it.
    shared::walk_ladder(&mut journal, key, PREDICATE, 1, WHOLE_LADDER)?;

    let completed_events = journal.committed()?;
    let completed_bytes = std::fs::read(&path)?;
    assert_eq!(
        completed_events.len(),
        WHOLE_LADDER,
        "the attempt this journal holds is complete, one rung at a time"
    );

    // Every refusal the ladder can produce for this key, and nothing written.
    // `Verified` is the one rung that is not refused after the ladder completes:
    // a later verification supersedes the verdict by design (#259), and the tests
    // in `durable_crash_observation` cover that acceptance, so it is not a
    // refusal to assert here.
    for event in shared::ladder(key, PREDICATE, 1)? {
        if event.kind() == EventKind::Verified {
            continue;
        }
        let tail = journal.tail();
        match journal.compare_and_append(tail, &event) {
            Err(JournalError::OutOfOrder { .. }) => {}
            other => {
                return Err(
                    format!("the completed ladder accepted another rung: {other:?}").into(),
                );
            }
        }
        assert_eq!(
            std::fs::read(&path)?,
            completed_bytes,
            "the refused append moved bytes on the device"
        );
    }

    // A key that differs in one identity field is at the foot of its own ladder,
    // and admitting it is *correct* — a distinct identity is distinct work. The
    // control: a test that asserted every delivery was refused would pass against
    // an implementation that refused to record work it had never seen.
    let mut admitted = 0usize;
    for wrong in WrongField::ALL {
        let submitted = wrong.key(key)?;
        assert_ne!(
            submitted,
            key,
            "{}: the wrong-field key must differ from the one the journal holds",
            wrong.tag()
        );
        let tail = journal.tail();
        journal.compare_and_append(tail, &EffectEvent::IntentAdmitted { key: submitted })?;
        admitted = admitted.saturating_add(1);
    }
    let after_admissions = std::fs::read(&path)?;
    assert_eq!(
        journal.committed()?.len(),
        completed_events.len().saturating_add(admitted),
        "each distinct identity was admitted at the foot of its own ladder"
    );

    // And now the second rung for each of *those*, which the ladder does refuse,
    // because the first already committed under the same key.
    for wrong in WrongField::ALL {
        let submitted = wrong.key(key)?;
        let tail = journal.tail();
        match journal.compare_and_append(tail, &EffectEvent::IntentAdmitted { key: submitted }) {
            Err(JournalError::OutOfOrder { .. }) => {}
            other => {
                return Err(format!(
                    "{}: a repeated admission for a key already holding one was \
                     accepted: {other:?}",
                    wrong.tag()
                )
                .into());
            }
        }
    }

    assert_eq!(
        std::fs::read(&path)?,
        after_admissions,
        "the repeated admissions moved bytes on the device"
    );
    assert!(
        after_admissions.starts_with(&completed_bytes),
        "and the completed attempt's bytes are still a literal prefix: a refusal \
         may only extend the file, never rewrite it"
    );

    // The chain still verifies, which is the byte-level claim's consequence: a
    // refusal that had corrupted a frame would fail here rather than passing an
    // equality check on a file nobody can reopen.
    assert!(
        lgwks_bot::journal::verify_chain(&journal.committed_entries()?).is_ok(),
        "every committed frame still follows from the ones before it"
    );

    drop(journal);
    Ok(())
}

/// The key the file row walks, built from the shared rig's own identity.
///
/// One constructor so the file row and the ledger rows are about the same action
/// family rather than about two actions that merely look similar, and so the
/// wrong-field rows and the ladder rows rewrite fields of one key rather than
/// two.
fn recorded_key() -> Result<EffectKey, Box<dyn std::error::Error>> {
    crate::sim::rig::attempt_key(2)
}
