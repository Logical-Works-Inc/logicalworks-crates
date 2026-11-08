//! A durable journal over a record the consumer defines.
//!
//! [`FileJournal`](crate::journal::FileJournal) persists one vocabulary, the
//! effect ladder, because its fence, its recovery fold and its checkpoint are all
//! statements about that ladder. A consumer with its own ordered history wants the
//! part underneath: an append that states the tail it believes, a chain that
//! commits to every record before it, and an acknowledgment that is true after a
//! kill. [`RecordJournal`] is that part, on the same frame grammar, the same
//! storage owner thread, the same chain function and the same seal step, so there
//! is one definition of a torn tail in this crate and not two.
//!
//! # What a frame holds
//!
//! A frame is the shared grammar's: a length prefix, a payload, and the head that
//! payload commits to. The payload opens with a one-byte tag, because a consumer's
//! archived bytes can spell anything. An untagged log would have to tell a
//! checkpoint from a record by looking for a magic prefix inside bytes it does not
//! own, and a record that happened to begin with it would be read as a seal.
//!
//! # Memory is the index, not the history
//!
//! The handle keeps one offset per frame, at most [`MAX_JOURNAL_EVENTS`] of them,
//! and no decoded record. [`RecordJournal::page`] and [`RecordJournal::replay`]
//! read from their own descriptor and verify the chain as they go, so a fold over
//! a full journal costs its largest frame rather than its whole log.
//!
//! # Continuation
//!
//! [`RecordJournal::continue_with`] seals this file with a checkpoint record the
//! consumer folded, and opens the successor from the same bytes: the seal frame is
//! identical in the predecessor's last place and the successor's first. Sequence
//! numbers carry across, so the checkpoint is the entry the sequence after the
//! predecessor's tail names and a cursor never sees a gap inside one chain. The
//! bytes decide who is authoritative: a successor holding that frame whole is the
//! journal and the predecessor refuses with [`JournalError::Superseded`]. A seal
//! with no such successor never committed, was never acknowledged, and is taken
//! back on the next open.
//!
//! # One journal per tenant
//!
//! A journal is one chain under one lock. Tenants take a path each; the file name
//! is theirs to derive, so two tenants never share a chain, a lock or a ceiling.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use lgwks_std::wire::rkyv::de::pooling::Pool;
use lgwks_std::wire::rkyv::rancor::Strategy;
use lgwks_std::wire::{Archive, Deserialize, Serialize, WireError};

use super::continuation::{
    ContinuationPolicy, SealPause, is_generation_path, refuse_ambiguous_base, successor_path,
};
use super::file::{MAX_GENERATION_WALK, SealOutcome};
use super::frame::SaturatingFrom;
use super::owner::StorageOwner;
use super::{
    Corruption, CorruptionKind, JournalError, JournalLimitKind, JournalPosition, MAX_JOURNAL_BYTES,
    MAX_JOURNAL_EVENTS,
};

mod read;
mod replay;
mod scan;
#[cfg(test)]
mod sim_records;
#[cfg(test)]
mod sim_records_crash;
mod write;

pub use replay::{RecordEntry, RecordReplay};
use scan::{SealAt, Stop, scan};

/// The first payload byte of a consumer's record.
const RECORD_TAG: u8 = 0x01;

/// The first payload byte of a seal: the tag, the predecessor's sequence and head,
/// then the checkpoint record.
const SEAL_TAG: u8 = 0x02;

/// The bound a record must satisfy to be persisted.
///
/// `lgwks_std::wire`'s checked decoder names the deserializer strategy it uses
/// rather than leaving it to inference, and that strategy type is a detail of
/// the facade. Naming the bound once here means a signature reads as `T: Durable`,
/// something a consumer can see and a doc can explain, instead of four levels of
/// rkyv generics that would change with the release.
///
/// # Examples
///
/// ```
/// use lgwks_bot::journal::Durable;
///
/// fn round_trip<T: Durable>(value: &T) -> Result<T, lgwks_std::wire::WireError> {
///     T::from_record(&value.to_record()?)
/// }
/// assert_eq!(round_trip(&7_u64).ok(), Some(7));
/// ```
pub trait Durable: Sized {
    /// Archive this value the way a store frames it.
    ///
    /// # Errors
    ///
    /// [`WireError`] when the value does not archive.
    fn to_record(&self) -> Result<Vec<u8>, WireError>;

