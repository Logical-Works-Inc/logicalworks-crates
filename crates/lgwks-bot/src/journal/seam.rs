//! The seam every journal adapter implements: what a caller may ask of a run's record.

#[cfg(feature = "wire")]
use crate::BoxFuture;
use crate::effect::EffectKey;

use super::frame::SaturatingFrom;
use super::{
    ContinuationWatermark, DurabilityPromise, DurableAck, EffectEvent, EffectEvidence,
    JournalEntry, JournalError, JournalPosition, MAX_JOURNAL_BYTES, MAX_JOURNAL_EVENTS,
};

/// An append-only record of a run's effects.
///
/// Implementors are adapters over a host's durable store. The trait is the
/// seam; the durability grade is what a caller checks before it uses one.
///
/// # Examples
///
/// ```
/// use lgwks_bot::journal::{DurabilityPromise, EffectJournal, JournalPosition, MemoryJournal};
///
/// let journal = MemoryJournal::new();
/// assert_eq!(journal.durability(), DurabilityPromise::Ephemeral);
/// assert_eq!(journal.tail(), JournalPosition::genesis());
/// ```
pub trait EffectJournal {
    /// What this journal promises about an acknowledged append.
    fn durability(&self) -> DurabilityPromise;

    /// The last committed position, or the genesis when nothing is committed.
    fn tail(&self) -> JournalPosition;

    /// Every committed event, in commit order.
    ///
    /// Required rather than optional, and read rather than streamed, because
    /// this is what `recover` consumes: a controller that comes back after a
    /// crash learns what was in flight by replaying its own journal, and a
    /// journal that cannot be read back cannot be the record behind an external
    /// handoff. An adapter over a store that can only be appended to is a
    /// *sink*, not a journal, and this trait is not a sink.
    ///
    /// The whole sequence rather than a page, because `recover` folds over it
    /// and a partial replay answers "what is uncertain" with a subset — which
    /// reads as "nothing is uncertain" for every attempt in the part that was
    /// not read.
    ///
    /// Shipped adapters enforce [`MAX_JOURNAL_EVENTS`] and
    /// [`MAX_JOURNAL_BYTES`]. They refuse new history at the cap instead of
    /// deleting old or unresolved events. External adapters remain responsible
    /// for bounding their own backing store and returned vector.
    ///
    /// # Errors
    ///
    /// [`JournalError::Storage`] when the backing store refused to be read.
    fn committed(&self) -> Result<Vec<EffectEvent>, JournalError>;

    /// Every committed entry, including the position each event occupies.
    ///
    /// An acknowledgement is meaningful only when positioned readback proves
    /// it names the exact appended fact. A later tail can belong to another
    /// controller and must never become this controller's append fence.
    ///
    /// Adapters that cannot provide positioned readback must refuse dispatch,
    /// including local effects, rather than authorize an unbound append.
    fn committed_entries(&self) -> Result<Vec<JournalEntry>, JournalError> {
        Err(JournalError::ReceiptUnavailable {
            required: self.durability(),
        })
    }

    /// Read one position-bound entry without returning the whole committed
    /// history.
    ///
    /// The default preserves correctness for external adapters while providing
    /// a migration path; shipped journals override it with sequence-indexed
    /// access. Callers still compare the returned event with the exact event
    /// they acknowledged, because position alone is not event identity.
    fn committed_entry(
        &self,
        position: JournalPosition,
    ) -> Result<Option<JournalEntry>, JournalError> {
        Ok(self
            .committed_entries()?
            .into_iter()
            .find(|entry| entry.position() == position))
    }

    /// Find the latest committed outcome for one attempt, with its position.
    ///
    /// This default is a compatibility path for external adapters. Shipped
    /// journals maintain a keyed index so repeated reconciliation does not
    /// clone and scan all historical events.
    fn outcome_at(
        &self,
        key: EffectKey,
    ) -> Result<Option<(JournalPosition, EffectEvidence)>, JournalError> {
        Ok(self
            .committed_entries()?
            .into_iter()
            .rev()
            .find_map(|entry| match *entry.event() {
                EffectEvent::OutcomeObserved {
                    key: held,
                    evidence,
                } if held == key => Some((entry.position(), evidence)),
                _ => None,
            }))
    }

