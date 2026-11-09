//! A consumer's fold on the ECS world, against a reference model and a saboteur.
//!
//! Source: a consumer's replay and multi-tenant simulations (E55, D04, R71), ported
//! from its runtime onto `RecordProjection`. The oracle is a `BTreeMap` fold written
//! beside the view, and every seeded family replays to one trace.

use std::collections::BTreeMap;
use std::convert::Infallible;

use lgwks_bot::journal::{ContinuationPolicy, RecordEntry, RecordJournal};
use lgwks_bot::{ProjectionError, RecordFold, RecordProjection};
use lgwks_std::seeded::Seeded;

use crate::sim::scratch_file::ScratchFile;

/// A test's outcome: setup failures propagate instead of panicking.
type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A tenant's row: the sequences it saw, so the order is part of the answer.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
struct Seen(Vec<u64>);

/// A record's tenant, which is its high half, and zero names no tenant.
const fn tenant_of(record: u64) -> u64 {
    record >> 32
}

/// A fold over tenants, which refuses one value when told to.
#[derive(Default)]
struct Tenants {
    /// Records naming no tenant.
    unrouted: u64,
    /// The value it refuses, if any.
    refuses: Option<u64>,
}

impl RecordFold<u64> for Tenants {
    type Key = u64;
    type Row = Seen;
    type Error = String;

    fn route(&self, record: &u64) -> Option<u64> {
        let tenant = tenant_of(*record);
        (tenant != 0).then_some(tenant)
    }

    fn apply(&mut self, entry: &RecordEntry<u64>, row: Option<&mut Seen>) -> Result<(), String> {
        if self.refuses == Some(*entry.record()) {
            let refusal = Err(format!("refused {}", entry.record()));
            lgwks_std::trace::debug!(error = ?refusal, "this fold was told to refuse");
            return refusal;
        }
        match row {
            Some(seen) => seen.0.push(entry.position().sequence()),
            None => self.unrouted = self.unrouted.saturating_add(1),
        }
        Ok(())
    }
}

/// A fold that cannot fail, so the error type is never a test's subject.
struct Count;

impl RecordFold<u64> for Count {
    type Key = u64;
    type Row = u64;
    type Error = Infallible;

    fn route(&self, record: &u64) -> Option<u64> {
        Some(tenant_of(*record))
    }

    fn apply(&mut self, _: &RecordEntry<u64>, row: Option<&mut u64>) -> Result<(), Infallible> {
        if let Some(count) = row {
            *count = count.saturating_add(1);
        }
        Ok(())
    }
}

/// A record for `tenant` carrying `value`.
const fn record(tenant: u64, value: u64) -> u64 {
    (tenant << 32) | value
}

/// A view's rows as plain values, for comparing.
fn rows_of(view: &RecordProjection<u64, Tenants>) -> Vec<(u64, Seen)> {
    view.rows().map(|(key, row)| (*key, row.clone())).collect()
}

/// Catches an empty journal folding to anything but nothing.
#[test]
fn sim_record_projection_giant_and_empty_an_empty_journal_folds_empty() -> Outcome {
    let scratch = ScratchFile::new("projection-empty")?;
    let journal = RecordJournal::<u64>::open(scratch.path())?;
    let mut view = RecordProjection::rebuild(&journal, Tenants::default(), 8)?;
    assert_eq!(view.applied(), None);
    assert_eq!(view.rows().count(), 0);
    assert_eq!(view.catch_up(&journal)?, 0);
    assert_eq!(view.row(&1), None);
    Ok(())
}

/// Catches rows read in entity order: tenants arrive backwards and read forwards.
#[test]
fn sim_record_projection_wrong_order_rows_read_by_key_not_by_arrival() -> Outcome {
    let scratch = ScratchFile::new("projection-order")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    let backwards = [record(9, 1), record(3, 2), record(7, 3), record(3, 4)];
    journal.compare_and_append_all(journal.tail(), &backwards)?;
    let view = RecordProjection::rebuild(&journal, Tenants::default(), 8)?;
    let keys: Vec<u64> = view.rows().map(|(key, _)| *key).collect();
    assert_eq!(keys, [3, 7, 9]);
    assert_eq!(view.row(&3), Some(&Seen(vec![2, 4])));
    Ok(())
}

