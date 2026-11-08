//! What an attempt became: settling, confirming and recording its outcome.

use super::{
    DurabilityPromise, DurableAck, EffectEvent, EffectEvidence, EffectKey, Effects, EventKind,
    JournalError, JournalPosition, changed_outcome,
};

impl Effects {
    /// Settle one recovered attempt, appending the outcome that settles it.
    ///
    /// The append lands first: the journal is the record, and a bot that
    /// dropped the key from its own list and then crashed before writing the
    /// outcome would come back with the same unknown it just cleared.
    ///
    /// # Errors
    ///
    /// Whatever the journal refuses.
    pub(in crate::ecs) fn settle_recovered(
        &mut self,
        key: EffectKey,
        evidence: EffectEvidence,
    ) -> Result<(), JournalError> {
        self.ensure_outcome(key, evidence)?;
        self.unsettled.retain(|held| *held != key);
        Ok(())
    }

    /// Upgrade a recovered outcome receipt without appending or re-entering
    /// the action that produced it.
    pub(in crate::ecs) fn settle_recording(
        &mut self,
        key: EffectKey,
        evidence: EffectEvidence,
    ) -> Result<(), JournalError> {
        self.confirm_recorded_outcome(key, evidence)?;
        self.recording.retain(|entry| entry.key != key);
        self.fold_outcome(key, evidence);
        Ok(())
    }

    /// Return the recorded position only when it holds the exact outcome.
    pub(in crate::ecs) fn outcome_position(
        &self,
        key: EffectKey,
        evidence: EffectEvidence,
    ) -> Result<JournalPosition, JournalError> {
        let event = EffectEvent::OutcomeObserved { key, evidence };
        let Some((position, recorded)) = self.scope.journal().outcome_at(key)? else {
            let refusal = Err(JournalError::OutOfOrder {
                key: Box::new(key),
                expected: Some(EventKind::OutcomeObserved),
                attempted: EventKind::OutcomeObserved,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "outcome_position: returning an error to the caller");
            return refusal;
        };
        if recorded != evidence {
            let refusal = Err(changed_outcome(key));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "outcome_position: the journal holds a different outcome");
            return refusal;
        }
        self.event_position(&event, position)?;
        Ok(position)
    }

