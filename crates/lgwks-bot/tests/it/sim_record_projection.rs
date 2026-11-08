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