/// Catches a record that names no tenant making a row, or being lost from the fold.
#[test]
fn sim_record_projection_weird_bytes_a_record_for_no_row_still_reaches_the_fold() -> Outcome {
    let scratch = ScratchFile::new("projection-unrouted")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    journal.compare_and_append_all(journal.tail(), &[5, record(1, 1), 6])?;
    let view = RecordProjection::rebuild(&journal, Tenants::default(), 8)?;
    assert_eq!(view.fold().unrouted, 2);
    assert_eq!(view.rows().count(), 1);
    assert_eq!(view.applied(), Some(3));
    Ok(())
}

/// Catches a catch-up that refolds, skips, or differs from a rebuild.
#[test]
fn sim_record_projection_double_clicker_catching_up_twice_folds_once() -> Outcome {
    let scratch = ScratchFile::new("projection-twice")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    journal.compare_and_append_all(journal.tail(), &[record(1, 1)])?;
    let mut view = RecordProjection::rebuild(&journal, Tenants::default(), 8)?;
    let batch: Vec<u64> = (0..2_500)
        .map(|value| record(1 + value % 3, value))
        .collect();
    journal.compare_and_append_all(journal.tail(), &batch)?;
    assert_eq!(view.catch_up(&journal)?, 2_500, "past one page of the read");
    assert_eq!(view.catch_up(&journal)?, 0);
    let rebuilt = RecordProjection::rebuild(&journal, Tenants::default(), 8)?;
    assert_eq!(rows_of(&view), rows_of(&rebuilt));
    Ok(())
}

/// Catches a poisoned view that answers on: half a fold must never look whole.
#[test]
fn sim_record_projection_saboteur_a_failed_fold_poisons_until_rebuilt() -> Outcome {
    let scratch = ScratchFile::new("projection-poison")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    journal.compare_and_append_all(journal.tail(), &[record(1, 1)])?;
    let refusing = Tenants {
        refuses: Some(record(1, 2)),
        ..Tenants::default()
    };
    let mut view = RecordProjection::rebuild(&journal, refusing, 8)?;
    journal.compare_and_append_all(journal.tail(), &[record(1, 2), record(1, 3)])?;
    assert!(matches!(
        view.catch_up(&journal),
        Err(ProjectionError::Fold(_))
    ));
    assert_eq!(view.applied(), Some(1), "the refused record is not applied");
    assert!(matches!(
        view.catch_up(&journal),
        Err(ProjectionError::Poisoned)
    ));
    let rebuilt = RecordProjection::rebuild(&journal, Tenants::default(), 8)?;
    assert_eq!(rebuilt.applied(), Some(3));
    Ok(())
}

/// Catches an unbounded view, and a limit that fires on a row it already holds.
#[test]
fn sim_record_projection_giant_and_empty_the_row_limit_binds_on_new_keys_only() -> Outcome {
    let scratch = ScratchFile::new("projection-limit")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    journal.compare_and_append_all(journal.tail(), &[record(1, 1), record(2, 2), record(1, 3)])?;
    let mut view = RecordProjection::rebuild(&journal, Tenants::default(), 2)?;
    assert_eq!(view.applied(), Some(3));
    journal.compare_and_append(journal.tail(), &record(3, 4))?;
    assert!(matches!(
        view.catch_up(&journal),
        Err(ProjectionError::RowLimit { limit: 2 })
    ));
    assert!(matches!(
        RecordProjection::rebuild(&journal, Tenants::default(), 2),
        Err(ProjectionError::RowLimit { limit: 2 })
    ));
    Ok(())
}

/// Catches a view walking past a sealed generation without saying so.
#[test]
fn sim_record_projection_pulled_plug_a_continued_journal_rebuilds_from_its_checkpoint() -> Outcome {
    let scratch = ScratchFile::new("projection-seal")?;
    let policy = ContinuationPolicy::declared();
    let mut journal = RecordJournal::<u64>::open_continuing(scratch.path(), policy)?;
    journal.compare_and_append_all(journal.tail(), &[record(1, 1), record(1, 2)])?;
    let mut view = RecordProjection::rebuild(&journal, Count, 8)?;
    let mut next = journal
        .continue_with(&record(1, 9))?
        .ok_or("this journal continues")?;
    next.compare_and_append(next.tail(), &record(2, 3))?;
    let reopened = RecordProjection::rebuild(&next, Count, 8)?;
    assert_eq!(
        reopened.row(&1),
        Some(&1),
        "only the checkpoint is left of it"
    );
    assert_eq!(reopened.row(&2), Some(&1));
    assert_eq!(
        view.catch_up(&next)?,
        2,
        "the cursor is still inside this generation"
    );
    Ok(())
}