    /// Rebuild this value from the bytes a store framed.
    ///
    /// # Errors
    ///
    /// [`WireError`] when the bytes are not this type's archive.
    fn from_record(bytes: &[u8]) -> Result<Self, WireError>;
}

impl<T> Durable for T
where
    T: Archive,
    for<'a> T: Serialize<
        Strategy<
            lgwks_std::wire::rkyv::ser::Serializer<
                lgwks_std::wire::AlignedVec,
                lgwks_std::wire::rkyv::ser::allocator::ArenaHandle<'a>,
                lgwks_std::wire::rkyv::ser::sharing::Share,
            >,
            WireError,
        >,
    >,
    for<'a> <T as Archive>::Archived: Deserialize<T, Strategy<Pool, WireError>>
        + lgwks_std::wire::rkyv::bytecheck::CheckBytes<
            Strategy<
                lgwks_std::wire::rkyv::validation::Validator<
                    lgwks_std::wire::rkyv::validation::archive::ArchiveValidator<'a>,
                    lgwks_std::wire::rkyv::validation::shared::SharedValidator,
                >,
                WireError,
            >,
        >,
{
    fn to_record(&self) -> Result<Vec<u8>, WireError> {
        lgwks_std::wire::to_bytes::<WireError>(self).map(|bytes| bytes.as_ref().to_vec())
    }

    fn from_record(bytes: &[u8]) -> Result<Self, WireError> {
        lgwks_std::wire::from_bytes::<T, WireError>(bytes)
    }
}

/// An ordered, hash-chained, crash-surviving log of a consumer's records.
///
/// Every acknowledged append is written and synced before its
/// [`DurableAck`](crate::journal::DurableAck) exists, so the
/// [`DurabilityPromise::ProcessCrash`](crate::journal::DurabilityPromise::ProcessCrash)
/// it carries is earned per append. A second opener is refused with
/// [`JournalError::Locked`] before a byte is read. Committed bytes that do not
/// re-derive the chain are refused, never trimmed. The bytes of an append a killed
/// writer never finished are taken back, and
/// [`RecordJournal::torn_tail_repaired`] says so.
///
/// Validation of what a record means is the consumer's: this journal orders and
/// keeps bytes, and refuses only a stale tail.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
/// use lgwks_bot::journal::RecordJournal;
///
/// let path = std::env::temp_dir().join(format!("record-doc-type-{}", std::process::id()));
/// # drop(std::fs::remove_file(&path));
/// let mut journal = RecordJournal::<u64>::open(&path)?;
/// let ack = journal.compare_and_append(journal.tail(), &7)?;
/// assert_eq!(ack.position(), journal.tail());
/// drop(journal);
/// let reopened = RecordJournal::<u64>::open(&path)?;
/// assert_eq!(reopened.committed(), 1);
/// # drop(reopened);
/// # drop(std::fs::remove_file(&path));
/// # Ok(())
/// # }
/// ```
pub struct RecordJournal<R> {
    /// Where the journal lives.
    path: PathBuf,
    /// The thread that holds the file and the poison latch.
    storage: StorageOwner<(), SealOutcome>,
    /// A read-only descriptor, so the fence can read the length without the owner.
    view: File,
    /// Where each entry's frame begins, in commit order.
    offsets: Vec<u64>,
    /// The last committed position, including this handle's own seal.
    position: JournalPosition,
    /// What this file's first frame chains from.
    base: JournalPosition,
    /// Whether this file's first frame is a predecessor's seal.
    carried: bool,
    /// Byte length of the acknowledged entries; a seal this handle wrote lies past it.
    disk_len: u64,
    /// Whether opening took back an append that was never acknowledged.
    repaired: bool,
    /// Where this handle continues, when it was opened to.
    policy: Option<ContinuationPolicy>,
    /// Where this handle's own seal sent the authoritative journal.
    sealed_by: Option<PathBuf>,
    /// The boundary an armed continuation stops at.
    pause: Option<SealPause>,
    /// The record type, with no bearing on whether the handle is `Send`.
    record: PhantomData<fn() -> R>,
}

