//! Durable journal: the append that has to land before the irreversible
//! boundary.
//!
//! [`crate::effect`] supplies the identity a settlement is about. This module
//! supplies the place that identity is written down *before* anything
//! irreversible happens, because an identity held only in memory is lost at the
//! exact moment it is needed. A controller that hands bytes to an external
//! system and then crashes has no way to say whether the bytes arrived, and a
//! controller that guesses will either duplicate a non-idempotent effect or
//! silently drop one.
//!
//! # The promise a durability profile makes
//!
//! [`DurabilityPromise`] is graded, and the grade is the whole point: an
//! in-memory adapter can promise nothing across a process exit, so it reports
//! [`DurabilityPromise::Ephemeral`] and [`EffectJournal::admit_external_handoff`]
//! refuses it. That refusal is the load-bearing part of this module. A journal
//! that accepts an append and loses it on restart is worse than no journal,
//! because it converts "I do not know" into a false "I never sent it".
//!
//! Nothing here can verify a promise. A filesystem, a device cache or a
//! virtualization layer can each accept a write and lose it anyway, so a
//! [`DurableAck`] records the promise that was claimed rather than implying one
//! was proven. The tests that matter for that claim are crash tests against a
//! real backing store, and they are not unit tests.
//!
//! # Why compare-and-append
//!
//! Two controllers recovering the same run must not both dispatch. `&mut self`
//! fences within one process, which is not enough: the failure this guards is a
//! second process. So every append states the tail it believes is committed, and
//! an append whose belief is stale is refused rather than merged. A refused
//! append has changed nothing, which is what makes it safe to recover from.
//!
//! [`EffectKey`]: crate::effect::EffectKey
//! [`DurableAck`]: crate::journal::DurableAck
//! [`DurabilityPromise`]: crate::journal::DurabilityPromise
//! [`EffectJournal`]: crate::journal::EffectJournal
//! [`DurabilityPromise::Ephemeral`]: crate::journal::DurabilityPromise::Ephemeral
//! [`EffectJournal::admit_external_handoff`]: crate::journal::EffectJournal::admit_external_handoff

use core::fmt;

use lgwks_std::hash::{Digest, blake3};
#[cfg(feature = "wire")]
use lgwks_std::wire::{AlignedVec, WireError};

use crate::effect::{EffectKey, Id128};

/// The evidence vocabulary is the ledger's, re-exported here rather than
/// restated.
///
/// The journal and the ledger answer the same question, "what does the evidence
/// say about whether the effect landed", and a second enum with the same two
/// arms would drift from this one the first time an arm was added. Re-exporting
/// keeps one definition with two consumers.
pub use crate::ecs::EffectEvidence;

/// What the shipped adapters share: the event ceiling and the trait methods
/// both answer the same way. First, so its macros are in scope in every adapter
/// module declared below.
#[macro_use]
mod adapters;

pub(crate) use adapters::bound_events;
#[cfg(feature = "wire")]
pub(crate) use adapters::confirm_outcome_by;

mod wire_form;

#[cfg(feature = "wire")]
pub use wire_form::{ArchivedEffectEvent, ArchivedVerificationResult};
/// The two journal records whose archived form is an enum, re-exported from the
/// private `wire_form` module, which records why they are declared there.
pub use wire_form::{EffectEvent, VerificationResult};

/// The file journal persists every record through the wire encoding, so it
/// exists only where `wire` is on (#687: a process-only build carries none).
#[cfg(feature = "wire")]
mod file;

/// The frame grammar the file journal and the run store share.
///
/// Private to the crate because a caller cannot frame a record for either store:
/// what a head is over, and what a frame even holds, is each store's own. The
/// grammar — length prefix, torn-tail rule, impossible-length refusal — is one
/// definition both call, so the two stores cannot disagree about what a torn tail
/// is.
pub(crate) mod frame;