    /// Append `event` if and only if `expected_tail` is still the committed
    /// tail.
    ///
    /// The `&mut self` receiver fences writers within one process; the tail
    /// check fences writers across processes, which is the case that actually
    /// happens.
    ///
    /// The outcome is one of exactly three things, and an adapter must say
    /// which:
    ///
    /// - `Ok(ack)` — the event is committed at `ack.position` under
    ///   `ack.promise`.
    /// - `Err` of [`JournalError::TailMismatch`], [`JournalError::OutOfOrder`],
    ///   [`JournalError::Exhausted`], [`JournalError::Encoding`] or
    ///   [`JournalError::Storage`] — the event is *not* committed and the
    ///   journal is unchanged. A retry may re-append.
    /// - `Err(JournalError::OutcomeUnknown)` — the event *may* be committed
    ///   and the acknowledgment was lost. A caller reconciles by readback and
    ///   must not re-append: re-appending an event that did land is how an
    ///   effect gets executed twice on the strength of a lost reply.
    ///
    /// # Errors
    ///
    /// [`JournalError::TailMismatch`] when another writer appended,
    /// [`JournalError::OutOfOrder`] when the event cannot follow what is
    /// committed for its key, [`JournalError::Exhausted`] when the position
    /// space is spent, [`JournalError::Storage`] when the backing store
    /// refused before the event was written, and
    /// [`JournalError::OutcomeUnknown`] when the event may have committed and
    /// the acknowledgment was lost.
    #[cfg(feature = "wire")]
    fn compare_and_append(
        &mut self,
        expected_tail: JournalPosition,
        event: &EffectEvent,
    ) -> Result<DurableAck, JournalError>;