impl<R> core::fmt::Debug for RecordJournal<R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RecordJournal")
            .field("path", &self.path)
            .field("committed", &self.offsets.len())
            .field("disk_len", &self.disk_len)
            .field("position", &self.position)
            .field("sealed_by", &self.sealed_by)
            .finish()
    }
}

impl<R: Durable> RecordJournal<R> {
    /// Open the journal at `path`, creating it when it does not exist.
    ///
    /// This handle stops at the ceilings rather than continuing past them.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::{DurabilityPromise, RecordJournal};
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-open-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let journal = RecordJournal::<u64>::open(&path)?;
    /// assert_eq!(journal.durability(), DurabilityPromise::ProcessCrash);
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`JournalError::Locked`] when another writer holds the file,
    /// [`JournalError::Corrupt`] when committed bytes are refused,
    /// [`JournalError::Superseded`] when a successor is authoritative,
    /// [`JournalError::CapacityExceeded`] when the file is over a ceiling, and
    /// [`JournalError::Storage`] when the device refuses.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, JournalError> {
        Self::open_impl(path.as_ref(), None)
    }

    /// Open a journal that continues at `policy`'s trigger points.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::{ContinuationPolicy, RecordJournal};
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-cont-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let journal = RecordJournal::<u64>::open_continuing(&path, ContinuationPolicy::declared())?;
    /// assert!(journal.watermark().continues());
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Whatever [`RecordJournal::open`] reports.
    pub fn open_continuing(
        path: impl AsRef<Path>,
        policy: ContinuationPolicy,
    ) -> Result<Self, JournalError> {
        Self::open_impl(path.as_ref(), Some(policy))
    }

    /// Open the journal at `path`, following its continuations to the live one.
    ///
    /// What a restarting process wants: it holds the path its host recorded, and
    /// the file there may be many sealed predecessors old. The walk is bounded by
    /// [`MAX_GENERATION_WALK`].
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::{ContinuationPolicy, RecordJournal};
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-active-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let journal = RecordJournal::<u64>::open_active(&path, ContinuationPolicy::declared())?;
    /// assert_eq!(journal.committed(), 0);
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Whatever the last journal reports, and [`JournalError::Superseded`] naming
    /// the next successor when the walk ran out of generations.
    pub fn open_active(
        path: impl AsRef<Path>,
        policy: ContinuationPolicy,
    ) -> Result<Self, JournalError> {
        let mut here = path.as_ref().to_path_buf();
        for _ in 0..MAX_GENERATION_WALK {
            match Self::open_impl(&here, Some(policy)) {
                Err(JournalError::Superseded { path: next }) => here = PathBuf::from(next),
                answer => return answer,
            }
        }
        let next = successor_path(&here).display().to_string();
        let refusal = Err(JournalError::Superseded { path: next });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "RecordJournal::open_active: the walk ran out of generations");
        refusal
    }

    /// The one implementation every constructor takes.
    fn open_impl(path: &Path, policy: Option<ContinuationPolicy>) -> Result<Self, JournalError> {
        refuse_ambiguous_base(path)?;
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(path)
            .map_err(JournalError::Storage)?;
        // The lock comes before the first read: the replay and the repair belong to
        // the lock's owner alone.
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => JournalError::Locked {
                path: path.display().to_string(),
            },
            std::fs::TryLockError::Error(io) => JournalError::Storage(io),
        })?;
        let length = file.metadata().map_err(JournalError::Storage)?.len();
        bound_bytes(length, 0)?;
        file.seek(SeekFrom::Start(0))
            .map_err(JournalError::Storage)?;
        let generation = is_generation_path(path);
        let scanned = scan::<R>(&mut BufReader::new(&mut file), generation)?;
        // A generation's name is a claim that a predecessor sealed into it. With no
        // seal at its head, treating it as empty would start a second chain at the
        // genesis beside the one it continues.
        if generation && !scanned.carried {
            return refuse(
                0,
                CorruptionKind::Sealed,
                "a generation file opens with no seal",
            );
        }
        let next = successor_path(path);
        if let Some(seal) = scanned.seal.as_ref()
            && successor_holds(&mut file, seal, &next)?
        {
            let refusal = Err(JournalError::Superseded {
                path: next.display().to_string(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "RecordJournal: a successor is authoritative");
            return refusal;
        }
        let (keep, position, repaired) = match (scanned.seal.as_ref(), scanned.stop) {
            // A seal no successor holds was never acknowledged. Rolling it back is
            // the same repair a torn tail gets, and it costs nothing the seal's
            // caller was ever told.
            (Some(seal), _) => (seal.offset, seal.before, true),
            (None, Stop::Complete(at)) => (at, scanned.tail, false),
            (None, Stop::Torn(at)) => (at, scanned.tail, true),
        };
        if repaired {
            file.set_len(keep).map_err(JournalError::Storage)?;
            file.sync_all().map_err(JournalError::Storage)?;
        }
        let storage = StorageOwner::spawn(file, (), false).map_err(JournalError::Storage)?;
        let view = File::open(path).map_err(JournalError::Storage)?;
        Ok(Self {
            path: path.to_path_buf(),
            storage,
            view,
            offsets: scanned.offsets,
            position,
            base: scanned.base,
            carried: scanned.carried,
            disk_len: keep,
            repaired,
            policy,
            sealed_by: None,
            pause: None,
            record: PhantomData,
        })
    }
}