/// Catches one tenant's records reaching another's row, across a thousand tenants.
#[test]
fn sim_record_projection_neighbour_a_thousand_tenants_keep_their_own_rows() -> Outcome {
    let scratch = ScratchFile::new("projection-tenants")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    let batch: Vec<u64> = (0..5_000)
        .map(|value| record(1 + value % 1_000, value))
        .collect();
    journal.compare_and_append_all(journal.tail(), &batch)?;
    let view = RecordProjection::rebuild(&journal, Tenants::default(), 1_000)?;
    assert_eq!(view.rows().count(), 1_000);
    for (tenant, row) in view.rows() {
        assert_eq!(row.0.len(), 5, "tenant {tenant}");
        for sequence in &row.0 {
            let value = sequence.saturating_sub(1);
            assert_eq!(1 + value % 1_000, *tenant, "sequence {sequence}");
        }
    }
    Ok(())
}

/// A view of `records`, each fed at the sequence it would have.
fn fed(records: &[u64], fold: Tenants, limit: usize) -> Outcome<RecordProjection<u64, Tenants>> {
    let mut view = RecordProjection::empty(fold, limit);
    let mut sequence = 0_u64;
    for value in records {
        sequence = sequence.saturating_add(1);
        view.fold_one(&RecordEntry::staged(sequence, *value))
            .map_err(|cause| cause.to_string())?;
    }
    Ok(view)
}

/// Catches a fed view that differs from a rebuild of the same records, and a log kept
/// only in memory needing a file: the rebuild is the reference.
#[test]
fn sim_record_projection_neighbour_a_view_fed_with_no_file_equals_a_rebuild() -> Outcome {
    let scratch = ScratchFile::new("projection-fed")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    let batch: Vec<u64> = (0..3_000).map(|value| record(value % 7, value)).collect();
    journal.compare_and_append_all(journal.tail(), &batch)?;
    let view = fed(&batch, Tenants::default(), 8)?;
    let rebuilt = RecordProjection::rebuild(&journal, Tenants::default(), 8)?;
    assert_eq!(rows_of(&view), rows_of(&rebuilt));
    assert_eq!(view.fold().unrouted, rebuilt.fold().unrouted);
    assert_eq!(view.applied(), rebuilt.applied());
    Ok(())
}

/// Catches an empty view starting anywhere but the start of the journal.
#[test]
fn sim_record_projection_giant_and_empty_an_empty_view_catches_up_to_a_rebuild() -> Outcome {
    let scratch = ScratchFile::new("projection-empty-view")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    let mut view = RecordProjection::empty(Tenants::default(), 8);
    assert_eq!(view.applied(), None);
    assert_eq!(view.rows().count(), 0);
    assert_eq!(view.catch_up(&journal)?, 0);
    journal.compare_and_append_all(journal.tail(), &[record(1, 1), record(2, 2)])?;
    assert_eq!(view.catch_up(&journal)?, 2);
    let rebuilt = RecordProjection::rebuild(&journal, Tenants::default(), 8)?;
    assert_eq!(rows_of(&view), rows_of(&rebuilt));
    Ok(())
}

/// Catches a gap, a repeat or a step back folding on: each is named, then the view
/// refuses everything, and the refused record changed nothing.
#[test]
fn sim_record_projection_wrong_order_a_fed_record_out_of_order_poisons() -> Outcome {
    for (next, expected) in [(3, 2), (2, 2), (1, 2), (0, 2), (u64::MAX, 2)] {
        let mut view = fed(&[record(1, 1)], Tenants::default(), 8)?;
        let answered = view.fold_one(&RecordEntry::staged(next, record(1, 2)));
        if next == 2 {
            assert!(answered.is_ok(), "the next record folds");
            assert_eq!(view.applied(), Some(2));
            continue;
        }
        assert!(
            matches!(answered, Err(ProjectionError::Diverged { expected: wanted, found }) if wanted == expected && found == next),
            "sequence {next}"
        );
        assert_eq!(view.applied(), Some(1), "sequence {next}");
        assert_eq!(view.row(&1), Some(&Seen(vec![1])), "sequence {next}");
        assert!(matches!(
            view.fold_one(&RecordEntry::staged(2, record(1, 2))),
            Err(ProjectionError::Poisoned)
        ));
    }
    Ok(())
}