    /// Require the acknowledged position to hold the exact event.
    ///
    /// A tail equality check alone proves only that *something* occupies the
    /// acknowledged position. Binding the event prevents an adapter from
    /// laundering a concurrent writer's tail into this controller's fold.
    pub(in crate::ecs) fn event_position(
        &self,
        expected: &EffectEvent,
        position: JournalPosition,
    ) -> Result<(), JournalError> {
        let Some(entry) = self.scope.journal().committed_entry(position)? else {
            let refusal = Err(JournalError::OutOfOrder {
                key: Box::new(expected.key()),
                expected: Some(expected.kind()),
                attempted: expected.kind(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "event_position: returning an error to the caller");
            return refusal;
        };
        if entry.event() != expected {
            let refusal = Err(JournalError::EntryMismatch {
                position,
                expected: Box::new(*expected),
                actual: Box::new(*entry.event()),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "event_position: returning an error to the caller");
            return refusal;
        }
        Ok(())
    }

    /// The outcome already committed for `key`, when there is one.
    ///
    /// The durable half of idempotent settlement: a repeat after reconstruction
    /// has no live `AttemptRecord` to consult, and answering `NoSuchWork` there
    /// is how the same evidence stops being a safe retry across a restart.
    ///
    /// # Errors
    ///
    /// Whatever the journal refuses when read. An unreadable journal is not an
    /// absent outcome: collapsing the two is how a transient storage failure
    /// becomes a terminal "this work never existed" (issue #123).
    pub(in crate::ecs) fn outcome_for(
        &self,
        key: &EffectKey,
    ) -> Result<Option<EffectEvidence>, JournalError> {
        Ok(self
            .scope
            .journal()
            .outcome_at(*key)?
            .map(|(_, evidence)| evidence))
    }

    /// Require the receipt for an outcome to meet the grade admitted for its
    /// attempt. A weak append can have occupied the ladder already; the
    /// journal's explicit receipt operation is the only safe retry in that
    /// case.
    ///
    /// The receipt binds to the keyed outcome, not to whatever the
    /// acknowledgment names: an ack for a position that holds no outcome for
    /// this key is a [`JournalError::ReceiptMismatch`], never an acceptance.
    pub(in crate::ecs) fn confirm_outcome(
        &mut self,
        key: EffectKey,
        evidence: EffectEvidence,
        required: DurabilityPromise,
        acknowledgment: DurableAck,
    ) -> Result<(), JournalError> {
        let position = self.outcome_position(key, evidence)?;
        if acknowledgment.position() != position {
            let refusal = Err(JournalError::ReceiptMismatch {
                expected: position,
                actual: acknowledgment.position(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "confirm_outcome: returning an error to the caller");
            return refusal;
        }
        if acknowledgment.promise().meets(required) {
            return Ok(());
        }
        self.obtain_receipt(key, evidence, position, required)
    }

    /// Ask the journal for its own receipt for the outcome at `position` and
    /// require it to name that position at the grade `required`.
    ///
    /// The one receipt check both [`Self::confirm_outcome`] (a weak append whose
    /// acknowledgment fell short) and [`Self::confirm_recorded_outcome`] (recovery
    /// of an outcome whose acknowledgment is gone) end in, so the two cannot
    /// disagree about what a receipt must prove.
    pub(in crate::ecs) fn obtain_receipt(
        &mut self,
        key: EffectKey,
        evidence: EffectEvidence,
        position: JournalPosition,
        required: DurabilityPromise,
    ) -> Result<(), JournalError> {
        let receipt = self
            .scope
            .journal_mut()
            .confirm_outcome(key, evidence, position, required)?;
        if receipt.position() != position {
            let refusal = Err(JournalError::ReceiptMismatch {
                expected: position,
                actual: receipt.position(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "obtain_receipt: the receipt names another position");
            return refusal;
        }
        if receipt.promise().meets(required) {
            Ok(())
        } else {
            Err(JournalError::PromiseUnmet {
                required,
                offered: receipt.promise(),
            })
        }
    }

    /// Fold a receipted outcome into the local state.
    pub(in crate::ecs) fn fold_outcome(&mut self, key: EffectKey, evidence: EffectEvidence) {
        self.requirements.retain(|entry| entry.0 != key);
        self.note_journal_attempt(key.action(), key.attempt());
        if evidence == EffectEvidence::Applied {
            self.note_applied(key);
        }
    }

    /// Verify a previously committed outcome before recovery treats it as
    /// settled. A durable adapter must provide positioned readback; otherwise
    /// the action remains blocked rather than being re-entered.
    ///
    /// An ephemeral journal skips the receipt operation: readback is the
    /// strongest proof its grade can offer, and the default
    /// [`EffectJournal::confirm_outcome`] refuses exactly because readback
    /// bytes prove nothing about survival. For a journal that promises to
    /// survive anything, the record must be attested, not merely present.
    pub(in crate::ecs) fn confirm_recorded_outcome(
        &mut self,
        key: EffectKey,
        evidence: EffectEvidence,
    ) -> Result<(), JournalError> {
        let required = self.scope.journal().durability();
        if required == DurabilityPromise::Ephemeral {
            return Ok(());
        }
        let position = self.outcome_position(key, evidence)?;
        self.obtain_receipt(key, evidence, position, required)
    }

    /// Settle an outcome the journal already holds.
    ///
    /// The committed record is the fact; what is left is the receipt the
    /// attempt was admitted under. The journal's own grade must meet it — a
    /// weaker store is refused rather than settled below the grade the
    /// dispatch was admitted with — and a store that grades itself above
    /// ephemeral must re-attest through its receipt operation, because
    /// readback proves only that bytes are present, not that they will
    /// survive what the grade promises.
    ///
    /// # Errors
    ///
    /// [`JournalError::ReceiptUnavailable`] when the journal's grade is below
    /// `required` or it has no receipt operation, and whatever the readback or
    /// the receipt operation refuses.
    pub(in crate::ecs) fn settle_committed_outcome(
        &mut self,
        key: EffectKey,
        evidence: EffectEvidence,
        required: DurabilityPromise,
    ) -> Result<(), JournalError> {
        let offered = self.scope.journal().durability();
        if !offered.meets(required) {
            let refusal = Err(JournalError::ReceiptUnavailable { required });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "settle_committed_outcome: returning an error to the caller");
            return refusal;
        }
        if offered != DurabilityPromise::Ephemeral {
            self.confirm_recorded_outcome(key, evidence)?;
        }
        self.accept_position(
            &EffectEvent::OutcomeObserved { key, evidence },
            self.outcome_position(key, evidence)?,
        )?;
        self.fold_outcome(key, evidence);
        Ok(())
    }

    /// Record an outcome for `key` if one is not already committed, and fold
    /// it into the in-memory applied set.
    ///
    /// Idempotent: a repeat of the same evidence does not append twice. The
    /// ladder's next step after `OutcomeObserved` is `Verified`, so an
    /// `OutOfOrder` that expected `Verified` (or nothing) means this exact
    /// record is already in the journal and the call is a success. This is the
    /// one settlement write, shared by the live and recovered paths so a
    /// restart cannot resurrect an attempt a live resolve already settled.
    ///
    /// # Errors
    ///
    /// Whatever the journal refuses, other than the idempotent repeat above.
    pub(in crate::ecs) fn ensure_outcome(
        &mut self,
        key: EffectKey,
        evidence: EffectEvidence,
    ) -> Result<(), JournalError> {
        // A key with no requirement of its own is held to the journal's grade:
        // that is the promise every other key in the scope is held to.
        let required = match self.requirements.iter().find(|entry| entry.0 == key) {
            Some(&(_, declared)) => declared,
            None => self.scope.journal().durability(),
        };
        if let Some(recorded) = self.outcome_for(&key)? {
            if recorded != evidence {
                let refusal = Err(changed_outcome(key));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "ensure_outcome: the journal holds a different outcome");
                return refusal;
            }
            // The record is already in the journal. The only open question is
            // the receipt: the original append's acknowledgment is not in hand
            // on this path, so the journal's own receipt operation is what
            // settles a graded attempt (issue #118).
            return self.settle_committed_outcome(key, evidence, required);
        }
        match self.append(&EffectEvent::OutcomeObserved { key, evidence }) {
            Ok(acknowledgment) => self.record_new_outcome(key, evidence, required, acknowledgment),
            Err(JournalError::OutOfOrder { expected, .. })
                if expected == Some(EventKind::Verified) || expected.is_none() =>
            {
                // Idempotent success only when the *same* evidence is already
                // recorded. A ladder-complete `OutOfOrder` alone is not proof of
                // which outcome landed, and treating it as success is how a
                // contradictory `Applied` gets acknowledged (issue #106).
                match self.outcome_for(&key)? {
                    Some(recorded) if recorded == evidence => {
                        // The original append acknowledgment is unavailable
                        // after an out-of-order retry. The journal's receipt
                        // operation, not a minted replacement, is what
                        // settles a graded attempt.
                        self.settle_committed_outcome(key, evidence, required)
                    }
                    _ => Err(JournalError::OutOfOrder {
                        key: Box::new(key),
                        expected,
                        attempted: EventKind::OutcomeObserved,
                    }),
                }
            }
            Err(cause @ JournalError::OutcomeUnknown { .. }) => {
                // The store may hold the event; the reply was lost. Read back
                // before anything else: a matching record means the effect and
                // its record both landed, and the attempt settles instead of
                // reporting an ambiguity the next retry can never resolve.
                match self.outcome_for(&key)? {
                    Some(recorded) if recorded == evidence => {
                        self.settle_committed_outcome(key, evidence, required)
                    }
                    Some(_) => Err(JournalError::OutOfOrder {
                        key: Box::new(key),
                        expected: Some(EventKind::Verified),
                        attempted: EventKind::OutcomeObserved,
                    }),
                    // Nothing landed. The ambiguous failure is the honest
                    // report: the caller is post-effect and must surface
                    // `EffectUnrecorded` with `Occurred` certainty, and a
                    // recovery pass reconciles from the journal's own replay.
                    None => Err(cause),
                }
            }
            Err(cause) => {
                // A refusal that left the journal unchanged. Surface it now:
                // the caller is post-effect and must report `EffectUnrecorded`,
                // not a clean settle.
                Err(cause)
            }
        }
    }
}
