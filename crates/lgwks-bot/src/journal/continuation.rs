//! Continue-as-new for the effect journal: a sealed checkpoint, and the
//! successor opened from it.
//!
//! # The shape of the fix
//!
//! `MAX_JOURNAL_EVENTS` and `MAX_JOURNAL_BYTES` are hard ceilings, and an
//! unattended bot that reaches one stops taking effects. It stops *safely* —
//! nothing is truncated, nothing unresolved is dropped — and it still stops.
//! This module is the way past the ceiling that keeps the two properties the
//! journal already has:
//!
//! - the journal stays **append-only**, so a chain head still commits to every
//!   fact behind it and a tampered frame is still a refusal rather than a trim;
//! - **unresolved work is never dropped**, so an attempt whose outcome is unknown
//!   stays unknown in the successor until evidence settles it.
//!
//! The two production references agree on the mechanism. Temporal's
//! continue-as-new closes a workflow run whose history nears its limit and
//! starts a fresh run carrying only the state it needs; the old history is
//! closed, not rewritten. Raft and etcd take a snapshot at an index, keep the
//! entries after it, and drop the entries before it only once the snapshot is
//! durable. Both answer the same question — *where does a reopen start?* — and
//! both make the answer decidable from the bytes on the disk rather than from a
//! pointer somebody has to keep in step.
//!
//! The module holds the checkpoint record itself; `watermark` decides *when* a
//! journal continues and `naming` decides *where* its successor lives.
//!
//! # One frame grammar, one writer thread
//!
//! The checkpoint is a **second record type in the same chain**, framed by
//! `crate::journal::frame` with the same chain function every effect event
//! uses. It is not a second grammar and not a second writer: the seal runs on
//! the journal's existing storage-owner thread, so the length fence, the write
//! and the flush that cover it are the same ordered step every append already
//! takes (INV-BOT-50, INV-BOT-130).
//!
//! A reader tells the two record types apart by a magic prefix on the payload
//! (`CHECKPOINT_MAGIC`), never by trying one decode and falling back to the
//! other: a payload that does not carry the magic is an event, and a payload
//! that carries it is a checkpoint. See [`Continuation::is_payload`].
//!
//! # What a crash leaves
//!
//! One durable fact decides, and it is the **successor's first frame**: the seal
//! frame's bytes are written into the predecessor *and* into the successor, byte
//! for byte, so the successor's chain starts at the predecessor's tail and its
//! first frame authenticates against it.
//!
//! | crash point | what a reopen finds | who is authoritative |
//! |---|---|---|
//! | before the predecessor's seal frame is whole | a torn tail, repaired | the predecessor |
//! | after the predecessor's seal, before the successor exists | a seal frame whose successor is absent | the predecessor, which resumes appending |
//! | after the successor's first frame is whole | the seal is in force | the successor |
//!
//! Exactly one journal is authoritative in every row, and the decision is made
//! from the bytes: a journal holding a seal frame whose successor is complete
//! refuses itself as [`JournalError::Superseded`] and names where the
//! authoritative one is. A seal whose successor is absent or torn is *not* in
//! force — the continuation never committed — so the predecessor resumes and the
//! interrupted continuation is completed rather than abandoned. Nothing is ever
//! rewritten: the predecessor's seal frame stays in its history either way.
//!
//! # What the checkpoint carries, and what it compacts
//!
//! Carried, in full identity, and never dropped:
//!
//! - **every unresolved attempt** — a `DispatchPrepared` with no outcome, or an
//!   `IntentAdmitted` with no preparation — with the rung it reached. A key that
//!   is `OutcomeUnknown` in the predecessor is `OutcomeUnknown` in the
//!   successor, and the ladder refuses a fresh `IntentAdmitted` for it, so it
//!   cannot be resent;
//! - **the latest attempt of every action**, with its key, its resolved
//!   `AttemptStatus` including `VerificationFailed`, and the verification digest
//!   a `Verified` or `VerificationFailed` status is qualified by;
//! - **the predecessor's chain head** the successor's own chain starts from.
//!
//! Compacted, and the compaction is what makes reopen bounded: an attempt *older*
//! than the one its action reached is refused rather than remembered.
//! `AttemptId` is monotonic per `ActionId` and never reused, so "at or below the
//! latest attempt of that action" *is* "the predecessor already walked it", and
//! the refusal is exactly the ladder's own answer. Carrying one record per action
//! instead of one per attempt is what makes a successor's reopen cost and RSS
//! flat in the number of attempts the *predecessor* ever made — measured in
//! `examples/journal_continuation.rs`.

