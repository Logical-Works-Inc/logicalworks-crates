//! A consumer that checks each record before it writes it, against a saboteur.
//!
//! Source: a consumer's runtime that folds every record ahead of its group-commit
//! sync and must refuse a bad record before the write (#905). Its refusal rules lived
//! twice, once in its pre-check and once in its fold; here they live once, in
//! `RecordFold::check`, and `RecordProjection::admit` runs them without touching the
//! view. The oracle is the same records folded by `rebuild` from the file.

use lgwks_bot::journal::{RecordEntry, RecordJournal};
use lgwks_bot::{ProjectionError, RecordFold, RecordProjection};
use lgwks_std::seeded::Seeded;

use crate::sim::scratch_file::ScratchFile;

/// A test's outcome: setup failures propagate instead of panicking.
type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Every row of a view, owned, in key order.
type Rows = Vec<(u64, Vec<u64>)>;

/// Why the guarded fold refused a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refused {
    /// The record's low half is the value this fold was told to refuse.
    Banned(u64),
    /// The row already holds its most records.
    Full(u64),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Banned(value) => write!(f, "the value {value} is refused"),
            Self::Full(tenant) => write!(f, "tenant {tenant}'s row is full"),
        }
    }
}

/// A record's tenant, which is its high half, and zero names no tenant.
const fn tenant_of(record: u64) -> u64 {
    record >> 32
}

/// A record's value, which is its low half.
const fn value_of(record: u64) -> u64 {
    record & 0xFFFF_FFFF
}

/// A record for `tenant` carrying `value`.
const fn record(tenant: u64, value: u64) -> u64 {
    (tenant << 32) | value
}

/// A fold whose refusal rules live once, in `check`, and which `apply` relies on.
struct Guarded {
    /// The value it refuses.
    banned: u64,
    /// The most records one row may hold.
    per_row: usize,
    /// Records naming no tenant.
    unrouted: u64,
}

impl Guarded {
    /// A fold refusing `banned`, with at most `per_row` records in a row.
    const fn new(banned: u64, per_row: usize) -> Self {
        Self {
            banned,
            per_row,
            unrouted: 0,
        }
    }
}

impl RecordFold<u64> for Guarded {
    type Key = u64;
    type Row = Vec<u64>;
    type Error = Refused;

    fn route(&self, record: &u64) -> Option<u64> {
        let tenant = tenant_of(*record);
        (tenant != 0).then_some(tenant)
    }

    fn apply(
        &mut self,
        entry: &RecordEntry<u64>,
        row: Option<&mut Vec<u64>>,
    ) -> Result<(), Refused> {
        self.check(entry.record(), row.as_deref())?;
        match row {
            Some(seen) => seen.push(entry.position().sequence()),
            None => self.unrouted = self.unrouted.saturating_add(1),
        }
        Ok(())
    }

    fn check(&self, record: &u64, row: Option<&Vec<u64>>) -> Result<(), Refused> {
        if value_of(*record) == self.banned {
            let refusal = Err(Refused::Banned(self.banned));
            lgwks_std::trace::debug!(error = ?refusal, "this fold refuses the value");
            return refusal;
        }
        if let Some(seen) = row
            && seen.len() >= self.per_row
        {
            let refusal = Err(Refused::Full(tenant_of(*record)));
            lgwks_std::trace::debug!(error = ?refusal, "this row is full");
            return refusal;
        }
        Ok(())
    }
}

/// Every row of `view`, owned, in key order.
fn rows_of(view: &RecordProjection<u64, Guarded>) -> Rows {
    view.rows()
        .map(|(key, seen)| (*key, seen.clone()))
        .collect()
}

/// Catches the bug this issue names: a refused first record leaving a default row
/// that `row` and `rows` then report.
#[test]
fn sim_record_projection_admit_saboteur_a_refused_first_record_leaves_no_row() -> Outcome {
    let mut view = RecordProjection::empty(Guarded::new(9, 8), 8);
    view.fold_one(&RecordEntry::staged(1, record(1, 1)))?;
    assert!(matches!(
        view.fold_one(&RecordEntry::staged(2, record(2, 9))),
        Err(ProjectionError::Fold(Refused::Banned(9)))
    ));
    assert_eq!(view.row(&2), None, "the refused tenant has no row");
    assert_eq!(rows_of(&view), [(1, vec![1])]);
    assert_eq!(view.applied(), Some(1));
    Ok(())
}

