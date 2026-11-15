//! What the ledger still owes, reported: the unresolved entries, the first of
//! them, and how many there are.

use super::Ledger;
use crate::ecs::work::{PendingWork, TransitionHold, WorkId};

impl Ledger {
    /// Every entry that is still unresolved, in `(chain, entry)` order.
    ///
    /// One fold, and every report that used to disagree is rendered from it:
    /// [`Self::first_unresolved`] is the first item, [`Self::pending`] is the
    /// whole list, and [`Self::unresolved_count`] is its length.
    ///
    /// "Unresolved" is open *or* abandoned, and the second half is load-bearing.
    /// An abandoned entry asks nothing of the substrate — it will not be
    /// attempted again and no evidence is owed by the tick — so a scan that
    /// looked only for open entries walked straight past it and reported a clean
    /// tick. But abandoned work is not handled work: [`Self::pending`] names it,
    /// and a caller that read `Ok` as "the chain is done" was reading the
    /// opposite of what the ledger held. It is also a prerequisite that is *not*
    /// satisfied, so the entries behind it are unresolved through it.
    ///
    /// A recovered attempt nothing has settled is reported for its declared
    /// work whether or not a transition exists: the barrier is durable and does
    /// not wait for a poll, so a bot assembled against a journal that already
    /// says the bytes may be live is stuck before its first observation (issue
    /// #104). It is reported *instead of* the entry's own hold, because two
    /// reports for one entry would read as two problems — and it is why the
    /// three reports now agree: a skipped unknown used to produce `Ok` from
    /// `tick` while `pending` still named it.
    pub(in crate::ecs) fn unresolved(&self, budget: u32) -> Vec<PendingWork> {
        let mut unresolved = Vec::new();
        for chain in 0..self.actions.len().max(self.transitions.len()) {
            // The declared actions and the live transition can disagree in
            // width: a world mutated behind the builder's back is a real
            // state, and a scan that trusted only one of the two walked past
            // work the other one holds.
            let declared = match self.actions.get(chain) {
                Some(actions) => actions.len(),
                None => 0,
            };
            let held = match self.transitions.get(chain).and_then(Option::as_ref) {
                Some(transition) => transition.entries.len(),
                None => 0,
            };
            for entry in 0..declared.max(held) {
                if let Some(work) = self.unresolved_entry(WorkId { chain, entry }, budget) {
                    unresolved.push(work);
                }
            }
        }
        unresolved
    }

    /// What one entry still owes, or `None` when it owes nothing.
    ///
    /// A recovered unsettled attempt first, then a failed recording, then the
    /// entry's own open or abandoned hold: one report per entry, in that order.
    fn unresolved_entry(&self, id: WorkId, budget: u32) -> Option<PendingWork> {
        if let Some(key) = self
            .action_of(id)
            .and_then(|action| self.effects.unsettled_for(action))
        {
            let action = key.action();
            return Some(PendingWork {
                id,
                key: Some(Box::new(key)),
                hold: TransitionHold::UnsettledByRecovery { action },
            });
        }
        if let Some(recording) = self
            .action_of(id)
            .and_then(|action| self.effects.recording_for(action))
        {
            return Some(PendingWork {
                id,
                key: Some(Box::new(recording.key)),
                hold: TransitionHold::RecordingFailed {
                    evidence: recording.evidence,
                    cause: recording.cause.to_string(),
                },
            });
        }
        let transition = self.transitions.get(id.chain).and_then(Option::as_ref)?;
        let state = transition.entries.get(id.entry)?;
        if !(state.is_open() || state.is_abandoned()) {
            return None;
        }
        // An open or abandoned state always renders a hold.
        let hold = state.hold(budget)?;
        Some(PendingWork {
            id,
            key: self.key_of(id.chain, id.entry, transition).map(Box::new),
            hold,
        })
    }

    /// The first entry that is unresolved, in `(chain, entry)` order: the
    /// abandonment in front of its successors, the recovered unknown in front
    /// of a fresh attempt, the open entry itself otherwise. Taking the first
    /// in declaration order is deliberate: the barrier is the entry that
    /// explains every successor blocked behind it, so naming it is naming the
    /// reason.
    pub(in crate::ecs) fn first_unresolved(&self, budget: u32) -> Option<PendingWork> {
        self.unresolved(budget).into_iter().next()
    }

    /// Every entry that is not finished, in `(chain, entry)` order: open,
    /// given up on and still reported, or held by a recovered unknown.
    pub(in crate::ecs) fn pending(&self, budget: u32) -> Vec<PendingWork> {
        self.unresolved(budget)
    }

    /// How many entries are still unresolved, across every chain.
    ///
    /// Counted the same way [`Self::first_unresolved`] scans, because the two
    /// are rendered together in [`EcsError::PendingTransition`]: a count that
    /// omitted abandoned entries would report a non-zero entry alongside an
    /// "0 outstanding", which reads as the one thing the pair is there to rule
    /// out — a chain that is somehow both stuck and finished. A recovered
    /// unknown is counted for the same reason: it is what is holding the work.
    ///
    /// [`EcsError::PendingTransition`]: crate::ecs::EcsError::PendingTransition
    pub(in crate::ecs) fn unresolved_count(&self, budget: u32) -> usize {
        self.unresolved(budget).len()
    }
}