use core::fmt;

#[cfg(feature = "wire")]
use lgwks_std::wire::WireError;

use crate::effect::{ActionId, AttemptId, EffectKey};

use super::frame::SaturatingFrom;
use super::{
    AttemptStatus, EventKind, JournalError, JournalLimitKind, JournalPosition, Verification,
};

// Naming and the measured watermark serve only the file journal, so they build with it.
#[cfg(feature = "wire")]
mod naming;
#[cfg(all(test, feature = "wire"))]
mod sim_naming;
#[cfg(test)]
mod sim_watermark;
mod watermark;

#[cfg(feature = "wire")]
pub(crate) use naming::{generation_of, is_generation_path, refuse_ambiguous_base, successor_path};
pub use watermark::{ContinuationPolicy, ContinuationWatermark};

/// The prefix that marks a frame payload as a checkpoint rather than an event.
///
/// Padded to a multiple of eight so the archived record behind it starts on the
/// alignment a decoder expects, and declared rather than written inline because
/// the reader's first question about every frame is this one.
///
/// The padding is not decoration: these bytes are compared, so a magic that ran
/// into the archived record would make a checkpoint's own first field part of the
/// discriminator.
pub(crate) const CHECKPOINT_MAGIC: &[u8; 32] = b"lgwks.journal.checkpoint.v1\0\0\0\0\0";

/// The most unresolved attempts one checkpoint may carry.
///
/// Checked before the checkpoint is archived, so an over-large carry is a typed
/// refusal that leaves both journals untouched rather than a frame the writer
/// cannot produce. The archived bytes are bounded independently by the shared
/// grammar's own frame ceiling, and whichever bound trips first is the one
/// reported.
pub const MAX_CHECKPOINT_UNRESOLVED: usize = 128;

/// The most actions one checkpoint may fold.
///
/// One record per *action*, not per attempt — see the module's note on what the
/// checkpoint compacts — so this is bounded by the shape of a bot rather than by
/// how long it has run.
///
/// Sized so the widest checkpoint both counts admit fits the one frame it is
/// sealed in: a settled record archives to at most 240 bytes and an unresolved one
/// to 160, so this many settled records beside a full [`MAX_CHECKPOINT_UNRESOLVED`]
/// carry is 58,976 bytes of a 65,536-byte frame. It was 256, which archives to
/// 90,208 bytes: a count no checkpoint could reach, refused by the frame as a
/// storage fault instead of by the count it names.
pub const MAX_CHECKPOINT_SETTLED: usize = 160;

/// How far along a continuation may be driven, for a harness that must kill the
/// process exactly there.
///
/// The four points are the four places bytes become durable in order, and a
/// crash between any two of them leaves a different file to reopen. A
/// continuation armed to stop at one of them refuses *after* the named step has
/// completed and before the next byte moves, so a process killed on that refusal
/// is killed exactly at the boundary rather than somewhere near it.
///
/// Public rather than `cfg(test)` for the reason
/// `FileJournal::open_with_stalled_storage` is: "what does this
/// bot's record look like after the disk dies mid-continuation" is a question
/// about a real process, and a probe that only exists inside the crate's own test
/// binary cannot answer it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SealPause {
    /// The predecessor's seal frame is written and not yet flushed, and the
    /// successor does not exist.
    AfterSealWrite,
    /// The successor's first frame is written and not yet flushed.
    AfterSuccessorWrite,
    /// The successor's first frame is flushed and its directory entry is not.
    AfterSuccessorSync,
    /// The successor's directory entry is flushed and the predecessor's seal is
    /// not yet flushed.
    AfterDirectorySync,
}

impl SealPause {
    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AfterSealWrite => "after_seal_write",
            Self::AfterSuccessorWrite => "after_successor_write",
            Self::AfterSuccessorSync => "after_successor_sync",
            Self::AfterDirectorySync => "after_directory_sync",
        }
    }

    /// Every boundary, in the order the continuation reaches them.
    ///
    /// A crash harness sweeps this rather than spelling four values by hand, so a
    /// boundary added to this enum is swept the day it exists.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [
            Self::AfterSealWrite,
            Self::AfterSuccessorWrite,
            Self::AfterSuccessorSync,
            Self::AfterDirectorySync,
        ]
    }
}

