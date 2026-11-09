//! A consumer's fold over its own record journal, held on the ECS world.
//!
//! The journal is the truth and this is a view of it. [`RecordProjection::rebuild`]
//! folds every record of the live generation in order, one row per key, so a restart
//! is a rebuild and two rebuilds of one journal are equal row for row. Rows are
//! visited in key order rather than entity order, because an entity id says when a
//! row was made and nothing about what it means.
//!
//! A fold that fails leaves the view half applied, so the view refuses every later
//! call until the host rebuilds it. A continued journal is read from its checkpoint,
//! so the checkpoint must carry what the fold needs, and
//! [`RecordEntry::is_checkpoint`] says which record that is.
//!
//! A consumer that batches its writes can read its view before the sync lands, by
//! feeding it through [`RecordProjection::fold_one`]. Until then the view holds a
//! record the journal does not, and a crash loses it. The journal is still the
//! truth: a restart rebuilds from the file, and what the sync never wrote is gone
//! from the view as it is from the log (ADR 0024).

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

// `self` is load-bearing: the `Component` derive expands to `bevy_ecs::…` paths.
use lgwks_deps::bevy_ecs::{
    self,
    prelude::{Component, Entity, World},
};

use crate::journal::{Durable, JournalError, RecordEntry, RecordJournal};

/// How many records one read of a catch-up asks for.
const CATCH_UP_PAGE: usize = 1_024;

/// What a consumer's records mean to its view.
///
/// # Examples
///
/// ```
/// use lgwks_bot::journal::RecordEntry;
/// use lgwks_bot::RecordFold;
///
/// /// Counts each tenant's records: the high half of a record names the tenant.
/// #[derive(Default)]
/// struct Tally;
///
/// impl RecordFold<u64> for Tally {
///     type Key = u64;
///     type Row = u64;
///     type Error = std::convert::Infallible;
///
///     fn route(&self, record: &u64) -> Option<u64> {
///         Some(*record >> 32)
///     }
///
///     fn apply(
///         &mut self,
///         _entry: &RecordEntry<u64>,
///         row: Option<&mut u64>,
///     ) -> Result<(), Self::Error> {
///         if let Some(count) = row {
///             *count = count.saturating_add(1);
///         }
///         Ok(())
///     }
/// }
/// ```
pub trait RecordFold<R> {
    /// What a row is found by, and the order rows are read in.
    type Key: Ord + Clone;
    /// One row of the view, starting from its default.
    type Row: Default + Send + Sync + 'static;
    /// Why a record could not be folded.
    type Error;

    /// The row `record` belongs to, or `None` for a record about no row.
    fn route(&self, record: &R) -> Option<Self::Key>;

    /// Fold one entry, with its row when [`route`](Self::route) named one.
    ///
    /// # Errors
    ///
    /// Whatever the consumer says: a failure stops the fold and poisons the view.
    fn apply(
        &mut self,
        entry: &RecordEntry<R>,
        row: Option<&mut Self::Row>,
    ) -> Result<(), Self::Error>;
}

/// Why a view could not be built or brought up to date.
///
/// # Examples
///
/// ```
/// use lgwks_bot::ProjectionError;
///
/// let refused = ProjectionError::<std::convert::Infallible>::RowLimit { limit: 8 };
/// assert_eq!(refused.to_string(), "the view holds its limit of 8 rows");
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum ProjectionError<E> {
    /// The consumer's fold refused a record.
    Fold(E),
    /// The journal refused the read.
    Journal(JournalError),
    /// A record is not the one after the last the view applied.
    Diverged {
        /// The sequence the view expected.
        expected: u64,
        /// The sequence the journal returned.
        found: u64,
    },
    /// A new row would pass the limit the view was built with.
    RowLimit {
        /// The limit.
        limit: usize,
    },
    /// An earlier failure left this view half applied. Rebuild it.
    Poisoned,
}

