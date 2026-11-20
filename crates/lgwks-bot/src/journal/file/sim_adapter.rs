//! Seeded simulation of the file adapter's `confirm_outcome`, inside the
//! adapter's own module path.
//!
//! A receipt is a liar's favourite document: the right position with the wrong
//! fact, the right fact under someone else's key, a neighbour's position, or a
//! grade the disk never promised. Each seed draws the outcome's evidence and how
//! many neighbour ladders sit in front of it, and the oracle is the ladder the
//! seed wrote: only the receipt that names exactly that committed outcome, at a
//! grade a file can keep, is attested.

#[cfg(test)]
use std::error::Error;
use std::num::NonZeroU64;
use std::path::PathBuf;

use super::{
    DurabilityPromise, DurableAck, EffectEvent, EffectEvidence, EffectJournal, FileJournal,
    JournalError, JournalPosition,
};
use crate::effect::{
    ActionDigest, ActionId, AttemptId, DigestError, EffectIdentity, EffectKey, EnvironmentEpoch,
    EnvironmentId, FlowRevision, IdError, RunId,
};
use crate::journal::file::tests::scratch;

/// Why a seed's row could not be run, as opposed to an assertion it failed.
#[derive(Debug)]
enum SimError {
    /// A fixture id did not parse.
    Id(IdError),
    /// A fixture digest did not parse.
    Digest(DigestError),
    /// The journal refused the honest ladder or the honest receipt.
    Journal(JournalError),
    /// The scratch path could not be named.
    Scratch(std::io::Error),
}

impl std::fmt::Display for SimError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Id(ref cause) => write!(formatter, "fixture id: {cause}"),
            Self::Digest(ref cause) => write!(formatter, "fixture digest: {cause:?}"),
            Self::Journal(ref cause) => write!(formatter, "journal: {cause}"),
            Self::Scratch(ref cause) => write!(formatter, "scratch path: {cause}"),
        }
    }
}

impl Error for SimError {}

impl From<IdError> for SimError {
    fn from(cause: IdError) -> Self {
        Self::Id(cause)
    }
}

impl From<DigestError> for SimError {
    fn from(cause: DigestError) -> Self {
        Self::Digest(cause)
    }
}

impl From<JournalError> for SimError {
    fn from(cause: JournalError) -> Self {
        Self::Journal(cause)
    }
}

impl From<std::io::Error> for SimError {
    fn from(cause: std::io::Error) -> Self {
        Self::Scratch(cause)
    }
}

/// Removes one seed's journal file however the row ends, and says so when it cannot.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let removed = match self.0.is_dir() {
            true => std::fs::remove_dir_all(&self.0),
            false => std::fs::remove_file(&self.0),
        };
        if let Err(error) = removed {
            lgwks_std::trace::debug!(error = ?error, path = %self.0.display(), "sim_adapter: scratch left behind");
        }
    }
}

/// The key for attempt `1 + offset` of one fixed run: rows and neighbours differ
/// only there. The attempt is built from a non-zero base, so no offset can name
/// the zero attempt the schema excludes.
fn key_of(offset: u64) -> Result<EffectKey, SimError> {
    let identity = EffectIdentity::new(
        RunId::from_hex(&"5a".repeat(16))?,
        EnvironmentId::from_hex(&"6b".repeat(16))?,
        FlowRevision::from_tagged("blake3_256", &"7c".repeat(32))?,
    );
    Ok(identity.key(
        ActionId::from_hex(&"8d".repeat(16))?,
        AttemptId::new(NonZeroU64::MIN.saturating_add(offset)),
        ActionDigest::from_tagged("blake3_256", &"9e".repeat(32))?,
        EnvironmentEpoch::FIRST,
    ))
}

/// Climbs `key`'s whole ladder and answers the outcome rung's acknowledgment.
fn settle(
    journal: &mut FileJournal,
    key: EffectKey,
    evidence: EffectEvidence,
) -> Result<DurableAck, JournalError> {
    for rung in [
        EffectEvent::IntentAdmitted { key },
        EffectEvent::DispatchPrepared { key },
    ] {
        journal.compare_and_append(journal.tail(), &rung)?;
    }
    journal.compare_and_append(
        journal.tail(),
        &EffectEvent::OutcomeObserved { key, evidence },
    )
}

/// Whether `answer` is the mismatch that names `expected` against `tail`.
fn mismatched(
    answer: &Result<DurableAck, JournalError>,
    expected: JournalPosition,
    tail: JournalPosition,
) -> bool {
    matches!(*answer, Err(JournalError::ReceiptMismatch { expected: named, actual }) if named == expected && actual == tail)
}

#[test]
fn sim_confirm_outcome_attests_only_the_receipt_the_adapter_committed() -> Result<(), SimError> {
    let grade = DurabilityPromise::ProcessCrash;
    for seed in 0..8_u64 {
        let path = scratch(&format!("sim-adapter-{seed}"))?;
        let _scratch = Scratch(path.clone());
        let mut journal = FileJournal::open(&path)?;
        let mut neighbour = None;
        for slot in 0..(seed >> 1) % 3 {
            neighbour = Some(settle(
                &mut journal,
                key_of(100 + slot)?,
                EffectEvidence::Applied,
            )?);
        }
        let (evidence, other) = match seed & 1 {
            0 => (EffectEvidence::Applied, EffectEvidence::NotApplied),
            _ => (EffectEvidence::NotApplied, EffectEvidence::Applied),
        };
        let key = key_of(1 + seed)?;
        let position = settle(&mut journal, key, evidence)?.position();
        let tail = journal.tail();

        let attested = journal.confirm_outcome(key, evidence, position, grade)?;
        assert_eq!(
            attested.position(),
            position,
            "seed {seed}: the honest receipt"
        );
        assert!(
            mismatched(
                &journal.confirm_outcome(key, other, position, grade),
                position,
                tail
            ),
            "seed {seed}: the right position with the wrong evidence is refused"
        );
        assert!(
            mismatched(
                &journal.confirm_outcome(key_of(1000 + seed)?, evidence, position, grade),
                position,
                tail
            ),
            "seed {seed}: the right outcome under another key is refused"
        );
        if let Some(ack) = neighbour {
            assert!(
                mismatched(
                    &journal.confirm_outcome(key, evidence, ack.position(), grade),
                    ack.position(),
                    tail
                ),
                "seed {seed}: a neighbour's committed outcome is not this key's"
            );
        }
        assert!(
            matches!(
                journal.confirm_outcome(key, evidence, position, DurabilityPromise::PowerLoss),
                Err(JournalError::ReceiptUnavailable {
                    required: DurabilityPromise::PowerLoss
                })
            ),
            "seed {seed}: a file journal never attests power-loss durability"
        );
    }
    Ok(())
}