/// The one thread that owns a file-backed store's device.
///
/// Extracted from `file` because the run store inherited the same blocking-write
/// defect and needed the same mechanism, and a second owner thread beside the
/// first would be two poison latches with two behaviours. The poison-safe lock
/// helpers it also holds serve the whole crate; the owner thread itself exists
/// only with `wire`, which both stores persist through.
pub(crate) mod owner;

/// Continue-as-new: a sealed checkpoint and the successor opened from it.
///
/// Re-exported beside the two adapters because it is what a caller asks *of* an
/// adapter rather than what an adapter is: the policy lives here, the file that
/// can carry it out is `FileJournal`, and the trait's two continuation methods
/// are what joins them.
pub mod continuation;

pub use continuation::{
    CONTINUATION_WATERMARK_DENOMINATOR, CONTINUATION_WATERMARK_NUMERATOR, Continuation,
    ContinuationPolicy, ContinuationWatermark, MAX_CHECKPOINT_SETTLED, MAX_CHECKPOINT_UNRESOLVED,
    SealPause, SettledAttempt, UnresolvedAttempt, is_generation_path, successor_path,
};

/// The file-backed journal, re-exported from the private `file` module beside
/// the in-memory one: the trait's second shipped adapter, and the one whose
/// promises a process kill can check.
///
/// `StorageGate` is deliberately absent from this list: one `journal::owner`
/// serves both file-backed stores, so the gate is re-exported from its owning
/// module below rather than from either store, and naming it here would make a
/// caller believe the journal owns a type the run store shares.
#[cfg(feature = "wire")]
pub use file::{Corruption, CorruptionKind, FileJournal, Replay};

/// The durable journal over a consumer's own record, on the grammar, the owner
/// thread and the seal step the effect journal uses.
#[cfg(feature = "wire")]
mod records;

#[cfg(feature = "wire")]
pub use records::{Durable, RecordEntry, RecordJournal, RecordReplay};

/// The handle that releases a stalled store's parked flush.
///
/// Public because "what does this bot do while its disk has stopped answering" is
/// a question an operator has to be able to ask on a real process: the caller
/// awaiting the append holds the journal's borrow for the whole wait, so it cannot
/// reach the journal to release its own device.
#[cfg(feature = "wire")]
pub use owner::StorageGate;

/// The domain separator hashed into the genesis position.
///
/// A chain has to start somewhere, and "started from 32 zero bytes" is a value
/// any other hash could coincide with. Separating the domain means the genesis
/// head cannot be mistaken for the hash of an empty or zeroed event.
const GENESIS_DOMAIN: &[u8] = b"lgwks.journal.v1.genesis";

/// Hard ceiling on events retained by either shipped journal adapter.
///
/// The full history remains available; once this count is reached, new events
/// are refused rather than deleting facts needed to resolve an effect or
/// deduplicate a retry. The journal path is full until its owner performs an
/// explicitly safe handoff to a distinct run and journal; this adapter does not
/// compact or rotate it automatically.
pub const MAX_JOURNAL_EVENTS: usize = 100_000;

/// Hard ceiling on encoded frame bytes retained by either shipped adapter.
///
/// The memory adapter applies the same logical-frame accounting even though it
/// retains entries and indexes rather than framed byte buffers.
pub const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;

/// What a journal can promise about an append that it has acknowledged.
///
/// Ordered from weakest to strongest, and compared by that order: a caller that
/// needs process-crash survival is satisfied by a power-loss promise, never the
/// reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DurabilityPromise {
    /// Survives neither a process exit nor a crash. An in-memory adapter
    /// reports this, and this is the only honest thing it can report.
    Ephemeral,
    /// Survives the writing process dying, including a kill. May still be lost
    /// if the machine loses power before the store flushes.
    ProcessCrash,
    /// Survives power loss, to the extent the device honours its own flush
    /// contract.
    PowerLoss,
}

impl DurabilityPromise {
    /// Whether an append under this promise survives the writer dying.
    ///
    /// This is the threshold an external handoff requires. Below it, a
    /// recovered controller cannot distinguish "the effect never left" from
    /// "the record of it leaving was lost", and that is the state in which a
    /// blind resend happens.
    #[must_use]
    pub const fn survives_process_crash(self) -> bool {
        matches!(self, Self::ProcessCrash | Self::PowerLoss)
    }