/// Catches a check that mutates, poisons, or answers differently from `fold_one`.
#[test]
fn sim_record_projection_admit_saboteur_a_refusal_admitted_first_never_poisons() -> Outcome {
    let mut view = RecordProjection::empty(Guarded::new(9, 2), 2);
    view.fold_one(&RecordEntry::staged(1, record(1, 1)))?;
    view.fold_one(&RecordEntry::staged(2, record(1, 2)))?;
    let before = rows_of(&view);
    assert!(matches!(
        view.admit(&record(1, 3)),
        Err(ProjectionError::Fold(Refused::Full(1)))
    ));
    assert!(matches!(
        view.admit(&record(3, 9)),
        Err(ProjectionError::Fold(Refused::Banned(9)))
    ));
    view.fold_one(&RecordEntry::staged(3, record(2, 3)))?;
    assert!(matches!(
        view.admit(&record(3, 4)),
        Err(ProjectionError::RowLimit { limit: 2 })
    ));
    assert!(
        view.admit(&record(2, 4)).is_ok(),
        "a held row is not a new one"
    );
    assert!(
        view.admit(&4).is_ok(),
        "a record about no row is checked with none"
    );
    assert_eq!(view.applied(), Some(3), "admit applied nothing");
    let mut after = before;
    after.push((2, vec![3]));
    assert_eq!(rows_of(&view), after);
    view.fold_one(&RecordEntry::staged(4, record(2, 4)))?;
    assert_eq!(
        view.applied(),
        Some(4),
        "the view still folds after every refusal"
    );
    Ok(())
}

/// Catches an admit that answers on a view an earlier failure left half applied.
#[test]
fn sim_record_projection_admit_saboteur_a_poisoned_view_admits_nothing() -> Outcome {
    let mut view = RecordProjection::empty(Guarded::new(9, 8), 8);
    assert!(matches!(
        view.fold_one(&RecordEntry::staged(2, record(1, 1))),
        Ok(())
    ));
    assert!(matches!(
        view.fold_one(&RecordEntry::staged(4, record(1, 2))),
        Err(ProjectionError::Diverged {
            expected: 3,
            found: 4
        })
    ));
    assert!(matches!(
        view.admit(&record(1, 3)),
        Err(ProjectionError::Poisoned)
    ));
    Ok(())
}

/// One seeded run: a consumer that admits before it writes, against a rebuild.
///
/// Each step draws a record that may be banned, may overfill a row, or may open a
/// tenant past the row limit. Only what `admit` passes is written and fed, so the
/// fed view must never poison and must equal a rebuild of the file row for row.
fn admitted_run(seed: u64) -> Outcome<(Rows, usize)> {
    let mut dice = Seeded::from_seed(seed);
    let scratch = ScratchFile::new(&format!("projection-admit-{seed}"))?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    let mut view = RecordProjection::empty(Guarded::new(7, 6), 5);
    let mut refused = 0_usize;
    let mut sequence = 0_u64;
    for _ in 0..200 {
        let candidate = record(dice.below(8)?, dice.below(12)?);
        if view.admit(&candidate).is_err() {
            refused = refused.saturating_add(1);
            continue;
        }
        journal.compare_and_append(journal.tail(), &candidate)?;
        sequence = sequence.saturating_add(1);
        view.fold_one(&RecordEntry::staged(sequence, candidate))
            .map_err(|cause| format!("seed {seed}: {cause:?}"))?;
    }
    let rebuilt = RecordProjection::rebuild(&journal, Guarded::new(7, 6), 5)
        .map_err(|cause| format!("seed {seed}: {cause:?}"))?;
    assert_eq!(rows_of(&view), rows_of(&rebuilt), "seed {seed}");
    assert_eq!(view.applied(), rebuilt.applied(), "seed {seed}");
    assert_eq!(view.fold().unrouted, rebuilt.fold().unrouted, "seed {seed}");
    Ok((rows_of(&view), refused))
}

/// Catches a check and a fold that drift: across seeds, every admitted record folds,
/// every refusal is caught before the write, and each seed replays to one trace.
#[test]
fn sim_record_projection_admit_monkey_admitted_writes_never_poison_and_replay() -> Outcome {
    let mut refusals = 0_usize;
    for seed in 0..64_u64 {
        let first = admitted_run(seed)?;
        let again = admitted_run(seed)?;
        assert_eq!(first, again, "seed {seed} replays to one trace");
        refusals = refusals.saturating_add(first.1);
    }
    assert!(refusals > 0, "the saboteur refused something");
    Ok(())
}
