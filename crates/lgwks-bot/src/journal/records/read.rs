//! What a handle answers without touching the owner: its counts, its pages and its replay.

use super::super::continuation::ContinuationWatermark;
use super::super::frame::SaturatingFrom;
use super::super::{
    DurabilityPromise, JournalError, JournalPosition, MAX_JOURNAL_BYTES, MAX_JOURNAL_EVENTS,
};
use super::replay::{RecordEntry, RecordReplay};
use super::{Durable, RecordJournal};

/// The most entries one [`RecordJournal::page`] answers.
///
/// A cursor is how a consumer asks for more, so a smaller page than the one asked
/// for is an ordinary answer. A bound here is what keeps one call's memory at this
/// many frames of at most the frame ceiling.
const MAX_PAGE_ENTRIES: usize = 1024;

impl<R: Durable> RecordJournal<R> {
    /// Up to `limit` entries after sequence `after`, in commit order.
    ///
    /// At most 1,024 come back at once; a cursor is how a consumer asks for more.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::RecordJournal;
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-page-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let mut journal = RecordJournal::<u64>::open(&path)?;
    /// journal.compare_and_append_all(journal.tail(), &[5, 6, 7])?;
    /// let page = journal.page(1, 1)?;
    /// assert_eq!(page.first().map(|entry| *entry.record()), Some(6));
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`JournalError::CursorBehindGeneration`] when `after` names a record older
    /// than this generation holds, which a silent start at the checkpoint would
    /// hide from a consumer folding by cursor, and whatever a replay reports.
    pub fn page(&self, after: u64, limit: usize) -> Result<Vec<RecordEntry<R>>, JournalError> {
        let first = self.first_sequence();
        if after.saturating_add(1) < first {
            let refusal = Err(JournalError::CursorBehindGeneration {
                cursor: after,
                first,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "RecordJournal::page: this cursor is older than the generation");
            return refusal;
        }
        let index = usize::saturating_from(after.saturating_add(1).saturating_sub(first));
        self.replay_from(index)?
            .take(limit.min(MAX_PAGE_ENTRIES))
            .collect()
    }

    /// Stream every entry of this generation from its own descriptor.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::RecordJournal;
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-replay-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let mut journal = RecordJournal::<u64>::open(&path)?;
    /// journal.compare_and_append_all(journal.tail(), &[5, 6])?;
    /// let entries: Result<Vec<_>, _> = journal.replay()?.collect();
    /// assert_eq!(entries?.len(), 2);
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`JournalError::Storage`] when the descriptor cannot be opened.
    pub fn replay(&self) -> Result<RecordReplay<R>, JournalError> {
        self.replay_from(0)
    }

    /// A replay that begins at entry `index` of this file.
    fn replay_from(&self, index: usize) -> Result<RecordReplay<R>, JournalError> {
        let end = self.disk_len;
        let Some(previous) = index.checked_sub(1) else {
            return RecordReplay::open(&self.path, self.base, self.carried, 0, 0, end);
        };
        let Some(offset) = self.offsets.get(index).copied() else {
            return RecordReplay::open(&self.path, self.position, false, index, end, end);
        };
        let sequence = self
            .first_sequence()
            .saturating_add(u64::saturating_from(previous));
        let from = RecordReplay::<R>::position_before(&self.path, offset, sequence)?;
        RecordReplay::open(&self.path, from, false, index, offset, end)
    }
}

impl<R> RecordJournal<R> {
    /// The last committed position, or the genesis when nothing is committed.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::{JournalPosition, RecordJournal};
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-tail-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let journal = RecordJournal::<u64>::open(&path)?;
    /// assert_eq!(journal.tail(), JournalPosition::genesis());
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn tail(&self) -> JournalPosition {
        self.position
    }

    /// How many entries this file holds, its carried checkpoint included.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::RecordJournal;
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-count-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let journal = RecordJournal::<u64>::open(&path)?;
    /// assert_eq!(journal.committed(), 0);
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn committed(&self) -> usize {
        self.offsets.len()
    }

    /// The sequence of this file's first entry: one for a base journal, and the
    /// checkpoint's for a successor.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::RecordJournal;
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-first-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let journal = RecordJournal::<u64>::open(&path)?;
    /// assert_eq!(journal.first_sequence(), 1);
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn first_sequence(&self) -> u64 {
        self.base.sequence().saturating_add(1)
    }

    /// Whether opening took back an append a killed writer never finished.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::{JournalError, RecordJournal};
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-torn-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let mut journal = RecordJournal::<u64>::open(&path)?;
    /// journal.compare_and_append(journal.tail(), &1)?;
    /// drop(journal);
    /// let mut bytes = std::fs::read(&path).map_err(JournalError::Storage)?;
    /// bytes.pop();
    /// std::fs::write(&path, bytes).map_err(JournalError::Storage)?;
    /// let repaired = RecordJournal::<u64>::open(&path)?;
    /// assert!(repaired.torn_tail_repaired());
    /// # drop(repaired);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn torn_tail_repaired(&self) -> bool {
        self.repaired
    }

    /// What this journal promises about an acknowledged append.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::{DurabilityPromise, RecordJournal};
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-promise-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let journal = RecordJournal::<u64>::open(&path)?;
    /// assert!(journal.durability().meets(DurabilityPromise::ProcessCrash));
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn durability(&self) -> DurabilityPromise {
        DurabilityPromise::ProcessCrash
    }

    /// How much of each ceiling this journal has used, and whether it continues.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::RecordJournal;
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-mark-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let journal = RecordJournal::<u64>::open(&path)?;
    /// assert!(!journal.watermark().is_due());
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn watermark(&self) -> ContinuationWatermark {
        let events = u64::saturating_from(self.offsets.len());
        let events_limit = u64::saturating_from(MAX_JOURNAL_EVENTS);
        match self.policy {
            Some(policy) => ContinuationWatermark::measured(
                events,
                events_limit,
                self.disk_len,
                MAX_JOURNAL_BYTES,
                policy,
            ),
            None => {
                ContinuationWatermark::inert(events, events_limit, self.disk_len, MAX_JOURNAL_BYTES)
            }
        }
    }
}