    /// Whether this promise is at least as strong as `required`.
    ///
    /// A per-append acknowledgment is checked against the promise the handoff
    /// needs, not against the journal's advertisement: a store that claims
    /// `ProcessCrash` and acks `Ephemeral` is weaker than it says, and the
    /// weaker fact is the one that matters (issue #100).
    #[must_use]
    pub const fn meets(self, required: Self) -> bool {
        match (self, required) {
            (_, Self::Ephemeral) => true,
            (Self::ProcessCrash | Self::PowerLoss, Self::ProcessCrash) => true,
            (Self::PowerLoss, Self::PowerLoss) => true,
            (Self::Ephemeral, _) => false,
            (Self::ProcessCrash, Self::PowerLoss) => false,
        }
    }

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ephemeral => "ephemeral",
            Self::ProcessCrash => "process_crash",
            Self::PowerLoss => "power_loss",
        }
    }
}

impl fmt::Display for DurabilityPromise {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a committed event sits in one run's journal.
///
/// The pair of sequence and head is the whole point. A sequence alone says
/// which append this was and nothing about what it contained, so two journals
/// that diverged would agree on their sequence numbers. The head is a hash over
/// every event up to and including this one, so a position is a commitment to
/// the entire history behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "wire",
    derive(
        lgwks_std::wire::Archive,
        lgwks_std::wire::Serialize,
        lgwks_std::wire::Deserialize
    )
)]
#[cfg_attr(feature = "wire", rkyv(crate = lgwks_std::wire::rkyv, compare(PartialEq), derive(Debug)))]
pub struct JournalPosition {
    /// How many events are committed at or before this position. Zero is the
    /// genesis, which holds no events.
    sequence: u64,
    /// Hash over every committed event, in order, up to and including this one.
    head: Digest,
}

impl JournalPosition {
    /// The position before the first append.
    ///
    /// Its head is a domain-separated constant rather than a zero digest, so a
    /// journal that was never written cannot be confused with one whose first
    /// event hashed to zero.
    #[must_use]
    pub fn genesis() -> Self {
        Self {
            sequence: 0,
            head: blake3(GENESIS_DOMAIN),
        }
    }

    /// How many events are committed at or before this position.
    #[must_use]
    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    /// The hash over every committed event up to and including this one.
    #[must_use]
    pub const fn head(self) -> Digest {
        self.head
    }
}

impl fmt::Display for JournalPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} @ {}", self.sequence, self.head)
    }
}

/// Which kind of fact an [`EffectEvent`] records.
///
/// Separate from the event because the ordering ladder is stated in kinds, and
/// a refusal that names the two kinds involved reads in a log line in a way
/// that a rendered event carrying its full key does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EventKind {
    /// The intent was frozen and admitted. Nothing has left the process.
    IntentAdmitted,
    /// Broker authority was obtained and this exact attempt was prepared. This
    /// is the last append before the irreversible boundary.
    DispatchPrepared,
    /// What the evidence says about whether the effect landed.
    OutcomeObserved,
    /// A named predicate was evaluated and produced a result.
    Verified,
}

/// Which journal resource reached its hard admission ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum JournalLimitKind {
    /// Number of retained events.
    Events,
    /// Encoded frame bytes admitted to the journal history.
    Bytes,
    /// A continuation watermark at or past the ceiling it would be measured
    /// against.
    ContinuationWatermark,
    /// Generations a chain of journals may hold, counted from the name a
    /// continuation is derived from.
    Generation,
    /// Actions one sealed checkpoint may fold.
    CheckpointActions,
    /// Unresolved attempts one sealed checkpoint may carry.
    CheckpointUnresolved,
    /// Bytes one frame may hold, which a consumer's own record can reach.
    FrameBytes,
}

