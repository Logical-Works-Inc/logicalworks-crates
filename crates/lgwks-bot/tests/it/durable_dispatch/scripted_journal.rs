//! One journal over a shared store, with each fault the durable-dispatch tests
//! inject as a named knob.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lgwks_bot::effect::{AttemptId, EffectKey};
use lgwks_bot::journal::{
    DurabilityPromise, DurableAck, EffectEvent, EffectJournal, JournalEntry, JournalError,
    JournalPosition, MemoryJournal,
};
use lgwks_bot::spec::EffectEvidence;

/// One journal over a shared store, with each fault this file injects as a
/// named knob.
///
/// Every adapter the tests need is the same delegation to a `MemoryJournal`
/// plus one deviation. One type with the deviations named means a test cannot
/// pass against a delegation that quietly differs from the one its neighbour
/// tested. [`ScriptedJournal::over`] is the plain delegation, and each knob
/// changes one answer and nothing else.
pub(super) struct ScriptedJournal {
    /// The store every read and write goes through.
    store: Rc<RefCell<MemoryJournal>>,
    /// What `durability`, and so `admit_external_handoff`, advertises; the
    /// store's own grade when `None`.
    advertised: Option<DurabilityPromise>,
    /// The promise every append acknowledges at the new tail, in place of the
    /// store's own acknowledgment. A store that advertises one grade and acks
    /// another is weaker than it says, and the handoff is checked against the
    /// acknowledgment (issue #100).
    ack: Option<DurabilityPromise>,
    /// Whether `committed_entries` reads the store. When it does not, it
    /// refuses as the trait's default does, like an adapter with no positioned
    /// read.
    entries: bool,
    /// One event the next `tail` read commits before it answers: another
    /// controller committing after recovery read the history and before it
    /// adopted the tail, an interleaving an append fence alone cannot observe.
    inject_on_tail: RefCell<Option<EffectEvent>>,
    /// When set, the first append also commits a foreign unknown handoff if it
    /// admits an intent, and acknowledges at that later position: a hostile
    /// adapter returning someone else's tail as the caller's receipt.
    forge_once: Cell<bool>,
    /// When set, `committed` and `committed_entries` fail with `Storage`: the
    /// issue #123 transient read failure, with the outcome in the store and the
    /// write path healthy.
    fail_reads: Rc<Cell<bool>>,
    /// How outcome receipts are answered; the trait's refusal when `None`.
    receipts: Option<Receipts>,
}

/// The outcome-receipt knobs of a [`ScriptedJournal`]: a receipt for either the
/// named outcome or an unrelated position, to exercise receipt identity
/// verification.
pub(super) struct Receipts {
    /// The grade every `confirm_outcome` receipt reports.
    pub(super) receipt: Rc<Cell<DurabilityPromise>>,
    /// When set, a receipt names an unrelated position.
    pub(super) unrelated_receipt: Rc<Cell<bool>>,
    /// When set, an outcome's append acknowledgment names an unrelated position.
    pub(super) unrelated_append_ack: Rc<Cell<bool>>,
}

impl Receipts {
    /// The acknowledgment an append of `event` reports, given the store's own.
    ///
    /// An outcome is acked `Ephemeral` and everything else `ProcessCrash`, so
    /// only a receipt can lift an outcome to the grade a handoff needs.
    fn append_ack(&self, event: &EffectEvent, acknowledgment: DurableAck) -> DurableAck {
        let outcome = matches!(event, EffectEvent::OutcomeObserved { .. });
        if outcome && self.unrelated_append_ack.get() {
            return DurableAck::new(JournalPosition::genesis(), DurabilityPromise::ProcessCrash);
        }
        let promise = if outcome {
            DurabilityPromise::Ephemeral
        } else {
            DurabilityPromise::ProcessCrash
        };
        DurableAck::new(acknowledgment.position(), promise)
    }
}

impl ScriptedJournal {
    /// The plain delegation to `store`, with every knob off.
    pub(super) fn over(store: Rc<RefCell<MemoryJournal>>) -> Self {
        Self {
            store,
            advertised: None,
            ack: None,
            entries: false,
            inject_on_tail: RefCell::new(None),
            forge_once: Cell::new(false),
            fail_reads: Rc::new(Cell::new(false)),
            receipts: None,
        }
    }

    /// Advertise `promise` instead of the store's grade.
    pub(super) fn advertising(mut self, promise: DurabilityPromise) -> Self {
        self.advertised = Some(promise);
        self
    }

    /// Acknowledge every append as `promise`.
    pub(super) fn acking(mut self, promise: DurabilityPromise) -> Self {
        self.ack = Some(promise);
        self
    }

