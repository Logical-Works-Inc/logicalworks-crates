//! A consumer's crash-and-restore scenarios, ported onto the record journal.
//!
//! Each runs the same way the consumer's own does: record a run's facts, crash by
//! dropping the handle with nothing flushed by hand, restore by folding the file
//! back, and require the same trace from two identical runs of each seed. The fold
//! here stands in for the consumer's projection, because what these prove is what
//! the journal owes it: the same facts, in the same order, after any kill.

use std::collections::BTreeMap;
use std::path::Path;

use lgwks_std::hash::Digest;

use super::super::JournalError;
use super::RecordJournal;
use super::sim_records::{Fact, Outcome, Scratch, fact, held, replay};

/// A run was asked for; the note is its command identity.
const SUBMIT: u8 = 1;
/// An effect left, holding budget until it settles.
const DISPATCH: u8 = 2;
/// The effect's outcome was lost.
const UNKNOWN: u8 = 3;
/// The effect succeeded, and this is its receipt.
const SUCCESS: u8 = 4;
/// Policy refused the run.
const DENIED: u8 = 5;
/// The run was cancelled.
const CANCELLED: u8 = 6;

/// Where a run stands in the fold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// Admitted and waiting.
    Pending,
    /// Its effect left and nothing has come back.
    Dispatched,
    /// Its outcome is unknown and has to be reconciled.
    Reconciling,
    /// It finished with a receipt.
    Completed,
    /// Refused.
    Failed,
    /// Cancelled.
    Cancelled,
}

/// What a consumer folds from its journal.
#[derive(Debug, Default, PartialEq, Eq)]
struct Ledger {
    /// Each run's standing.
    runs: BTreeMap<u64, Standing>,
    /// Receipts in the order they landed.
    receipts: Vec<u64>,
    /// Which run and payload each command identity first named.
    commands: BTreeMap<Vec<u8>, u64>,
    /// Budget held by effects that have not settled.
    reserved: u64,
}

impl Ledger {
    /// Fold every record of `journal` in commit order.
    fn of(journal: &RecordJournal<Fact>) -> Outcome<Self> {
        let mut ledger = Self::default();
        for record in held(journal)? {
            ledger.apply(&record);
        }
        Ok(ledger)
    }

    /// Fold one record.
    fn apply(&mut self, record: &Fact) {
        let standing = match record.kind {
            SUBMIT => {
                self.commands
                    .entry(record.note.clone())
                    .or_insert(record.run);
                Standing::Pending
            }
            DISPATCH => {
                self.reserved = self.reserved.saturating_add(1);
                Standing::Dispatched
            }
            UNKNOWN => Standing::Reconciling,
            SUCCESS => {
                self.reserved = self.reserved.saturating_sub(1);
                self.receipts.push(record.run);
                Standing::Completed
            }
            DENIED => Standing::Failed,
            _ => Standing::Cancelled,
        };
        self.runs.insert(record.run, standing);
    }

    /// The standing of `run`.
    fn standing(&self, run: u64) -> Option<Standing> {
        self.runs.get(&run).copied()
    }
}

/// Append each record at the journal's own tail.
fn record(journal: &mut RecordJournal<Fact>, facts: &[Fact]) -> Outcome {
    for held_fact in facts {
        journal.compare_and_append(journal.tail(), held_fact)?;
    }
    Ok(())
}

/// A journal holding `facts`, crashed by dropping it, and its restored handle.
fn crashed(path: &Path, facts: &[Fact]) -> Outcome<RecordJournal<Fact>> {
    let mut journal = RecordJournal::<Fact>::open(path)?;
    record(&mut journal, facts)?;
    drop(journal);
    Ok(RecordJournal::<Fact>::open(path)?)
}

/// A run through to its unknown outcome.
fn lost(run: u64) -> Vec<Fact> {
    let note = run.to_be_bytes();
    vec![
        fact(run, SUBMIT, &note),
        fact(run, DISPATCH, b""),
        fact(run, UNKNOWN, b""),
    ]
}