impl EventKind {
    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IntentAdmitted => "intent_admitted",
            Self::DispatchPrepared => "dispatch_prepared",
            Self::OutcomeObserved => "outcome_observed",
            Self::Verified => "verified",
        }
    }

    /// The kind that may follow this one for the same key.
    ///
    /// One attempt at one intent walks the *effect* ladder exactly once. A retry
    /// is a new [`crate::effect::AttemptId`] and therefore a new key, which is
    /// what stops a second `DispatchPrepared` for an existing key from being
    /// representable at all.
    ///
    /// Verification is the one rung that may repeat. It is a statement about the
    /// world *at the time a predicate ran*, and the world moves: the file is
    /// edited, the remote state changes, the predicate itself is revised. A
    /// latched verdict would stay `Verified` forever with nothing able to say it
    /// no longer holds, so a later `Verified` event supersedes the earlier one and
    /// the recovered status follows the latest verdict. The effect has still
    /// happened once; only what is known about its postcondition is revised.
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        match self {
            Self::IntentAdmitted => Some(Self::DispatchPrepared),
            Self::DispatchPrepared => Some(Self::OutcomeObserved),
            Self::OutcomeObserved | Self::Verified => Some(Self::Verified),
        }
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl VerificationResult {
    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Satisfied => "satisfied",
            Self::NotSatisfied => "not_satisfied",
        }
    }
}

impl fmt::Display for VerificationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A named predicate's answer, with what it was evaluated against.
///
/// The observations are carried as a digest rather than an inline record so the
/// journal stays bounded, and as a digest rather than a summary so the answer
/// cannot be re-read under a different input set. A bare "done" has no place
/// here: it names no predicate, so nothing can re-evaluate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "wire",
    derive(
        lgwks_std::wire::Archive,
        lgwks_std::wire::Serialize,
        lgwks_std::wire::Deserialize
    )
)]
#[cfg_attr(feature = "wire", rkyv(crate = lgwks_std::wire::rkyv, compare(PartialEq), derive(Debug)))]
pub struct Verification {
    /// Which predicate was evaluated.
    predicate: Id128,
    /// Which revision of that predicate. A changed predicate is a different
    /// claim over the same observations, so the version is part of the record.
    predicate_version: u64,
    /// Digest of the observations the predicate was evaluated against.
    observations: Digest,
    /// The answer.
    result: VerificationResult,
}

impl Verification {
    /// Build a verification record.
    #[must_use]
    pub const fn new(
        predicate: Id128,
        predicate_version: u64,
        observations: Digest,
        result: VerificationResult,
    ) -> Self {
        Self {
            predicate,
            predicate_version,
            observations,
            result,
        }
    }

    /// Which predicate was evaluated.
    #[must_use]
    pub const fn predicate(self) -> Id128 {
        self.predicate
    }

    /// Which revision of that predicate.
    #[must_use]
    pub const fn predicate_version(self) -> u64 {
        self.predicate_version
    }

    /// Digest of the observations the predicate was evaluated against.
    #[must_use]
    pub const fn observations(self) -> Digest {
        self.observations
    }

    /// The answer.
    #[must_use]
    pub const fn result(self) -> VerificationResult {
        self.result
    }
}

impl EffectEvent {
    /// The attempt this fact is about.
    #[must_use]
    pub const fn key(self) -> EffectKey {
        match self {
            Self::IntentAdmitted { key }
            | Self::DispatchPrepared { key }
            | Self::OutcomeObserved { key, .. }
            | Self::Verified { key, .. } => key,
        }
    }

    /// Which kind of fact this is.
    #[must_use]
    pub const fn kind(self) -> EventKind {
        match self {
            Self::IntentAdmitted { .. } => EventKind::IntentAdmitted,
            Self::DispatchPrepared { .. } => EventKind::DispatchPrepared,
            Self::OutcomeObserved { .. } => EventKind::OutcomeObserved,
            Self::Verified { .. } => EventKind::Verified,
        }
    }

