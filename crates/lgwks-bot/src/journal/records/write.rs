//! The doors a record reaches the disk through: the append, the batch and the seal.

use super::super::continuation::{SealPause, generation_of, successor_path};
use super::super::file::{MAX_FRAME_BYTES, SealOutcome, commit, seal_on_owner, write_refusal};
use super::super::frame::{SaturatingFrom, frame_record, framed_len};
use super::super::{DurableAck, JournalError, JournalLimitKind, JournalPosition, chain_over_bytes};
use super::scan::SEAL_HEADER;
use super::{Durable, RECORD_TAG, RecordJournal, SEAL_TAG, bound_bytes, bound_events};

impl<R: Durable> RecordJournal<R> {
    /// Append `record` if `expected_tail` is still the tail, and return once it is
    /// on the disk.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::{JournalError, RecordJournal};
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-cas-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let mut journal = RecordJournal::<u64>::open(&path)?;
    /// let stale = journal.tail();
    /// journal.compare_and_append(stale, &1)?;
    /// let refused = journal.compare_and_append(stale, &2);
    /// assert!(matches!(refused, Err(JournalError::TailMismatch { .. })));
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`JournalError::TailMismatch`] when the caller's tail is stale,
    /// [`JournalError::Superseded`] when this handle sealed its journal,
    /// [`JournalError::CapacityExceeded`] over a ceiling,
    /// [`JournalError::Encoding`] when the record does not archive,
    /// [`JournalError::Storage`] when the handle is stale and nothing was written,
    /// and [`JournalError::OutcomeUnknown`] when bytes may be on the disk with the
    /// reply lost, after which only a reopen settles it.
    pub fn compare_and_append(
        &mut self,
        expected_tail: JournalPosition,
        record: &R,
    ) -> Result<DurableAck, JournalError> {
        let (position, frames) = self.prepare(expected_tail, record)?;
        let (expected, length) = (self.disk_len, frames.len());
        self.storage
            .submit(move |file, _state| commit(file, expected, &frames))
            .map_err(write_refusal)?;
        Ok(self.accept(position, length))
    }

    /// The same append, for a task that must not sit through the device.
    ///
    /// A caller that walks away mid-await leaves bytes this handle never counted,
    /// so its fence refuses the next append until a reopen replays the truth.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::RecordJournal;
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-await-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let mut journal = RecordJournal::<u64>::open(&path)?;
    /// let tail = journal.tail();
    /// lgwks_std::task::block_on(journal.compare_and_append_async(tail, &1))?;
    /// assert_eq!(journal.committed(), 1);
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Whatever [`RecordJournal::compare_and_append`] reports.
    pub async fn compare_and_append_async(
        &mut self,
        expected_tail: JournalPosition,
        record: &R,
    ) -> Result<DurableAck, JournalError> {
        let (position, frames) = self.prepare(expected_tail, record)?;
        let (expected, length) = (self.disk_len, frames.len());
        self.storage
            .enqueue_awaiting(move |file, _state| commit(file, expected, &frames))
            .await
            .map_err(write_refusal)?;
        Ok(self.accept(position, length))
    }

    /// Append every record in one write and one flush, or none of them.
    ///
    /// Every check runs before a byte moves, so a refusal writes nothing. A failed
    /// write is [`JournalError::OutcomeUnknown`]: a prefix of the batch may be on the
    /// disk, and a reopen repairs it as a torn tail.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::RecordJournal;
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-all-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let mut journal = RecordJournal::<u64>::open(&path)?;
    /// let acks = journal.compare_and_append_all(journal.tail(), &[1, 2, 3])?;
    /// assert_eq!(acks.len(), 3);
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Whatever [`RecordJournal::compare_and_append`] reports.
    pub fn compare_and_append_all(
        &mut self,
        expected_tail: JournalPosition,
        records: &[R],
    ) -> Result<Vec<DurableAck>, JournalError> {
        self.refuse_stale(expected_tail)?;
        bound_events(self.offsets.len(), records.len())?;
        let mut position = self.position;
        let mut frames = Vec::new();
        let mut staged = Vec::with_capacity(records.len());
        for record in records {
            let (next, framed) = frame_tagged(record, position)?;
            position = next;
            frames.extend_from_slice(&framed);
            self.bound_append(frames.len())?;
            staged.push((next, framed.len()));
        }
        if !frames.is_empty() {
            let expected = self.disk_len;
            self.storage
                .submit(move |file, _state| commit(file, expected, &frames))
                .map_err(write_refusal)?;
        }
        let mut acks = Vec::with_capacity(staged.len());
        for (next, length) in staged {
            acks.push(self.accept(next, length));
        }
        Ok(acks)
    }

    /// Whether the next append would take `record`, asked without writing it.
    ///
    /// A consumer that stages records and syncs them as a group asks this before
    /// it folds a record into its view, so a record the file would refuse never
    /// reaches the view. It runs the append's own checks, on the same path: the
    /// seal, the stale-handle fence, the event and byte ceilings, and the frame
    /// ceiling on the archived record. Nothing is written and
    /// [`RecordJournal::committed`] does not move.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::{JournalError, RecordJournal};
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-admits-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let journal = RecordJournal::<Vec<u8>>::open(&path)?;
    /// journal.admits(&vec![7; 16])?;
    /// let refused = journal.admits(&vec![7; 128 * 1024]);
    /// assert!(matches!(refused, Err(JournalError::CapacityExceeded { .. })));
    /// assert_eq!(journal.committed(), 0);
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// What [`RecordJournal::compare_and_append`] would report before writing:
    /// [`JournalError::Superseded`], [`JournalError::CapacityExceeded`],
    /// [`JournalError::Encoding`], and [`JournalError::Storage`] for a stale handle.
    pub fn admits(&self, record: &R) -> Result<(), JournalError> {
        self.prepare(self.position, record).map(|_| ())
    }

    /// Everything one append checks and frames before the disk is touched.
    fn prepare(
        &self,
        expected_tail: JournalPosition,
        record: &R,
    ) -> Result<(JournalPosition, Vec<u8>), JournalError> {
        self.refuse_stale(expected_tail)?;
        bound_events(self.offsets.len(), 1)?;
        let (position, frames) = frame_tagged(record, self.position)?;
        self.bound_append(frames.len())?;
        Ok((position, frames))
    }

    /// Refuse an append that would leave no room for the seal this handle owes.
    ///
    /// A continuing journal keeps one largest frame in hand, so appending up to the
    /// ceiling can never strand it with a history it is no longer able to seal.
    fn bound_append(&self, adding: usize) -> Result<(), JournalError> {
        let owed = match self.policy {
            Some(_) => framed_len(MAX_FRAME_BYTES),
            None => 0,
        };
        bound_bytes(self.disk_len.saturating_add(owed), adding)
    }

    /// Seal this journal with `checkpoint` and hand back the successor.
    ///
    /// `checkpoint` is what the consumer folded: everything a reader of the
    /// successor needs instead of the sealed history. `None` when this handle was
    /// not opened to continue. The predecessor stays readable and refuses appends.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::{ContinuationPolicy, RecordJournal, successor_path};
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-seal-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// # drop(std::fs::remove_file(successor_path(&path)));
    /// let mut journal = RecordJournal::<u64>::open_continuing(&path, ContinuationPolicy::declared())?;
    /// journal.compare_and_append(journal.tail(), &1)?;
    /// let successor = journal.continue_with(&100)?;
    /// assert_eq!(successor.map(|next| next.committed()), Some(1));
    /// # drop(journal);
    /// # drop(std::fs::remove_file(successor_path(&path)));
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`JournalError::Superseded`] when this handle already sealed,
    /// [`JournalError::CapacityExceeded`] when the checkpoint is over the frame
    /// ceiling or the chain has no generation left,
    /// [`JournalError::ContinuationPaused`] when an armed boundary stopped it, and
    /// [`JournalError::OutcomeUnknown`] when bytes may be on the disk with the reply
    /// lost.
    pub fn continue_with(&mut self, checkpoint: &R) -> Result<Option<Self>, JournalError> {
        let Some(policy) = self.policy else {
            return Ok(None);
        };
        self.refuse_stale(self.position)?;
        let successor = successor_path(&self.path);
        if successor == self.path {
            let generation = generation_of(&self.path);
            let refusal = Err(JournalError::CapacityExceeded {
                resource: JournalLimitKind::Generation,
                limit: generation,
                requested: generation.saturating_add(1),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "RecordJournal::continue_with: this chain has no generation left");
            return refusal;
        }
        let (position, framed) = frame_seal(checkpoint, self.position)?;
        bound_bytes(self.disk_len, framed.len())?;
        let (expected, pause, sealed_at) = (self.disk_len, self.pause.take(), successor.clone());
        let answer = self
            .storage
            .submit(move |file, _state| seal_on_owner(file, expected, &framed, &sealed_at, pause))
            .map_err(write_refusal)?;
        self.sealed_by = Some(successor.clone());
        match answer {
            SealOutcome::Sealed { .. } => {
                self.position = position;
                Self::open_impl(&successor, Some(policy)).map(Some)
            }
            SealOutcome::Paused(boundary) => {
                let refusal = Err(JournalError::ContinuationPaused { boundary });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "RecordJournal::continue_with: an armed boundary stopped the seal");
                refusal
            }
            SealOutcome::Appended => {
                let refusal = Err(JournalError::Storage(std::io::Error::other(
                    "the seal reported an append's answer; the journal's own steps disagree",
                )));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "RecordJournal::continue_with: the owner answered out of kind");
                refusal
            }
        }
    }
}