/// A run through to its receipt.
fn finished(run: u64) -> Vec<Fact> {
    let note = run.to_be_bytes();
    vec![
        fact(run, SUBMIT, &note),
        fact(run, DISPATCH, b""),
        fact(run, SUCCESS, b""),
    ]
}

/// A restored run reads as reconciling until its settlement lands, then as completed.
fn settles(restored: &mut RecordJournal<Fact>, seed: u64) -> Outcome<Digest> {
    assert_eq!(
        Ledger::of(restored)?.standing(seed),
        Some(Standing::Reconciling)
    );
    record(restored, &[fact(seed, SUCCESS, b"")])?;
    assert_eq!(
        Ledger::of(restored)?.standing(seed),
        Some(Standing::Completed)
    );
    Ok(restored.tail().head())
}

/// One scenario over `seeds`, each in a directory of its own, returning its trace.
fn scenario(
    name: &str,
    seeds: core::ops::Range<u64>,
    body: impl Fn(&Path, u64) -> Outcome<Digest>,
) -> Outcome {
    replay(seeds, |seed| {
        let scratch = Scratch::new(name)?;
        body(&scratch.file("journal"), seed)
    })
}

/// An unknown outcome at crash time reconciles after restore.
#[test]
fn sim_records_crash_unknown_reconciles_after_restore() -> Outcome {
    scenario("unknown", 0..8, |path, seed| {
        let mut restored = crashed(path, &lost(seed))?;
        settles(&mut restored, seed)
    })
}

/// A published receipt survives the crash without a second one.
#[test]
fn sim_records_crash_success_receipt_not_duplicated() -> Outcome {
    scenario("receipt", 8..16, |path, seed| {
        let restored = crashed(path, &finished(seed))?;
        assert_eq!(Ledger::of(&restored)?.receipts, vec![seed]);
        assert_eq!(restored.committed(), 3, "restore records nothing");
        Ok(restored.tail().head())
    })
}

/// Crashing before any record restores an empty, idle journal.
#[test]
fn sim_records_crash_empty_journal_restores_idle() -> Outcome {
    scenario("empty", 16..24, |path, _| {
        let restored = crashed(path, &[])?;
        assert_eq!(restored.committed(), 0);
        assert!(!restored.torn_tail_repaired());
        assert_eq!(Ledger::of(&restored)?, Ledger::default());
        Ok(restored.tail().head())
    })
}

/// Restoring the same file twice folds the same ledger.
#[test]
fn sim_records_crash_same_journal_restores_twice_identically() -> Outcome {
    scenario("twice", 24..32, |path, seed| {
        let first = crashed(path, &finished(seed))?;
        let (ledger, tail) = (Ledger::of(&first)?, first.tail());
        drop(first);
        let second = RecordJournal::<Fact>::open(path)?;
        assert_eq!(Ledger::of(&second)?, ledger);
        assert_eq!(second.tail(), tail);
        Ok(tail.head())
    })
}

/// A new run after restore completes and takes its budget.
#[test]
fn sim_records_crash_new_run_after_restore_completes() -> Outcome {
    scenario("new-run", 32..40, |path, seed| {
        let mut restored = crashed(path, &finished(seed))?;
        record(&mut restored, &finished(seed.saturating_add(1)))?;
        let ledger = Ledger::of(&restored)?;
        assert_eq!(ledger.receipts, vec![seed, seed.saturating_add(1)]);
        assert_eq!(ledger.reserved, 0);
        Ok(restored.tail().head())
    })
}

/// A settlement the crash tore is not a settlement: the run reconciles again.
#[test]
fn sim_records_crash_mid_reconcile_finishes() -> Outcome {
    scenario("mid", 40..48, |path, seed| {
        let mut journal = crashed(path, &lost(seed))?;
        record(&mut journal, &[fact(seed, SUCCESS, b"")])?;
        drop(journal);
        let mut bytes = std::fs::read(path)?;
        bytes.truncate(bytes.len().saturating_sub(1));
        std::fs::write(path, &bytes)?;
        let mut restored = RecordJournal::<Fact>::open(path)?;
        assert!(restored.torn_tail_repaired());
        settles(&mut restored, seed)
    })
}