    /// Answer `committed_entries` from the store.
    pub(super) fn with_entries(mut self) -> Self {
        self.entries = true;
        self
    }

    /// Commit `event` on the next `tail` read.
    pub(super) fn injecting(mut self, event: EffectEvent) -> Self {
        self.inject_on_tail = RefCell::new(Some(event));
        self
    }

    /// Forge a foreign handoff behind the first admitted intent.
    pub(super) fn forging(mut self) -> Self {
        self.forge_once = Cell::new(true);
        self
    }

    /// Fail every read while `fail_reads` is set.
    pub(super) fn failing_reads(mut self, fail_reads: Rc<Cell<bool>>) -> Self {
        self.fail_reads = fail_reads;
        self
    }

    /// Answer outcome receipts with `receipts`.
    pub(super) fn receipting(mut self, receipts: Receipts) -> Self {
        self.receipts = Some(receipts);
        self
    }

    /// `Storage` while reads are failing, logged where it is made.
    fn readable(&self) -> Result<(), JournalError> {
        if self.fail_reads.get() {
            let refusal = Err(JournalError::Storage(std::io::Error::other(
                "transient read failure",
            )));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "readable: returning an error to the caller");
            return refusal;
        }
        Ok(())
    }
}

impl EffectJournal for ScriptedJournal {
    fn durability(&self) -> DurabilityPromise {
        match self.advertised {
            Some(advertised) => advertised,
            None => self.store.borrow().durability(),
        }
    }

    fn tail(&self) -> JournalPosition {
        if let Some(event) = self.inject_on_tail.borrow_mut().take() {
            let mut store = self.store.borrow_mut();
            let expected = store.tail();
            if store.compare_and_append(expected, &event).is_err() {
                return expected;
            }
        }
        self.store.borrow().tail()
    }

    fn committed(&self) -> Result<Vec<EffectEvent>, JournalError> {
        self.readable()?;
        EffectJournal::committed(&*self.store.borrow())
    }

    fn committed_entries(&self) -> Result<Vec<JournalEntry>, JournalError> {
        self.readable()?;
        if self.entries {
            Ok(self.store.borrow().committed().to_vec())
        } else {
            Err(JournalError::ReceiptUnavailable {
                required: self.durability(),
            })
        }
    }

    fn compare_and_append(
        &mut self,
        expected_tail: JournalPosition,
        event: &EffectEvent,
    ) -> Result<DurableAck, JournalError> {
        let acknowledgment = self
            .store
            .borrow_mut()
            .compare_and_append(expected_tail, event)?;
        if self.forge_once.replace(false) && matches!(event, EffectEvent::IntentAdmitted { .. }) {
            let attempt = attempt_two()?;
            let foreign_key = event.key().with_attempt(attempt);
            let mut store = self.store.borrow_mut();
            let tail = store.tail();
            store.compare_and_append(tail, &EffectEvent::IntentAdmitted { key: foreign_key })?;
            let tail = store.tail();
            store.compare_and_append(tail, &EffectEvent::DispatchPrepared { key: foreign_key })?;
            return Ok(DurableAck::new(store.tail(), acknowledgment.promise()));
        }
        if let Some(receipts) = self.receipts.as_ref() {
            return Ok(receipts.append_ack(event, acknowledgment));
        }
        Ok(match self.ack {
            Some(promise) => DurableAck::new(self.store.borrow().tail(), promise),
            None => acknowledgment,
        })
    }

    fn confirm_outcome(
        &mut self,
        _key: EffectKey,
        _evidence: EffectEvidence,
        position: JournalPosition,
        required: DurabilityPromise,
    ) -> Result<DurableAck, JournalError> {
        match self.receipts.as_ref() {
            Some(receipts) => Ok(DurableAck::new(
                if receipts.unrelated_receipt.get() {
                    JournalPosition::genesis()
                } else {
                    position
                },
                receipts.receipt.get(),
            )),
            None => Err(JournalError::ReceiptUnavailable { required }),
        }
    }
}

/// The forged attempt id the [`ScriptedJournal::forging`] knob admits under.
///
/// A helper so the forged append's one fallible step has a named refusal, the
/// way every other refusal in the crate does.
fn attempt_two() -> Result<AttemptId, JournalError> {
    // "2" is a literal in this fixture, so the parse cannot fail; the arm exists
    // because the type demands one and `JournalError` has nothing to add that
    // the parse error does not already say.
    match AttemptId::from_decimal("2") {
        Ok(attempt) => Ok(attempt),
        Err(_not_an_attempt) => Err(JournalError::Exhausted),
    }
}