    /// The event's encoding, which is what the chain hashes.
    ///
    /// Public because an adapter outside this crate has to compute the same
    /// head to be a journal at all. The encoding is the estate's, not this
    /// module's: the record is archived by [`lgwks_std::wire`] exactly as every
    /// other estate type that crosses a byte boundary, so the discriminant, the
    /// identity and the payload are one encode rather than a framing this module
    /// maintains beside it. A reader accesses the archive in place instead of
    /// decoding it, which is the reason the format exists.
    ///
    /// The bytes are what a chain head commits to, so they are part of this
    /// module's durable contract: a change to the record's shape changes every
    /// head computed after it.
    ///
    /// # Errors
    ///
    /// [`WireError`] if the archive cannot be allocated.
    #[cfg(feature = "wire")]
    pub fn to_bytes(self) -> Result<AlignedVec, WireError> {
        lgwks_std::wire::to_bytes::<WireError>(&self)
    }
}

impl fmt::Display for EffectEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.kind(), self.key())
    }
}

/// One committed entry: the event and the position it landed at.
///
/// A named struct rather than a tuple because the two fields answer different
/// questions and a reader should not have to remember which came first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JournalEntry {
    /// Where the entry sits in the chain.
    position: JournalPosition,
    /// The fact that was appended.
    event: EffectEvent,
}

impl JournalEntry {
    /// Build an entry.
    #[must_use]
    pub const fn new(position: JournalPosition, event: EffectEvent) -> Self {
        Self { position, event }
    }

    /// Where the entry sits in the chain.
    #[must_use]
    pub const fn position(&self) -> JournalPosition {
        self.position
    }

    /// The fact that was appended.
    ///
    /// By reference: [`EffectEvent`] carries a whole [`EffectKey`], and a
    /// journal replay walks every entry, so copying one per step would be a
    /// 192-byte move for no gain.
    #[must_use]
    pub const fn event(&self) -> &EffectEvent {
        &self.event
    }
}

/// Proof that an append is committed under a named durability promise.
///
/// An acknowledgment is not a buffer acceptance, a log line or a `Drop`. It is
/// the statement that the configured promise has been met and that the journal's
/// head is now at [`Self::position`]. An adapter mints this only after the
/// promise is met, and nothing here can check that it did, which is why the
/// promise travels on the acknowledgment: a caller can at least see what was
/// claimed, and a test against a real store is what decides whether the claim
/// was true.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DurableAck {
    /// The committed position, sequence and head together.
    position: JournalPosition,
    /// The promise the journal claims to have met.
    promise: DurabilityPromise,
}

impl DurableAck {
    /// Mint an acknowledgment.
    ///
    /// Public because an adapter implementing [`EffectJournal`] outside this
    /// crate is where an acknowledgment is produced, and the type cannot be
    /// built any other way.
    #[must_use]
    pub const fn new(position: JournalPosition, promise: DurabilityPromise) -> Self {
        Self { position, promise }
    }

    /// Where the append landed.
    #[must_use]
    pub const fn position(self) -> JournalPosition {
        self.position
    }

    /// What the journal claims about surviving a crash.
    #[must_use]
    pub const fn promise(self) -> DurabilityPromise {
        self.promise
    }
}

#[cfg(test)]
#[cfg(feature = "wire")]
mod tests;

// The refusal vocabulary, recovery, and the chain check with the in-memory
// journal, each in its own file; re-exported so every path stays
// `journal::<Name>`.
mod error;
mod memory;
mod recover;
mod seam;

// The chain helpers the file adapter and the other stores share.
#[cfg(test)]
#[cfg(feature = "wire")]
use memory::chain;
#[cfg(feature = "wire")]
pub(crate) use memory::chain_over_bytes;
#[cfg(feature = "wire")]
use memory::check_append_order;
#[cfg(feature = "wire")]
use memory::next_allowed_of;

pub use error::JournalError;
pub use memory::{ChainBreak, MemoryJournal};
#[cfg(feature = "wire")]
pub use memory::{verify_chain, verify_chain_from};
pub use recover::{Attempt, AttemptStatus, Recovered, Transition, recover, recover_continued};
pub use seam::EffectJournal;