/// Catches the double-click: the same entry twice must not count twice.
#[test]
fn sim_record_projection_double_clicker_the_same_entry_fed_twice_is_refused() -> Outcome {
    let mut view = RecordProjection::empty(Tenants::default(), 8);
    let entry = RecordEntry::staged(1, record(1, 1));
    view.fold_one(&entry)?;
    assert!(matches!(
        view.fold_one(&entry),
        Err(ProjectionError::Diverged {
            expected: 2,
            found: 1
        })
    ));
    assert_eq!(view.row(&1), Some(&Seen(vec![1])));
    Ok(())
}

/// Catches a fed failure leaving a view that answers on, by either path: half a fold
/// must look the same wherever it came from.
#[test]
fn sim_record_projection_saboteur_a_refused_fed_record_poisons_both_paths() -> Outcome {
    let scratch = ScratchFile::new("projection-fed-poison")?;
    let journal = RecordJournal::<u64>::open(scratch.path())?;
    let refusing = Tenants {
        refuses: Some(record(1, 2)),
        ..Tenants::default()
    };
    let mut view = fed(&[record(1, 1)], refusing, 8)?;
    assert!(matches!(
        view.fold_one(&RecordEntry::staged(2, record(1, 2))),
        Err(ProjectionError::Fold(_))
    ));
    assert_eq!(view.applied(), Some(1), "the refused record is not applied");
    assert!(matches!(
        view.fold_one(&RecordEntry::staged(2, record(1, 3))),
        Err(ProjectionError::Poisoned)
    ));
    assert!(matches!(
        view.catch_up(&journal),
        Err(ProjectionError::Poisoned)
    ));
    Ok(())
}

/// Catches a limit that a fed record slips past, one that fires on a held row, and a
/// view left answering after it fired.
#[test]
fn sim_record_projection_giant_and_empty_a_fed_view_keeps_the_row_limit() -> Outcome {
    let mut view = fed(
        &[record(1, 1), record(2, 2), record(1, 3)],
        Tenants::default(),
        2,
    )?;
    assert_eq!(view.applied(), Some(3));
    assert!(matches!(
        view.fold_one(&RecordEntry::staged(4, record(3, 4))),
        Err(ProjectionError::RowLimit { limit: 2 })
    ));
    assert_eq!(view.row(&3), None);
    assert!(matches!(
        view.fold_one(&RecordEntry::staged(4, record(1, 4))),
        Err(ProjectionError::Poisoned)
    ));
    let mut none = RecordProjection::empty(Tenants::default(), 0);
    assert!(matches!(
        none.fold_one(&RecordEntry::staged(1, record(1, 1))),
        Err(ProjectionError::RowLimit { limit: 0 })
    ));
    Ok(())
}

/// Catches a record fed ahead of the sync being folded again when the file reaches it,
/// and a catch-up that fails because the view is ahead of the file.
#[test]
fn sim_record_projection_clock_fool_a_view_ahead_of_the_file_does_not_refold() -> Outcome {
    let scratch = ScratchFile::new("projection-ahead")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    let staged = [record(1, 1), record(1, 2), record(2, 3)];
    let mut view = fed(&staged, Tenants::default(), 8)?;
    assert_eq!(view.catch_up(&journal)?, 0, "the file holds nothing yet");
    journal.compare_and_append(journal.tail(), &staged[0])?;
    assert_eq!(view.catch_up(&journal)?, 0, "the file is still behind");
    journal.compare_and_append_all(journal.tail(), &staged[1..])?;
    assert_eq!(view.catch_up(&journal)?, 0, "the file reached the view");
    journal.compare_and_append(journal.tail(), &record(2, 4))?;
    assert_eq!(view.catch_up(&journal)?, 1, "only what is past the view");
    let rebuilt = RecordProjection::rebuild(&journal, Tenants::default(), 8)?;
    assert_eq!(rows_of(&view), rows_of(&rebuilt));
    Ok(())
}