/// A dispatch still open at the crash keeps holding its budget.
#[test]
fn sim_records_crash_open_reservation_holds_budget() -> Outcome {
    scenario("reserve", 48..56, |path, seed| {
        let restored = crashed(path, &lost(seed))?;
        assert_eq!(Ledger::of(&restored)?.reserved, 1);
        Ok(restored.tail().head())
    })
}

/// A refused run stays refused.
#[test]
fn sim_records_crash_denied_run_stays_failed() -> Outcome {
    scenario("denied", 56..64, |path, seed| {
        let note = seed.to_be_bytes();
        let facts = [fact(seed, SUBMIT, &note), fact(seed, DENIED, b"")];
        let restored = crashed(path, &facts)?;
        assert_eq!(
            Ledger::of(&restored)?.standing(seed),
            Some(Standing::Failed)
        );
        Ok(restored.tail().head())
    })
}

/// A cancelled run stays cancelled.
#[test]
fn sim_records_crash_cancelled_run_stays_cancelled() -> Outcome {
    scenario("cancelled", 64..72, |path, seed| {
        let note = seed.to_be_bytes();
        let facts = [fact(seed, SUBMIT, &note), fact(seed, CANCELLED, b"")];
        let restored = crashed(path, &facts)?;
        assert_eq!(
            Ledger::of(&restored)?.standing(seed),
            Some(Standing::Cancelled)
        );
        Ok(restored.tail().head())
    })
}

/// A restore that walks by smaller pages folds the same ledger as a whole replay.
#[test]
fn sim_records_crash_restore_under_smaller_sufficient_capacity() -> Outcome {
    scenario("pages", 72..80, |path, seed| {
        let facts: Vec<Fact> = (0..8)
            .flat_map(|run| finished(seed.saturating_add(run)))
            .collect();
        let restored = crashed(path, &facts)?;
        let mut walked = Ledger::default();
        let mut cursor = 0;
        loop {
            let page = restored.page(cursor, 5)?;
            let Some(last) = page.last() else { break };
            cursor = last.position().sequence();
            for entry in &page {
                walked.apply(entry.record());
            }
        }
        assert_eq!(walked, Ledger::of(&restored)?);
        Ok(restored.tail().head())
    })
}

/// A resubmission after restore finds the command it duplicates.
#[test]
fn sim_records_crash_resubmit_after_restore_is_duplicate() -> Outcome {
    scenario("dup", 80..88, |path, seed| {
        let restored = crashed(path, &finished(seed))?;
        let identity = seed.to_be_bytes().to_vec();
        assert_eq!(Ledger::of(&restored)?.commands.get(&identity), Some(&seed));
        Ok(restored.tail().head())
    })
}

/// A retry that names the command under another run sees the first owner, and a
/// retry from a stale view cannot land at all.
#[test]
fn sim_records_crash_conflicting_retry_after_restore_refused() -> Outcome {
    scenario("conflict", 88..96, |path, seed| {
        let stale = {
            let journal = crashed(path, &[])?;
            journal.tail()
        };
        let mut restored = crashed(path, &finished(seed))?;
        let retry = fact(seed.saturating_add(1), SUBMIT, &seed.to_be_bytes());
        let refused = restored.compare_and_append(stale, &retry);
        assert!(matches!(refused, Err(JournalError::TailMismatch { .. })));
        let identity = seed.to_be_bytes().to_vec();
        assert_eq!(Ledger::of(&restored)?.commands.get(&identity), Some(&seed));
        Ok(restored.tail().head())
    })
}