impl<E: fmt::Display> fmt::Display for ProjectionError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Fold(ref cause) => write!(f, "the fold refused a record: {cause}"),
            Self::Journal(ref cause) => write!(f, "the journal refused the read: {cause}"),
            Self::Diverged { expected, found } => write!(
                f,
                "the view expected record {expected} and the journal returned {found}"
            ),
            Self::RowLimit { limit } => write!(f, "the view holds its limit of {limit} rows"),
            Self::Poisoned => f.write_str("an earlier failure left this view half applied"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for ProjectionError<E> {}

/// One row, as the world holds it.
#[derive(Component)]
struct Held<T: Send + Sync + 'static>(T);

/// A consumer's view of its journal: a fold and one world entity per key.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use lgwks_bot::journal::{RecordEntry, RecordJournal};
/// use lgwks_bot::{RecordFold, RecordProjection};
///
/// #[derive(Default)]
/// struct Tally;
///
/// impl RecordFold<u64> for Tally {
///     type Key = u64;
///     type Row = u64;
///     type Error = std::convert::Infallible;
///
///     fn route(&self, record: &u64) -> Option<u64> {
///         Some(*record >> 32)
///     }
///
///     fn apply(&mut self, _: &RecordEntry<u64>, row: Option<&mut u64>) -> Result<(), Self::Error> {
///         if let Some(count) = row {
///             *count = count.saturating_add(1);
///         }
///         Ok(())
///     }
/// }
///
/// let path = std::env::temp_dir().join(format!("projection-doc-{}", std::process::id()));
/// # drop(std::fs::remove_file(&path));
/// let mut journal = RecordJournal::<u64>::open(&path)?;
/// journal.compare_and_append_all(journal.tail(), &[1 << 32, 2 << 32, 1 << 32])?;
///
/// let mut view = RecordProjection::rebuild(&journal, Tally, 16)?;
/// assert_eq!(view.row(&1), Some(&2));
/// assert_eq!(view.applied(), Some(3));
///
/// journal.compare_and_append(journal.tail(), &(2 << 32))?;
/// assert_eq!(view.catch_up(&journal)?, 1);
/// let rows: Vec<(&u64, &u64)> = view.rows().collect();
/// assert_eq!(rows, [(&1, &2), (&2, &2)]);
/// assert!(view.fold().route(&0).is_some());
/// # drop(journal);
/// # drop(std::fs::remove_file(&path));
/// # Ok(())
/// # }
/// ```
pub struct RecordProjection<R, F: RecordFold<R>> {
    /// The consumer's fold, with whatever it keeps beside the rows.
    fold: F,
    /// The world the rows live on.
    world: World,
    /// Each key's entity, in the order rows are read.
    entities: BTreeMap<F::Key, Entity>,
    /// The sequence of the last record applied.
    applied: Option<u64>,
    /// The most rows this view may hold.
    max_rows: usize,
    /// Whether a failure left this view half applied.
    poisoned: bool,
    /// The record type this view folds.
    records: PhantomData<fn(&R)>,
}

impl<R: Durable, F: RecordFold<R>> RecordProjection<R, F> {
    /// Fold every record of `journal`'s live generation into a new view.
    ///
    /// # Errors
    ///
    /// Any [`ProjectionError`] but `Poisoned`: nothing half built is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use lgwks_bot::journal::{RecordEntry, RecordJournal};
    /// use lgwks_bot::{RecordFold, RecordProjection};
    ///
    /// struct Empty;
    ///
    /// impl RecordFold<u64> for Empty {
    ///     type Key = u64;
    ///     type Row = u64;
    ///     type Error = std::convert::Infallible;
    ///     fn route(&self, _: &u64) -> Option<u64> {
    ///         None
    ///     }
    ///     fn apply(&mut self, _: &RecordEntry<u64>, _: Option<&mut u64>) -> Result<(), Self::Error> {
    ///         Ok(())
    ///     }
    /// }
    ///
    /// let path = std::env::temp_dir().join(format!("projection-doc-b-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let journal = RecordJournal::<u64>::open(&path)?;
    /// let view = RecordProjection::rebuild(&journal, Empty, 16)?;
    /// assert_eq!(view.applied(), None);
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    pub fn rebuild(
        journal: &RecordJournal<R>,
        fold: F,
        max_rows: usize,
    ) -> Result<Self, ProjectionError<F::Error>> {
        let mut view = Self::empty(fold, max_rows);
        for entry in journal.replay().map_err(ProjectionError::Journal)? {
            view.step(&entry.map_err(ProjectionError::Journal)?)?;
        }
        Ok(view)
    }

    /// Fold what the journal gained since the last record applied, and say how much.
    ///
    /// # Errors
    ///
    /// [`ProjectionError::Poisoned`] after any earlier failure of this call, and any
    /// other refusal poisons the view. A journal continued past this view's cursor
    /// answers [`JournalError::CursorBehindGeneration`] without poisoning it, and
    /// rebuilding is the way forward.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::RecordJournal;
    /// use lgwks_bot::{ProjectionError, RecordFold, RecordProjection};
    ///
    /// fn folded<F: RecordFold<u64>>(
    ///     view: &mut RecordProjection<u64, F>,
    ///     journal: &RecordJournal<u64>,
    /// ) -> Result<usize, ProjectionError<F::Error>> {
    ///     view.catch_up(journal)
    /// }
    /// ```
    pub fn catch_up(
        &mut self,
        journal: &RecordJournal<R>,
    ) -> Result<usize, ProjectionError<F::Error>> {
        if self.poisoned {
            let refusal = Err(ProjectionError::Poisoned);
            lgwks_std::trace::debug!(applied = ?self.applied, "RecordProjection::catch_up: this view must be rebuilt");
            return refusal;
        }
        let mut folded = 0_usize;
        loop {
            let mut after = 0;
            if let Some(last) = self.applied {
                after = last;
            }
            let page = journal
                .page(after, CATCH_UP_PAGE)
                .map_err(ProjectionError::Journal)?;
            if page.is_empty() {
                return Ok(folded);
            }
            for entry in &page {
                self.fold_one(entry)?;
                folded = folded.saturating_add(1);
            }
        }
    }
}

impl<R, F: RecordFold<R>> RecordProjection<R, F> {
    /// A view of nothing yet, for a consumer that feeds it with
    /// [`fold_one`](Self::fold_one) or [`catch_up`](Self::catch_up)s it from the start.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::{RecordFold, RecordProjection};
    ///
    /// fn fresh<F: RecordFold<u64>>(fold: F) -> bool {
    ///     let view: RecordProjection<u64, F> = RecordProjection::empty(fold, 16);
    ///     view.applied().is_none() && view.rows().count() == 0
    /// }
    /// ```
    #[must_use]
    pub fn empty(fold: F, max_rows: usize) -> Self {
        Self {
            fold,
            world: World::new(),
            entities: BTreeMap::new(),
            applied: None,
            max_rows,
            poisoned: false,
            records: PhantomData,
        }
    }

    /// Fold one entry, which may be one no file holds yet.
    ///
    /// The first entry of a view may have any sequence, as a continued journal's
    /// does, and each later one must be the next. A record the sync then loses stays
    /// in the view until the host rebuilds it from the file, which is what a restart
    /// does. [`catch_up`](Self::catch_up) reads past this view's cursor, so a record
    /// fed here is not folded again when the file reaches it. Nothing compares the
    /// two: a host that writes something else at that sequence must rebuild.
    ///
    /// # Errors
    ///
    /// [`ProjectionError::Poisoned`] after any earlier failure. [`Fold`], [`Diverged`]
    /// and [`RowLimit`] poison this view, as they do on a catch-up.
    ///
    /// [`Fold`]: ProjectionError::Fold
    /// [`Diverged`]: ProjectionError::Diverged
    /// [`RowLimit`]: ProjectionError::RowLimit
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use lgwks_bot::journal::RecordEntry;
    /// use lgwks_bot::{ProjectionError, RecordFold, RecordProjection};
    ///
    /// struct Tally;
    ///
    /// impl RecordFold<u64> for Tally {
    ///     type Key = u64;
    ///     type Row = u64;
    ///     type Error = std::convert::Infallible;
    ///
    ///     fn route(&self, record: &u64) -> Option<u64> {
    ///         Some(*record)
    ///     }
    ///
    ///     fn apply(&mut self, _: &RecordEntry<u64>, row: Option<&mut u64>) -> Result<(), Self::Error> {
    ///         if let Some(count) = row {
    ///             *count = count.saturating_add(1);
    ///         }
    ///         Ok(())
    ///     }
    /// }
    ///
    /// let mut view = RecordProjection::empty(Tally, 16);
    /// view.fold_one(&RecordEntry::staged(1, 5))?;
    /// assert_eq!(view.row(&5), Some(&1));
    /// assert!(matches!(
    ///     view.fold_one(&RecordEntry::staged(3, 5)),
    ///     Err(ProjectionError::Diverged { expected: 2, found: 3 })
    /// ));
    /// assert!(matches!(
    ///     view.fold_one(&RecordEntry::staged(2, 5)),
    ///     Err(ProjectionError::Poisoned)
    /// ));
    /// # Ok(())
    /// # }
    /// ```
    pub fn fold_one(&mut self, entry: &RecordEntry<R>) -> Result<(), ProjectionError<F::Error>> {
        if self.poisoned {
            let refusal = Err(ProjectionError::Poisoned);
            lgwks_std::trace::debug!(applied = ?self.applied, "RecordProjection::fold_one: this view must be rebuilt");
            return refusal;
        }
        let refusal = self.step(entry);
        if refusal.is_err() {
            self.poisoned = true;
            lgwks_std::trace::debug!(applied = ?self.applied, "RecordProjection::fold_one: poisoned");
        }
        refusal
    }

    /// Fold one entry.
    fn step(&mut self, entry: &RecordEntry<R>) -> Result<(), ProjectionError<F::Error>> {
        let found = entry.position().sequence();
        if let Some(last) = self.applied {
            let expected = last.saturating_add(1);
            if found != expected {
                let refusal = Err(ProjectionError::Diverged { expected, found });
                lgwks_std::trace::debug!(
                    expected,
                    found,
                    "RecordProjection: a record out of order"
                );
                return refusal;
            }
        }
        let row = match self.fold.route(entry.record()) {
            Some(key) => Some(self.entity_for(key)?),
            None => None,
        };
        let mut held = row.and_then(|entity| self.world.get_mut::<Held<F::Row>>(entity));
        let folded = match held {
            Some(ref mut cell) => self.fold.apply(entry, Some(&mut cell.0)),
            None => self.fold.apply(entry, None),
        };
        folded.map_err(ProjectionError::Fold)?;
        self.applied = Some(found);
        Ok(())
    }

    /// The entity holding `key`'s row, made on first use up to the limit.
    fn entity_for(&mut self, key: F::Key) -> Result<Entity, ProjectionError<F::Error>> {
        if let Some(found) = self.entities.get(&key) {
            return Ok(*found);
        }
        if self.entities.len() >= self.max_rows {
            let refusal = Err(ProjectionError::RowLimit {
                limit: self.max_rows,
            });
            lgwks_std::trace::debug!(limit = self.max_rows, "RecordProjection: the view is full");
            return refusal;
        }
        let entity = self.world.spawn(Held(F::Row::default())).id();
        self.entities.insert(key, entity);
        Ok(entity)
    }
}

impl<R, F: RecordFold<R>> RecordProjection<R, F> {
    /// The row `key` names.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::{RecordFold, RecordProjection};
    ///
    /// fn holds<F: RecordFold<u64>>(view: &RecordProjection<u64, F>, key: &F::Key) -> bool {
    ///     view.row(key).is_some()
    /// }
    /// ```
    #[must_use]
    pub fn row(&self, key: &F::Key) -> Option<&F::Row> {
        let entity = self.entities.get(key)?;
        let held = self.world.get::<Held<F::Row>>(*entity)?;
        Some(&held.0)
    }

    /// Every row, in key order.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::{RecordFold, RecordProjection};
    ///
    /// fn count<F: RecordFold<u64>>(view: &RecordProjection<u64, F>) -> usize {
    ///     view.rows().count()
    /// }
    /// ```
    pub fn rows(&self) -> impl Iterator<Item = (&F::Key, &F::Row)> {
        self.entities.iter().filter_map(|(key, entity)| {
            let held = self.world.get::<Held<F::Row>>(*entity)?;
            Some((key, &held.0))
        })
    }

    /// The fold, with whatever it keeps beside the rows.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::{RecordFold, RecordProjection};
    ///
    /// fn routes<F: RecordFold<u64>>(view: &RecordProjection<u64, F>, record: &u64) -> bool {
    ///     view.fold().route(record).is_some()
    /// }
    /// ```
    #[must_use]
    pub const fn fold(&self) -> &F {
        &self.fold
    }

    /// The sequence of the last record applied, or `None` before the first.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::{RecordFold, RecordProjection};
    ///
    /// fn started<F: RecordFold<u64>>(view: &RecordProjection<u64, F>) -> bool {
    ///     view.applied().is_some()
    /// }
    /// ```
    #[must_use]
    pub const fn applied(&self) -> Option<u64> {
        self.applied
    }
}

impl<R, F: RecordFold<R>> fmt::Debug for RecordProjection<R, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordProjection")
            .field("rows", &self.entities.len())
            .field("applied", &self.applied)
            .field("poisoned", &self.poisoned)
            .finish_non_exhaustive()
    }
}
