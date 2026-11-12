//! A view asked about a record before the record is written (#905).
//!
//! Source: a consumer's group commit, which folds each record as it stages it and
//! wants to refuse a bad one before the sync without poisoning its view. The oracle
//! is a `BTreeMap` model kept beside the view and a rebuild of the file, and every
//! seed replays to one trace.

use std::collections::BTreeMap;

use lgwks_bot::journal::{RecordEntry, RecordJournal};
use lgwks_bot::{ProjectionError, RecordFold, RecordProjection};
use lgwks_std::seeded::Seeded;

use crate::sim::scratch_file::ScratchFile;
use crate::sim_record_projection::{Seen, fed, record, rows_of};

/// A test's outcome: setup failures propagate instead of panicking.
type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// One answer of `admit`, as this fold's view gives it.
type Answer = Result<(), ProjectionError<String>>;

/// A fold over tenants whose check forbids one record and whose apply refuses one.
#[derive(Default)]
struct Gate {
    /// The record `check` refuses, before anything changes.
    forbids: Option<u64>,
    /// The record `apply` refuses, as a fold that only learns late would.
    refuses: Option<u64>,
}

impl RecordFold<u64> for Gate {
    type Key = u64;
    type Row = Seen;
    type Error = String;

    fn route(&self, record: &u64) -> Option<u64> {
        let tenant = *record >> 32;
        (tenant != 0).then_some(tenant)
    }

    fn check(&self, record: &u64, _row: Option<&Seen>) -> Result<(), String> {
        if self.forbids == Some(*record) {
            let refusal = Err(format!("forbidden {record}"));
            lgwks_std::trace::debug!(error = ?refusal, "this fold's check forbids the record");
            return refusal;
        }
        Ok(())
    }

    fn apply(&mut self, entry: &RecordEntry<u64>, row: Option<&mut Seen>) -> Result<(), String> {
        if self.refuses == Some(*entry.record()) {
            let refusal = Err(format!("refused {}", entry.record()));
            lgwks_std::trace::debug!(error = ?refusal, "this fold's apply refuses the record");
            return refusal;
        }
        if let Some(seen) = row {
            seen.0.push(entry.position().sequence());
        }
        Ok(())
    }
}

/// Catches a refused first record leaving its row behind: the row joins the view only
/// once the fold took the record, whether `check` or `apply` refused it.
#[test]
fn sim_projection_admit_saboteur_a_refused_first_record_leaves_no_row() -> Outcome {
    for (forbids, refuses) in [(Some(record(4, 1)), None), (None, Some(record(4, 1)))] {
        let mut view = fed(&[record(1, 1)], Gate { forbids, refuses }, 8)?;
        assert!(matches!(
            view.fold_one(&RecordEntry::staged(2, record(4, 1))),
            Err(ProjectionError::Fold(_))
        ));
        assert_eq!(view.row(&4), None, "check {forbids:?}, apply {refuses:?}");
        assert_eq!(rows_of(&view), [(1, Seen(vec![1]))]);
        assert_eq!(view.applied(), Some(1));
    }
    Ok(())
}

/// Catches an `admit` that poisons, makes a row, or answers other than a fold would.
#[test]
fn sim_projection_admit_fat_finger_admit_refuses_without_poisoning() -> Outcome {
    let fold = Gate {
        forbids: Some(record(2, 7)),
        refuses: None,
    };
    let mut view = fed(&[record(1, 1)], fold, 2)?;
    for _ in 0..2 {
        assert!(matches!(
            view.admit(&record(2, 7)),
            Err(ProjectionError::Fold(_))
        ));
    }
    assert_eq!(rows_of(&view), [(1, Seen(vec![1]))], "admit made a row");
    assert_eq!(view.applied(), Some(1));
    view.admit(&record(2, 8))?;
    view.fold_one(&RecordEntry::staged(2, record(2, 8)))?;
    assert_eq!(view.row(&2), Some(&Seen(vec![2])));
    assert!(matches!(
        view.admit(&record(3, 1)),
        Err(ProjectionError::RowLimit { limit: 2 })
    ));
    view.admit(&record(1, 9))?;
    view.admit(&record(0, 5))?;
    assert!(matches!(
        view.fold_one(&RecordEntry::staged(3, record(2, 7))),
        Err(ProjectionError::Fold(_))
    ));
    assert!(matches!(
        view.admit(&record(1, 9)),
        Err(ProjectionError::Poisoned)
    ));
    Ok(())
}