    /// The same append, for a caller that awaits rather than blocks.
    ///
    /// One contract with two doors, and the reason is the storage device's
    /// cost. A store whose durability path blocks — a `fsync` on the caller's
    /// own thread — charges that latency to everything else sharing the
    /// executor, and awaiting the surrounding tick does not make the call
    /// inside it non-blocking. An adapter that owns its storage on a separate
    /// thread overrides this and returns the same acknowledgment, from the
    /// same fence, over the same bytes.
    ///
    /// The default reaches the synchronous form, which is correct for a store
    /// with no blocking durability path. It is a default rather than a
    /// requirement so an adapter is not asked to reimplement the fence; what
    /// it must not do is answer with a weaker write than its synchronous form
    /// would have made.
    ///
    /// Dropping the returned future is not a cancellation of the append. An
    /// adapter whose append can outlive its caller has to leave the handle in
    /// a state that refuses the next append rather than continue from a view
    /// that may be behind the store.
    ///
    /// # Errors
    ///
    /// Every [`JournalError`] [`Self::compare_and_append`] produces, from the
    /// same checks in the same order.
    #[cfg(feature = "wire")]
    fn compare_and_append_async<'a>(
        &'a mut self,
        expected_tail: JournalPosition,
        event: &'a EffectEvent,
    ) -> BoxFuture<'a, Result<DurableAck, JournalError>> {
        Box::pin(async move { self.compare_and_append(expected_tail, event) })
    }

    /// Append `events` in order at the journal's own tail, as one batch where
    /// the adapter can make it one.
    ///
    /// The default appends rung by rung, so a refusal part way leaves the rungs
    /// before it recorded. An adapter that can frame the batch into one write
    /// and one flush overrides it: the file adapter does, so a batch costs one
    /// sync rather than one per rung, and is all or nothing.
    ///
    /// # Errors
    ///
    /// Every [`JournalError`] [`Self::compare_and_append`] produces.
    #[cfg(feature = "wire")]
    fn append_batch(&mut self, events: &[EffectEvent]) -> Result<Vec<DurableAck>, JournalError> {
        let mut acks = Vec::with_capacity(events.len());
        for event in events {
            let tail = self.tail();
            acks.push(self.compare_and_append(tail, event)?);
        }
        Ok(acks)
    }

    /// Confirm that an existing outcome record now meets `required`.
    ///
    /// An append can return a weak acknowledgment after it has already moved
    /// the per-key ladder to `OutcomeObserved`. A retry cannot append that
    /// outcome again, so an adapter that can flush, replicate, or otherwise
    /// obtain a stronger receipt implements this method. The default refuses
    /// rather than treating read-back of event bytes as a durability proof.
    ///
    /// # Errors
    ///
    /// [`JournalError::ReceiptUnavailable`] when the adapter has no receipt
    /// operation, or another journal error when it cannot obtain one.
    fn confirm_outcome(
        &mut self,
        _key: EffectKey,
        _evidence: EffectEvidence,
        _position: JournalPosition,
        required: DurabilityPromise,
    ) -> Result<DurableAck, JournalError> {
        Err(JournalError::ReceiptUnavailable { required })
    }

    /// Whether this journal may host an effect that has left the process.
    ///
    /// A journal that cannot survive its own writer dying must never be the
    /// record behind an external handoff, because after a crash it reports an
    /// empty run rather than an uncertain one.
    ///
    /// # Errors
    ///
    /// [`JournalError::PromiseUnmet`] when the journal is ephemeral.
    fn admit_external_handoff(&self) -> Result<DurabilityPromise, JournalError> {
        let offered = self.durability();
        if offered.survives_process_crash() {
            return Ok(offered);
        }
        Err(JournalError::PromiseUnmet {
            required: DurabilityPromise::ProcessCrash,
            offered,
        })
    }

    /// Whether this journal can admit `rungs` more events for an attempt it is
    /// about to hand off, decided *before* any of them is written.
    ///
    /// A handoff records at least three rungs — `IntentAdmitted`,
    /// `DispatchPrepared` and the settling `OutcomeObserved` — and the last one
    /// is written only after the effect has left the process. A journal that
    /// admits the first two and then refuses the third leaves an attempt that
    /// can never be settled without deleting unresolved evidence: the exact
    /// availability loss the hard ceiling must not cause. A bounded journal
    /// answers this by checking the whole attempt fits, so a refusal happens
    /// before the handoff rather than after it.
    ///
    /// The default is permissive, so external adapters keep their current
    /// behaviour; a bounded adapter overrides it. The check is a conservative
    /// admission test, not a held reservation: it is made on the serialized
    /// append path, where the count it reads cannot move between the check and
    /// the writes it authorizes.
    ///
    /// # Errors
    ///
    /// [`JournalError::CapacityExceeded`] when the attempt does not fit.
    fn reserve_handoff_capacity(&self, _rungs: u64) -> Result<(), JournalError> {
        Ok(())
    }

    /// How much of each ceiling this journal has used, and whether it continues
    /// past them at all.
    ///
    /// The question a caller asks before an append, and it is asked on the shipped
    /// append path rather than by a caller a test wrote, because a lifecycle
    /// policy nobody consults is a policy that does not exist (issue #267).
    ///
    /// The default measures the journal honestly and reports `continues() ==
    /// false`, which leaves every adapter's behaviour exactly as it was: the
    /// default is what the in-memory adapter and every external adapter want,
    /// and a `FileJournal` opened for an unattended run is the one that answers
    /// otherwise.
    ///
    /// # Errors
    ///
    /// [`JournalError::Storage`] when the store cannot be read for the counts.
    fn continuation_watermark(&self) -> Result<ContinuationWatermark, JournalError> {
        Ok(ContinuationWatermark::inert(
            0,
            u64::saturating_from(MAX_JOURNAL_EVENTS),
            0,
            MAX_JOURNAL_BYTES,
        ))
    }

    /// Seal this journal at the watermark and hand back the successor opened from
    /// the checkpoint this writes.
    ///
    /// `None` when this journal does not continue, which is the default and the
    /// answer every adapter but the continuing file journal gives. A `Some` hands
    /// the caller the authoritative journal: the predecessor is sealed, read-only
    /// and still readable, and every append from here lands in the successor.
    ///
    /// The caller must adopt the returned journal whole — its tail, its ladder and
    /// its carried state — rather than keeping the predecessor's own position,
    /// because the successor's chain starts at the predecessor's tail and its
    /// first frame is the seal frame. An append that kept the predecessor's fence
    /// would be refused as a stale tail, which is the correct answer to a
    /// question that must not be asked.
    ///
    /// # Errors
    ///
    /// sealed-handle refusals: [`JournalError::Superseded`] when
    /// this journal is already sealed, [`JournalError::CapacityExceeded`] when
    /// the carried state is past a declared bound, and the device's own errors
    /// through [`JournalError::OutcomeUnknown`] when the bytes may be on the disk
    /// under no acknowledgment.
    fn continue_as_new(&mut self) -> Result<Option<Box<dyn EffectJournal>>, JournalError> {
        Ok(None)
    }
}