impl fmt::Display for SealPause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One action's folded state at a continuation.
///
/// The latest attempt the predecessor walked, and what it resolved to. Every
/// attempt *older* than this one for the same action is refused by the successor
/// rather than carried, which is what keeps the carry bounded by actions rather
/// than by attempts; see the module's note on what the checkpoint compacts.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "wire",
    derive(
        lgwks_std::wire::Archive,
        lgwks_std::wire::Serialize,
        lgwks_std::wire::Deserialize
    )
)]
#[cfg_attr(feature = "wire", rkyv(crate = lgwks_std::wire::rkyv, compare(PartialEq), derive(Debug)))]
pub struct SettledAttempt {
    /// The latest attempt the predecessor walked for its action, whose action and
    /// attempt number are read out of it rather than stored a second time: two
    /// copies of one identity in a durable record are two answers a crafted
    /// checkpoint could make disagree.
    key: EffectKey,
    /// The rung the predecessor's ladder had reached for it.
    rung: u8,
    /// What the predecessor's fold resolved it to.
    status: u8,
    /// The verification the status is qualified by, when one was evaluated.
    verification: Option<Verification>,
}

impl SettledAttempt {
    /// Fold one action's latest attempt.
    #[must_use]
    pub fn new(
        key: EffectKey,
        rung: EventKind,
        status: AttemptStatus,
        verification: Option<Verification>,
    ) -> Self {
        Self {
            key,
            rung: rung_index(rung),
            status: status_index(status),
            verification,
        }
    }

    /// The action this record is about.
    #[must_use]
    pub const fn action(&self) -> ActionId {
        self.key.action()
    }

    /// The latest attempt the predecessor walked for that action.
    #[must_use]
    pub const fn attempt(&self) -> AttemptId {
        self.key.attempt()
    }

    /// That attempt's exact identity.
    #[must_use]
    pub const fn key(&self) -> EffectKey {
        self.key
    }

    /// The rung the ladder had reached, or `None` for a rung this build does not
    /// name.
    ///
    /// The decode is fallible because the record is durable: a checkpoint written
    /// by a build that named a rung this one does not is not silently read as the
    /// foot of the ladder, which would make a settled attempt replayable.
    #[must_use]
    pub const fn rung(&self) -> Option<EventKind> {
        rung_of(self.rung)
    }

    /// What the predecessor's fold resolved it to, or `None` for a status this
    /// build does not name.
    #[must_use]
    pub const fn status(&self) -> Option<AttemptStatus> {
        status_of(self.status)
    }

    /// The verification the status is qualified by.
    #[must_use]
    pub const fn verification(&self) -> Option<Verification> {
        self.verification
    }

    /// Whether `key` is this action's latest attempt or one the predecessor walked
    /// before it.
    ///
    /// The comparison is on `attempt` because `AttemptId` is monotonic per action
    /// and never reused, which is what makes a folded record answer "was this
    /// already walked?" without carrying every attempt that ever was.
    #[must_use]
    pub fn already_walked(&self, key: EffectKey) -> bool {
        key.action() == self.key.action() && key.attempt() <= self.key.attempt()
    }
}

/// One attempt the predecessor handed over and nothing has settled.
///
/// Carried in full, because dropping it would be the one thing this module must
/// never do: an attempt whose bytes may have reached the outside world and whose
/// outcome nobody established has to be unknown in the successor exactly as it
/// was in the predecessor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "wire",
    derive(
        lgwks_std::wire::Archive,
        lgwks_std::wire::Serialize,
        lgwks_std::wire::Deserialize
    )
)]
#[cfg_attr(feature = "wire", rkyv(crate = lgwks_std::wire::rkyv, compare(PartialEq), derive(Debug)))]
pub struct UnresolvedAttempt {
    /// The attempt.
    key: EffectKey,
    /// The rung it reached.
    rung: u8,
}

impl UnresolvedAttempt {
    /// Carry one unresolved attempt at the rung it reached.
    #[must_use]
    pub const fn new(key: EffectKey, rung: EventKind) -> Self {
        Self {
            key,
            rung: rung_index(rung),
        }
    }

    /// The attempt.
    #[must_use]
    pub const fn key(&self) -> EffectKey {
        self.key
    }

    /// The rung it reached, or `None` for a rung this build does not name.
    #[must_use]
    pub const fn rung(&self) -> Option<EventKind> {
        rung_of(self.rung)
    }