/// The most rows the seeded view holds, small so the limit binds often.
const ROWS: usize = 4;

/// What `admit` must answer for `drawn`, from the model alone: the row limit is
/// asked before the fold's check.
fn expected(drawn: u64, forbidden: u64, model: &BTreeMap<u64, Seen>) -> Answer {
    let tenant = drawn >> 32;
    if tenant != 0 && !model.contains_key(&tenant) && model.len() >= ROWS {
        lgwks_std::trace::debug!(drawn, "expected: the model is full");
        return Err(ProjectionError::RowLimit { limit: ROWS });
    }
    if drawn == forbidden {
        lgwks_std::trace::debug!(drawn, "expected: the model forbids it");
        return Err(ProjectionError::Fold(format!("forbidden {drawn}")));
    }
    Ok(())
}

/// Whether two answers are the same variant.
const fn same(left: &Answer, right: &Answer) -> bool {
    matches!(
        (left, right),
        (&Ok(()), &Ok(()))
            | (
                &Err(ProjectionError::RowLimit { .. }),
                &Err(ProjectionError::RowLimit { .. })
            )
            | (
                &Err(ProjectionError::Fold(_)),
                &Err(ProjectionError::Fold(_))
            )
    )
}

/// Records drawn by `seed` are asked about, folded only when admitted, synced at
/// random, and checked against the model and a rebuild of the file.
fn admit_then_fold(seed: u64) -> Outcome<Vec<(u64, bool)>> {
    let mut dice = Seeded::from_seed(seed);
    let forbidden = record(dice.below(5)?.saturating_add(1), dice.below(8)?);
    let gate = || Gate {
        forbids: Some(forbidden),
        refuses: None,
    };
    let scratch = ScratchFile::new("projection-admit")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    let mut view = RecordProjection::empty(gate(), ROWS);
    let mut model: BTreeMap<u64, Seen> = BTreeMap::new();
    let mut staged = Vec::new();
    let mut trace = Vec::new();
    for _ in 0..80 {
        let drawn = record(dice.below(8)?, dice.below(8)?);
        let before = rows_of(&view);
        let asked = view.admit(&drawn);
        assert_eq!(
            rows_of(&view),
            before,
            "seed {seed}: admit changed the view"
        );
        let wanted = expected(drawn, forbidden, &model);
        if !same(&asked, &wanted) {
            lgwks_std::trace::debug!(seed, drawn, "admit_then_fold: admit left the model");
            return Err(
                format!("seed {seed}: {drawn} gave {asked:?}, the model {wanted:?}").into(),
            );
        }
        trace.push((drawn, asked.is_ok()));
        if asked.is_err() {
            continue;
        }
        let sequence = journal
            .tail()
            .sequence()
            .saturating_add(u64::try_from(staged.len())?)
            .saturating_add(1);
        view.fold_one(&RecordEntry::staged(sequence, drawn))
            .map_err(|cause| cause.to_string())?;
        staged.push(drawn);
        if drawn >> 32 != 0 {
            model.entry(drawn >> 32).or_default().0.push(sequence);
        }
        if dice.below(4)? == 0 {
            journal.compare_and_append_all(journal.tail(), &staged)?;
            staged.clear();
        }
    }
    if !staged.is_empty() {
        journal.compare_and_append_all(journal.tail(), &staged)?;
    }
    let held: BTreeMap<u64, Seen> = rows_of(&view).into_iter().collect();
    assert_eq!(held, model, "seed {seed}: the fed view left the model");
    let rebuilt = RecordProjection::rebuild(&journal, gate(), ROWS)?;
    assert_eq!(
        rows_of(&rebuilt),
        rows_of(&view),
        "seed {seed}: fed and rebuilt differ"
    );
    Ok(trace)
}

/// Catches an `admit` that disagrees with the fold or the row limit, and a fed view
/// that differs from a rebuild, across 48 seeds that each replay.
#[test]
fn sim_projection_admit_monkey_admitted_records_fold_and_replay() -> Outcome {
    for seed in 0..48 {
        assert_eq!(
            admit_then_fold(seed)?,
            admit_then_fold(seed)?,
            "seed {seed} did not replay"
        );
    }
    Ok(())
}