impl<R> RecordJournal<R> {
    /// The one precondition every write takes.
    fn refuse_stale(&self, expected_tail: JournalPosition) -> Result<(), JournalError> {
        if let Some(next) = self.sealed_by.as_ref() {
            let refusal = Err(JournalError::Superseded {
                path: next.display().to_string(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "RecordJournal: this handle sealed its journal");
            return refusal;
        }
        if expected_tail != self.position {
            let refusal = Err(JournalError::TailMismatch {
                expected: expected_tail,
                actual: self.position,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "RecordJournal: the caller's tail is stale");
            return refusal;
        }
        if self.storage.poisoned()
            || self.view.metadata().map_err(JournalError::Storage)?.len() != self.disk_len
        {
            let refusal = Err(JournalError::Storage(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the file may hold unacknowledged bytes; reopen to replay",
            )));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "RecordJournal: this handle is stale");
            return refusal;
        }
        Ok(())
    }

    /// Fold a committed frame into this handle's view and mint its acknowledgment.
    fn accept(&mut self, position: JournalPosition, length: usize) -> DurableAck {
        self.offsets.push(self.disk_len);
        self.disk_len = self.disk_len.saturating_add(u64::saturating_from(length));
        self.position = position;
        DurableAck::new(position, self.durability())
    }

    /// Arm the next continuation to stop at `boundary`.
    ///
    /// A fault injector, public for the reason
    /// [`FileJournal::arm_continuation_pause`](crate::journal::FileJournal::arm_continuation_pause)
    /// is: which file is authoritative after a kill at each boundary is a question
    /// about a real process.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::{ContinuationPolicy, JournalError, RecordJournal, SealPause};
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-pause-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let mut journal = RecordJournal::<u64>::open_continuing(&path, ContinuationPolicy::declared())?;
    /// journal.arm_seal_pause(SealPause::AfterSealWrite);
    /// let paused = journal.continue_with(&1);
    /// assert!(matches!(paused, Err(JournalError::ContinuationPaused { .. })));
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    pub fn arm_seal_pause(&mut self, boundary: SealPause) {
        self.pause = Some(boundary);
    }
}

/// One record framed after `from`.
fn frame_tagged<R: Durable>(
    record: &R,
    from: JournalPosition,
) -> Result<(JournalPosition, Vec<u8>), JournalError> {
    frame_payload(record, from, |bytes| {
        let mut payload = Vec::with_capacity(bytes.len().saturating_add(1));
        payload.push(RECORD_TAG);
        payload.extend_from_slice(bytes);
        payload
    })
}

/// A checkpoint framed as this chain's seal after `from`.
fn frame_seal<R: Durable>(
    record: &R,
    from: JournalPosition,
) -> Result<(JournalPosition, Vec<u8>), JournalError> {
    frame_payload(record, from, |bytes| {
        let mut payload = Vec::with_capacity(bytes.len().saturating_add(SEAL_HEADER));
        payload.push(SEAL_TAG);
        payload.extend_from_slice(&from.sequence().to_be_bytes());
        payload.extend_from_slice(from.head().as_bytes());
        payload.extend_from_slice(bytes);
        payload
    })
}

/// Archive, bound, chain and lay out one payload through the shared framing step.
fn frame_payload<R: Durable>(
    record: &R,
    from: JournalPosition,
    lay_out: impl FnOnce(&[u8]) -> Vec<u8>,
) -> Result<(JournalPosition, Vec<u8>), JournalError> {
    let sequence = from
        .sequence()
        .checked_add(1)
        .ok_or(JournalError::Exhausted)?;
    let previous = from.head();
    let (framed, head) = frame_record(
        record,
        &previous,
        MAX_FRAME_BYTES,
        |held: &R| {
            held.to_record()
                .map(|bytes| lay_out(&bytes))
                .map_err(JournalError::Encoding)
        },
        |_held, chained, payload| chain_over_bytes(chained, payload),
        |length| JournalError::CapacityExceeded {
            resource: JournalLimitKind::FrameBytes,
            limit: u64::saturating_from(MAX_FRAME_BYTES),
            requested: u64::saturating_from(length),
        },
    )?;
    Ok((JournalPosition { sequence, head }, framed))
}