    /// What a recovery fold reports for this attempt.
    ///
    /// `None` for a rung this build does not name, and the caller refuses the
    /// checkpoint rather than reading an unknown rung as "nothing happened", which
    /// would turn an unresolved attempt into an absent one.
    #[must_use]
    pub const fn recovered_status(&self) -> Option<AttemptStatus> {
        match rung_of(self.rung) {
            Some(EventKind::IntentAdmitted) => Some(AttemptStatus::Prepared),
            Some(EventKind::DispatchPrepared) => Some(AttemptStatus::OutcomeUnknown),
            _ => None,
        }
    }
}

/// The archived index of a rung, for a record this module writes durably.
///
/// A byte rather than the enum's archived form, and the choice is the lint
/// contract's: an archived `pub enum` emits an exported resolver this workspace
/// cannot document (`journal::wire_form` records why), and a byte fails closed
/// instead — a rung index a later build does not name reads as `None`, never as
/// the foot of the ladder. The match is exhaustive inside this crate, and a rung
/// added to the enum therefore breaks the build here rather than reaching a
/// writer that would encode it as something no reader knows.
const fn rung_index(rung: EventKind) -> u8 {
    match rung {
        EventKind::IntentAdmitted => 0,
        EventKind::DispatchPrepared => 1,
        EventKind::OutcomeObserved => 2,
        EventKind::Verified => 3,
    }
}

/// The archived index of a status, for the same reason as [`rung_index`].
const fn status_index(status: AttemptStatus) -> u8 {
    match status {
        AttemptStatus::Prepared => 0,
        AttemptStatus::OutcomeUnknown => 1,
        AttemptStatus::Applied => 2,
        AttemptStatus::NotApplied => 3,
        AttemptStatus::Verified => 4,
        AttemptStatus::VerificationFailed => 5,
    }
}

/// The rung an archived index names, or `None` for one this build does not name.
const fn rung_of(index: u8) -> Option<EventKind> {
    match index {
        0 => Some(EventKind::IntentAdmitted),
        1 => Some(EventKind::DispatchPrepared),
        2 => Some(EventKind::OutcomeObserved),
        3 => Some(EventKind::Verified),
        _ => None,
    }
}

/// The status an archived index names, or `None` for one this build does not name.
const fn status_of(index: u8) -> Option<AttemptStatus> {
    match index {
        0 => Some(AttemptStatus::Prepared),
        1 => Some(AttemptStatus::OutcomeUnknown),
        2 => Some(AttemptStatus::Applied),
        3 => Some(AttemptStatus::NotApplied),
        4 => Some(AttemptStatus::Verified),
        5 => Some(AttemptStatus::VerificationFailed),
        _ => None,
    }
}

/// What a sealed journal hands to its successor.
///
/// The record a checkpoint writes: the predecessor's chain head, where the
/// successor's own chain starts, every unresolved attempt, and the folded state
/// of every action. Archived through `lgwks_std::wire` like every other estate
/// record that crosses a byte boundary, so a change to its shape changes every
/// head computed after it — which is what makes a checkpoint an audit record
/// rather than a hint.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "wire",
    derive(
        lgwks_std::wire::Archive,
        lgwks_std::wire::Serialize,
        lgwks_std::wire::Deserialize
    )
)]
#[cfg_attr(feature = "wire", rkyv(crate = lgwks_std::wire::rkyv, compare(PartialEq), derive(Debug)))]
pub struct Continuation {
    /// Which generation this successor is. One is the first successor of a journal
    /// that never continued.
    generation: u64,
    /// The predecessor's chain head, and the position the successor's first frame
    /// chains from.
    predecessor: JournalPosition,
    /// The latest attempt of every action the predecessor walked.
    settled: Vec<SettledAttempt>,
    /// Every attempt the predecessor handed over and nothing settled.
    unresolved: Vec<UnresolvedAttempt>,
}