/// Every entry replays with the same position on each restore.
#[test]
fn sim_records_crash_restored_authorizations_replay_identically() -> Outcome {
    scenario("positions", 96..104, |path, seed| {
        let first = crashed(path, &finished(seed))?;
        let once = first.page(0, 8)?;
        drop(first);
        let second = RecordJournal::<Fact>::open(path)?;
        assert_eq!(second.page(0, 8)?, once);
        Ok(second.tail().head())
    })
}

/// Two receipts both survive.
#[test]
fn sim_records_crash_two_receipts_survive() -> Outcome {
    scenario("two", 104..112, |path, seed| {
        let facts = [finished(seed), finished(seed.saturating_add(1))].concat();
        let restored = crashed(path, &facts)?;
        assert_eq!(Ledger::of(&restored)?.receipts.len(), 2);
        Ok(restored.tail().head())
    })
}

/// With no settlement to record, a lost outcome stays where it is.
#[test]
fn sim_records_crash_empty_owner_script_stays_reconciling() -> Outcome {
    scenario("idle", 112..120, |path, seed| {
        let restored = crashed(path, &lost(seed))?;
        drop(restored);
        let again = RecordJournal::<Fact>::open(path)?;
        assert_eq!(
            Ledger::of(&again)?.standing(seed),
            Some(Standing::Reconciling)
        );
        Ok(again.tail().head())
    })
}

/// A refused dispatch leaves the trace as if it had never been tried.
#[test]
fn sim_records_crash_fenced_dispatch_replays_same_trace() -> Outcome {
    scenario("fenced", 120..128, |path, seed| {
        let mut restored = crashed(path, &lost(seed))?;
        let before = restored.tail();
        let stale = restored.page(0, 1)?.first().ok_or("entry")?.position();
        let refused = restored.compare_and_append(stale, &fact(seed, DISPATCH, b""));
        assert!(matches!(refused, Err(JournalError::TailMismatch { .. })));
        assert_eq!(restored.tail(), before);
        Ok(restored.tail().head())
    })
}

/// The restored journal is exactly as long as what was acknowledged.
#[test]
fn sim_records_crash_journal_length_preserved() -> Outcome {
    scenario("length", 128..136, |path, seed| {
        let restored = crashed(path, &finished(seed))?;
        assert_eq!(restored.committed(), 3);
        assert_eq!(restored.tail().sequence(), 3);
        assert_eq!(restored.watermark().events().0, 3);
        Ok(restored.tail().head())
    })
}

/// A completed run survives two crashes in a row.
#[test]
fn sim_records_crash_two_crashes_keep_completed_run() -> Outcome {
    scenario("crashes", 136..144, |path, seed| {
        drop(crashed(path, &finished(seed))?);
        let restored = RecordJournal::<Fact>::open(path)?;
        assert_eq!(
            Ledger::of(&restored)?.standing(seed),
            Some(Standing::Completed)
        );
        Ok(restored.tail().head())
    })
}

/// A restore under the default bounds keeps the whole history.
#[test]
fn sim_records_crash_default_capacity_restore_keeps_history() -> Outcome {
    scenario("history", 144..152, |path, seed| {
        let facts: Vec<Fact> = (0..64)
            .flat_map(|run| finished(seed.saturating_add(run)))
            .collect();
        let restored = crashed(path, &facts)?;
        assert_eq!(held(&restored)?, facts);
        Ok(restored.tail().head())
    })
}

/// An outcome that is never settled stays unknown through restore after restore.
#[test]
fn sim_records_crash_unknown_permanent_stays_unknown() -> Outcome {
    scenario("permanent", 152..160, |path, seed| {
        drop(crashed(path, &lost(seed))?);
        drop(RecordJournal::<Fact>::open(path)?);
        let restored = RecordJournal::<Fact>::open(path)?;
        let ledger = Ledger::of(&restored)?;
        assert_eq!(ledger.standing(seed), Some(Standing::Reconciling));
        assert!(ledger.receipts.is_empty());
        Ok(restored.tail().head())
    })
}