/// Whether the successor at `next` holds this file's seal frame whole.
///
/// The one fact that decides who is authoritative, asked of the bytes. The seal
/// frame is written identically into both files, so anything short of that frame
/// at the successor's head is a continuation that never committed.
fn successor_holds(file: &mut File, seal: &SealAt, next: &Path) -> Result<bool, JournalError> {
    let Ok(held) = File::open(next) else {
        return Ok(false);
    };
    let mut frame = vec![0u8; usize::saturating_from(seal.length)];
    file.seek(SeekFrom::Start(seal.offset))
        .map_err(JournalError::Storage)?;
    file.read_exact(&mut frame).map_err(JournalError::Storage)?;
    let mut theirs = Vec::with_capacity(frame.len());
    held.take(seal.length)
        .read_to_end(&mut theirs)
        .map_err(JournalError::Storage)?;
    Ok(theirs == frame)
}

/// Refuse a count of entries past the event ceiling.
fn bound_events(held: usize, adding: usize) -> Result<(), JournalError> {
    let requested = held.saturating_add(adding);
    if requested > MAX_JOURNAL_EVENTS {
        let refusal = Err(JournalError::CapacityExceeded {
            resource: JournalLimitKind::Events,
            limit: u64::saturating_from(MAX_JOURNAL_EVENTS),
            requested: u64::saturating_from(requested),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "RecordJournal: past the event ceiling");
        return refusal;
    }
    Ok(())
}

/// Refuse a length past the byte ceiling.
fn bound_bytes(held: u64, adding: usize) -> Result<(), JournalError> {
    let requested = held.saturating_add(u64::saturating_from(adding));
    if requested > MAX_JOURNAL_BYTES {
        let refusal = Err(JournalError::CapacityExceeded {
            resource: JournalLimitKind::Bytes,
            limit: MAX_JOURNAL_BYTES,
            requested,
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "RecordJournal: past the byte ceiling");
        return refusal;
    }
    Ok(())
}

/// A named refusal of a frame's bytes.
fn refuse<T>(at: u64, kind: CorruptionKind, why: &'static str) -> Result<T, JournalError> {
    let refusal = Err(JournalError::Corrupt(Box::new(Corruption::new(at, kind))));
    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), why, "RecordJournal: refused its own bytes");
    refusal
}