impl Continuation {
    /// Build a checkpoint, refusing a carry this journal cannot frame.
    ///
    /// The bounds are charged here, before anything is archived or written, so a
    /// refused continuation leaves both files byte-identical.
    ///
    /// There is deliberately **no successor path** in this record. The path is
    /// [`successor_path`] of the file the reader is already holding, which is why
    /// the seal frame can be byte-identical in the predecessor and the successor:
    /// a record that named where it was going would have to name a different place
    /// in each of the two files, and a checkpoint whose meaning depends on which
    /// file it was read from is a second authority rather than a fact.
    ///
    /// # Errors
    ///
    /// [`JournalError::CapacityExceeded`] when either carried list is past its
    /// declared count.
    pub fn new(
        generation: u64,
        predecessor: JournalPosition,
        settled: Vec<SettledAttempt>,
        unresolved: Vec<UnresolvedAttempt>,
    ) -> Result<Self, JournalError> {
        if settled.len() > MAX_CHECKPOINT_SETTLED {
            let requested = u64::saturating_from(settled.len());
            let refusal = Err(JournalError::CapacityExceeded {
                resource: JournalLimitKind::CheckpointActions,
                limit: u64::saturating_from(MAX_CHECKPOINT_SETTLED),
                requested,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "Continuation::new: returning an error to the caller");
            return refusal;
        }
        if unresolved.len() > MAX_CHECKPOINT_UNRESOLVED {
            let requested = u64::saturating_from(unresolved.len());
            let refusal = Err(JournalError::CapacityExceeded {
                resource: JournalLimitKind::CheckpointUnresolved,
                limit: u64::saturating_from(MAX_CHECKPOINT_UNRESOLVED),
                requested,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "Continuation::new: returning an error to the caller");
            return refusal;
        }
        Ok(Self {
            generation,
            predecessor,
            settled,
            unresolved,
        })
    }

    /// Which generation this successor is.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// The predecessor's chain head, and the position this successor continues
    /// from.
    #[must_use]
    pub const fn predecessor(&self) -> JournalPosition {
        self.predecessor
    }

    /// The folded state of every action the predecessor walked.
    #[must_use]
    pub fn settled(&self) -> &[SettledAttempt] {
        &self.settled
    }

    /// Every attempt the predecessor handed over and nothing settled.
    #[must_use]
    pub fn unresolved(&self) -> &[UnresolvedAttempt] {
        &self.unresolved
    }

    /// The fold for `action`, when this checkpoint carries one.
    ///
    /// Linear rather than indexed because the list is bounded by actions and is
    /// read once per carried key while an open seeds it, not once per append.
    #[must_use]
    pub fn settled_for(&self, action: ActionId) -> Option<&SettledAttempt> {
        self.settled.iter().find(|entry| entry.action() == action)
    }

    /// The frame payload: the magic, then the archived record.
    ///
    /// # Errors
    ///
    /// [`JournalError::Encoding`] if the archive cannot be allocated.
    #[cfg(feature = "wire")]
    pub fn to_payload(&self) -> Result<Vec<u8>, JournalError> {
        let archived =
            lgwks_std::wire::to_bytes::<WireError>(self).map_err(JournalError::Encoding)?;
        let mut payload = Vec::with_capacity(CHECKPOINT_MAGIC.len().saturating_add(archived.len()));
        payload.extend_from_slice(CHECKPOINT_MAGIC);
        payload.extend_from_slice(archived.as_ref());
        Ok(payload)
    }

    /// Whether a frame payload is a checkpoint rather than an event.
    #[must_use]
    pub fn is_payload(payload: &[u8]) -> bool {
        payload.starts_with(CHECKPOINT_MAGIC)
    }

    /// Read a frame payload this journal's writer produced.
    ///
    /// `None` when the payload does not carry `CHECKPOINT_MAGIC`, so it is an
    /// event rather than a checkpoint: the magic is asked once, here, and a
    /// caller cannot decode a payload it did not first classify.
    ///
    /// # Errors
    ///
    /// `Some(Err(JournalError::Encoding))` when the bytes behind the magic are
    /// not the archived record this build writes. A checkpoint is written whole
    /// or not at all, so this is a refusal rather than a truncated read.
    #[cfg(feature = "wire")]
    pub fn from_payload(payload: &[u8]) -> Option<Result<Self, JournalError>> {
        let archived = payload.strip_prefix(CHECKPOINT_MAGIC.as_slice())?;
        match lgwks_std::wire::from_bytes::<Self, WireError>(archived) {
            Ok(checkpoint) => Some(Ok(checkpoint)),
            Err(cause) => {
                let refusal = Err(JournalError::Encoding(cause));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "Continuation::from_payload: returning an error to the caller");
                Some(refusal)
            }
        }
    }
}

#[cfg(test)]
#[cfg(feature = "wire")]
mod tests;