/// A consumer's group commit: staged records reach the view at once and the file at a
/// sync, with crashes between. Checked at each step against what the file holds.
fn group_commit(seed: u64) -> Outcome<Vec<(u64, Seen)>> {
    let mut dice = Seeded::from_seed(seed);
    let scratch = ScratchFile::new("projection-group")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    let mut view = RecordProjection::empty(Tenants::default(), 64);
    let mut durable: BTreeMap<u64, Seen> = BTreeMap::new();
    let mut shown = durable.clone();
    let mut staged = Vec::new();
    let mut sequence = 0_u64;
    for _ in 0..60 {
        match dice.below(4)? {
            0 | 1 => {
                let tenant = dice.below(6)?;
                let value = record(tenant, dice.below(1_000)?);
                sequence = sequence.saturating_add(1);
                view.fold_one(&RecordEntry::staged(sequence, value))
                    .map_err(|cause| cause.to_string())?;
                staged.push(value);
                if tenant != 0 {
                    shown.entry(tenant).or_default().0.push(sequence);
                }
            }
            2 => {
                if !staged.is_empty() {
                    journal.compare_and_append_all(journal.tail(), &staged)?;
                }
                staged.clear();
                durable.clone_from(&shown);
                assert_eq!(
                    view.catch_up(&journal)?,
                    0,
                    "seed {seed}: the sync refolded"
                );
            }
            _ => {
                staged.clear();
                sequence = journal.tail().sequence();
                shown.clone_from(&durable);
                view = RecordProjection::rebuild(&journal, Tenants::default(), 64)?;
                let held: BTreeMap<u64, Seen> = rows_of(&view).into_iter().collect();
                assert_eq!(held, durable, "seed {seed}: a crash keeps only the file");
            }
        }
        let held: BTreeMap<u64, Seen> = rows_of(&view).into_iter().collect();
        assert_eq!(held, shown, "seed {seed}: the view shows what was staged");
    }
    Ok(rows_of(&view))
}

/// Catches a view that loses a staged record, keeps a crashed one, or replays differently.
#[test]
fn sim_record_projection_pulled_plug_a_group_commit_matches_the_model_and_replays() -> Outcome {
    for seed in 0..48 {
        assert_eq!(
            group_commit(seed)?,
            group_commit(seed)?,
            "seed {seed} did not replay"
        );
    }
    Ok(())
}

/// Every operation a seed draws, checked against the reference model, as a trace.
fn monkey(seed: u64) -> Outcome<Vec<(u64, Seen)>> {
    let mut dice = Seeded::from_seed(seed);
    let scratch = ScratchFile::new("projection-monkey")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    let mut view = RecordProjection::rebuild(&journal, Tenants::default(), 64)?;
    let mut model: BTreeMap<u64, Seen> = BTreeMap::new();
    let mut sequence = 0_u64;
    for _ in 0..40 {
        match dice.below(3)? {
            0 => {
                let count = dice.below(8)?;
                let mut batch = Vec::new();
                for _ in 0..count {
                    let tenant = dice.below(6)?;
                    batch.push(record(tenant, dice.below(1_000)?));
                    sequence = sequence.saturating_add(1);
                    if tenant != 0 {
                        model.entry(tenant).or_default().0.push(sequence);
                    }
                }
                if !batch.is_empty() {
                    journal.compare_and_append_all(journal.tail(), &batch)?;
                }
            }
            1 => {
                view.catch_up(&journal)?;
                let held: BTreeMap<u64, Seen> = rows_of(&view).into_iter().collect();
                assert_eq!(held, model, "seed {seed} after a catch-up");
            }
            _ => {
                view = RecordProjection::rebuild(&journal, Tenants::default(), 64)?;
                let held: BTreeMap<u64, Seen> = rows_of(&view).into_iter().collect();
                assert_eq!(held, model, "seed {seed} after a rebuild");
            }
        }
    }
    let view = RecordProjection::rebuild(&journal, Tenants::default(), 64)?;
    Ok(rows_of(&view))
}

/// Catches any drift from the model, and any scheduler or hash order in the rows.
#[test]
fn sim_record_projection_monkey_random_ops_match_the_model_and_replay_exactly() -> Outcome {
    for seed in 0..48 {
        assert_eq!(monkey(seed)?, monkey(seed)?, "seed {seed} did not replay");
    }
    Ok(())
}
