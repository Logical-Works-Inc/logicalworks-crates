//! The stable-read protocol: an observation is only a change once it has
//! stopped moving.
//!
//! # The failure this module exists for
//!
//! `Observe` answers "has this changed?" by reading a subject and comparing the
//! value against the one it holds. A subject being written *while it is read* is
//! not a different subject: it is the same subject at two instants, and the
//! second read can contain the first write's bytes and not the second's. An
//! observer that reports that as a change hands the substrate a value nobody on
//! the system ever held — a truncated JSON document, a half-copied row, a
//! length that is smaller than the bytes behind it. The change then fires a
//! chain against a document that does not exist.
//!
//! That is the torn read, and the discipline here is the one the commercial
//! automation vendors apply it with: **a reading is admitted only after it has
//! been confirmed stable.** A read that cannot be confirmed is not a change and
//! not an absence either — it is *pending*, which is a third thing, and it is
//! what [`EcsError::UnstableObservation`] reports so the substrate keeps its
//! baseline and re-reads on the next tick rather than committing a value it
//! never saw twice.
//!
//! # The two protocols, and which one is sound
//!
//! 1. **Two reads that agree.** [`read_stable`] takes a reading twice and
//!    requires the two to agree on the subject's reported length, its
//!    modification time, and the digest of its bytes. It is cheap, needs no
//!    cooperation from the writer, and is what a source with no other option
//!    uses.
//! 2. **Write into place, rename over.** A writer that creates a temporary
//!    subject and renames it over the real one makes every open of the real
//!    name see either the old bytes or the new ones, with nothing in between.
//!    This is strictly stronger, it is the writer's discipline rather than the
//!    reader's, and no reader can detect its absence: a subject written in
//!    place by a writer that pauses between two writes passes both reads of
//!    protocol 1 while holding half a document. [`read_stable`] therefore
//!    claims exactly what it proves — *two independent reads agreed* — and no
//!    caller should read it as a promise about the writer.
//!
//! The in-place case has a second window that is worth naming because a real
//! test finds it in seconds: a writer that saves by **truncating** opens a
//! window in which the subject is *stably empty*, and two reads taken in that
//! window agree on nothing at all. `tests/it/stability.rs` asserts that this
//! state is admitted, because it is what the protocol actually proves — the
//! subject did not move between two reads — and the rename protocol is the
//! writer's way of never opening that window.
//!
//! **Not claimed:** that a stable reading is a *valid* one. Deciding that a
//! document parses, that a row is complete, or that a length is plausible is a
//! domain's own job, and it happens after this module returns.
//!
//! # Why the digest is the estate's
//!
//! The comparison is BLAKE3 over the bytes ([`lgwks_std::hash`]), which is this
//! workspace's one content-identity hash: a second digest implementation here
//! would be a second answer to "are these the same bytes", and two answers to
//! that question is the defect [`lgwks_std`]'s `INV-HASH-DETERMINISTIC` exists
//! to prevent. The length and the modification time are checked *before* the
//! digest, not instead of it: they are free, and they are what lets
//! [`Drift`] name the axis that moved rather than reporting "different".
//!
//! # Bounds
//!
//! One attempt takes at most [`MAX_STABILITY_READS`] reads and retains at most
//! two readings at a time, so a subject that never settles costs a fixed amount
//! of work and a fixed amount of memory. A caller that needs to read a subject
//! larger than one [`Reading`] has to say so through its own ceiling; this
//! module bounds the number of reads, not the size of the subject, because
//! "how big may this subject be" is the adapter's declaration rather than this
//! module's guess.
//!
//! [`EcsError::UnstableObservation`]: crate::ecs::EcsError::UnstableObservation
//! [`read_stable`]: crate::stability::read_stable
//! [`Drift`]: crate::stability::Drift
//! [`MAX_STABILITY_READS`]: crate::stability::MAX_STABILITY_READS
//! [`Reading`]: crate::stability::Reading

use std::fmt;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use lgwks_std::hash::{Digest, blake3};

// Defined with the verb whose failure carries them, so the world layer's error
// names `Unstable` without importing this protocol; the paths stay what they were.
pub use crate::verb::{Drift, Unstable};

/// How many reads one stable-read attempt may take before it reports the
/// subject as unsettled.
///
/// Eight rather than two because the common case — a writer part-way through a
/// document that pauses — settles on the next pair, and a protocol that
/// refused the first disagreement would report every concurrent writer as
/// unreadable rather than as unsettled. Bounded, because a subject under a
/// continuous write never agrees with itself and an unbounded loop would spin
/// on it for ever.
pub const MAX_STABILITY_READS: u32 = 8;

/// One read of a subject, with the metadata that decides whether a later read
/// saw the same thing.
///
/// The digest is computed once, at construction, because the comparison this
/// module exists for asks "are these the same bytes" on every read pair and a
/// reader that hashed on demand would either hash twice per pair or carry the
/// loop's own state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// The bytes the read returned, exactly as they arrived.
    bytes: Vec<u8>,
    /// The length the subject reported for itself when it was read.
    ///
    /// `None` for a subject with no length of its own — a socket read, a value
    /// handed over by a transport — so an adapter that reads one is not asked
    /// to invent a length to satisfy the protocol.
    reported: Option<u64>,
    /// The modification time the subject reported, when it has one.
    modified: Option<SystemTime>,
    /// The digest of `bytes`.
    digest: Digest,
    /// Whether the read itself saw the subject move while it was reading.
    ///
    /// A fact about one read rather than about a pair: a read whose subject
    /// grew, shrank or was modified between its first and last byte returned a
    /// state the subject may never have held, and agreeing with another such
    /// read proves nothing about it.
    torn: bool,
}

impl Reading {
    /// A reading of bytes that carry no subject metadata.
    ///
    /// The form an adapter reading from a transport uses: there is no length to
    /// compare and no modification time to lie, so the protocol falls back to
    /// the digest alone and says so through [`Reading::agrees_with`].
    #[must_use]
    pub fn of_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        let bytes = bytes.into();
        let digest = blake3(&bytes);
        Self {
            bytes,
            reported: None,
            modified: None,
            digest,
            torn: false,
        }
    }

    /// A reading of bytes plus the metadata the subject reported alongside them.
    ///
    /// `reported` is the length the subject claims to be, which is not the same
    /// fact as [`Reading::bytes`]'s length: a read cut short by a concurrent
    /// writer disagrees with its own subject's claim, and that disagreement is
    /// what [`Drift::Truncated`] names.
    #[must_use]
    pub fn of_subject(bytes: impl Into<Vec<u8>>, reported: u64, modified: SystemTime) -> Self {
        let mut reading = Self::of_bytes(bytes);
        reading.reported = Some(reported);
        reading.modified = Some(modified);
        reading
    }

    /// The bytes this read returned.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The length the subject reported, when it has one.
    #[must_use]
    pub fn reported(&self) -> Option<u64> {
        self.reported
    }

    /// The modification time the subject reported, when it has one.
    #[must_use]
    pub fn modified(&self) -> Option<SystemTime> {
        self.modified
    }

    /// The digest of the bytes this read returned.
    #[must_use]
    pub const fn digest(&self) -> Digest {
        self.digest
    }

    /// Whether the read returned as many bytes as the subject reported, and
    /// the subject did not move while it was read.
    ///
    /// `true` for a reading with no reported length that no read saw move: a
    /// subject that reports nothing cannot contradict itself.
    #[must_use]
    pub fn is_whole(&self) -> bool {
        // Compared in `u64`, the subject's own width: a read whose length does
        // not fit one cannot equal any length the subject reported.
        !self.torn
            && self
                .reported
                .is_none_or(|reported| u64::try_from(self.bytes.len()) == Ok(reported))
    }

    /// Which axis moved between this reading and `other`, or `None` when the
    /// two are the same subject at the same instant.
    ///
    /// Checked in a fixed order — truncation, then length, then modification
    /// time, then contents — so the answer names the *strongest* evidence first
    /// and two readers of the same pair cannot disagree about which axis it was.
    ///
    /// Truncation comes first because it is the one check that does not compare
    /// two readings at all: a read that returned fewer bytes than its own
    /// subject reported is torn whatever else agrees, including itself on a
    /// later pass. Leaving it to the end would make it unreachable — a pair
    /// whose lengths and digests agree is either whole on both sides or torn on
    /// both — and an unreachable arm is a documented check that never fires.
    #[must_use]
    pub fn agrees_with(&self, other: &Reading) -> Option<Drift> {
        if !self.is_whole() || !other.is_whole() {
            return Some(Drift::Truncated);
        }
        if self.reported != other.reported {
            return Some(Drift::Length);
        }
        if self.modified != other.modified {
            return Some(Drift::Modified);
        }
        if self.digest != other.digest {
            return Some(Drift::Contents);
        }
        None
    }
}

/// Why a stable read did not produce a reading.
///
/// Two arms, because the two are different facts with different repairs: an
/// unsettled subject is one to read again, and an unreadable subject is one to
/// report. Neither is a change, and neither is a value.
#[derive(Debug)]
#[non_exhaustive]
pub enum ReadFailure {
    /// The subject moved while it was being read and never settled within
    /// [`MAX_STABILITY_READS`] reads.
    Unstable(Unstable),
    /// The subject could not be read at all.
    ///
    /// Carries the platform's own report as the cause rather than a rendered
    /// string, so a caller triaging a permission failure or a vanished file
    /// reads the error kind the operating system gave.
    Unreadable {
        /// What the platform reported.
        cause: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl ReadFailure {
    /// The unsettled reading, when the subject was moving.
    ///
    /// `None` for an unreadable subject: there is no reading to report, which
    /// is the distinction a caller polling a file that does not exist yet is
    /// branching on.
    #[must_use]
    pub const fn unstable(&self) -> Option<&Unstable> {
        match *self {
            Self::Unstable(ref unstable) => Some(unstable),
            Self::Unreadable { .. } => None,
        }
    }
}

impl fmt::Display for ReadFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Unstable(unstable) => write!(formatter, "{unstable}"),
            Self::Unreadable { ref cause } => {
                write!(formatter, "the subject could not be read: {cause}")
            }
        }
    }
}

impl std::error::Error for ReadFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Unstable(_) => None,
            Self::Unreadable { ref cause } => Some(cause.as_ref()),
        }
    }
}

impl From<io::Error> for ReadFailure {
    fn from(cause: io::Error) -> Self {
        Self::Unreadable {
            cause: Box::new(cause),
        }
    }
}

/// Read `read` until two readings agree, or the attempt gives up.
///
/// The closure is called at least twice: one reading is not evidence of
/// stability, and a single-read "confirmation" is precisely the check this
/// module replaces. Each call must perform an **independent** read of the
/// subject — not return a cached value — because two calls that return the same
/// bytes without touching the subject prove nothing about whether the subject
/// is moving.
///
/// A subject that settles returns the second of the two agreeing readings, so
/// the value handed back is one the caller has seen twice. A subject that never
/// settles returns [`ReadFailure::Unstable`] naming the axis that kept moving.
///
/// # Errors
///
/// [`ReadFailure::Unreadable`] when a read could not be performed at all, and
/// [`ReadFailure::Unstable`] when the subject moved on every comparison.
pub fn read_stable<R>(mut read: R) -> Result<Reading, ReadFailure>
where
    R: FnMut() -> Result<Reading, ReadFailure>,
{
    let mut held = read()?;
    let mut reads: u32 = 1;
    let mut last_move: Option<(Drift, Digest, Digest)> = None;
    // Bounded by the declared constant rather than by a condition on the
    // subject: a subject under a continuous write never agrees with itself, and
    // the loop's exit has to be a property of this module rather than of what
    // the writer happens to be doing.
    while reads < MAX_STABILITY_READS {
        let observed = read()?;
        reads = reads.saturating_add(1);
        match held.agrees_with(&observed) {
            None => return Ok(observed),
            Some(axis) => {
                last_move = Some((axis, held.digest(), observed.digest()));
                held = observed;
            }
        }
    }
    let refusal = match last_move {
        Some((axis, before, after)) => Err(ReadFailure::Unstable(Unstable::new(
            reads, axis, before, after,
        ))),
        // Reachable only if the declared bound were one, in which case there is
        // no pair to compare and nothing moved as far as this module can tell.
        None => Err(ReadFailure::Unstable(Unstable::new(
            reads,
            Drift::Contents,
            held.digest(),
            held.digest(),
        ))),
    };
    lgwks_std::trace::debug!(
        error = ?refusal.as_ref().err(),
        "read_stable: returning an error to the caller"
    );
    refusal
}

/// Read a file until two reads of it agree.
///
/// The blocking read runs on the estate's blocking pool rather than on the
/// caller's thread, because every `Observe::poll` in this crate is polled on a
/// runtime's executor and a filesystem read parked on it is a stalled tick for
/// every task beside it.
///
/// # Errors
///
/// [`ReadFailure::Unreadable`] when the file cannot be opened or read, and
/// [`ReadFailure::Unstable`] when it never settled.
pub async fn read_stable_file(path: &Path) -> Result<Reading, ReadFailure> {
    let owned = path.to_path_buf();
    // The whole loop runs inside the blocking task, so each pass through
    // `read_file` is a fresh open of the path. A loop that read once and then
    // re-checked its own bytes would return a settled value it never observed
    // twice, which is the defect this module exists to remove.
    // The bounded form: a full queue or a closed pool is a typed refusal the
    // caller reads as an unreadable subject, never an unbounded backlog.
    let blocking = match lgwks_std::task::try_spawn_blocking(move || settle_file(&owned)) {
        Ok(handle) => handle,
        Err(refused) => {
            let refusal = Err(ReadFailure::Unreadable {
                cause: Box::new(refused),
            });
            lgwks_std::trace::debug!(
                error = ?refusal.as_ref().err(),
                "read_stable_file: returning an error to the caller"
            );
            return refusal;
        }
    };
    blocking.await
}

/// How many bytes one `read` call asks a file for.
const READ_CHUNK: usize = 65_536;

/// One pass: the bytes, and the metadata the subject reported with them.
///
/// The subject is opened once, and its metadata is taken from that handle both
/// before and after the bytes are read, so every fact in the reading is about
/// the one file the bytes came from: a writer that renames a new file over the
/// path mid-read leaves this handle on the old file, whose bytes it did hold.
///
/// A read that is not one `read` call can straddle a rewrite in place: the
/// first call returns the old document up to its end, the writer truncates and
/// writes a longer one, and the next call returns the new document's tail. The
/// result is a state the file never held, as long as the new document, and on
/// a filesystem that stamps modification times to the second (HFS+, a macOS
/// RAM disk) its length and time match a stat taken afterwards. Two reads that
/// straddle the same way then agreed, and the pair was admitted. A run of this
/// crate's suite with its temporary files on a RAM disk found exactly that:
/// `{"revision":1}"note":"rewritten"}`, two documents' bytes spliced. So a
/// read that saw the subject's end move, or whose length or time moved between
/// the two stats, is torn, whatever another read agrees with.
fn read_file(path: &Path) -> Result<Reading, ReadFailure> {
    let mut file = std::fs::File::open(path)?;
    let before = file.metadata()?;
    let (bytes, grew) = read_to_end_watching(&mut file)?;
    let after = file.metadata()?;
    let mut reading = Reading::of_bytes(bytes);
    reading.reported = Some(after.len());
    // A platform that keeps no modification time reports `Unsupported` here, and
    // the reading carries that absence rather than a stand-in instant: both
    // passes then carry the same `None`, so the length and the digest decide,
    // and no reading ever claims a time the filesystem did not state.
    reading.modified = stamp_of(&after);
    // Any one of the three moving is a tear; they are named so a reader sees
    // which axis each test drives.
    let length_moved = before.len() != after.len();
    let time_moved = stamp_of(&before) != reading.modified;
    reading.torn = [grew, length_moved, time_moved].contains(&true);
    Ok(reading)
}

/// The modification time `metadata` states, or `None` when the platform keeps
/// none.
///
/// The absence is traced rather than swallowed: it is a fact about the
/// platform that decides which axes of the comparison can fire.
fn stamp_of(metadata: &std::fs::Metadata) -> Option<SystemTime> {
    match metadata.modified() {
        Ok(stamp) => Some(stamp),
        Err(unsupported) => {
            lgwks_std::trace::debug!(
                error = ?unsupported,
                "stamp_of: no modification time; the length and the digest decide"
            );
            None
        }
    }
}

/// Read `source` to its end, and say whether its end moved while it was read.
///
/// A regular file returns fewer bytes than a `read` asked for only at its end,
/// so a short read followed by one that returned more bytes saw the end of the
/// file move: the file grew between the two calls. A full read followed by more
/// is an ordinary file larger than one chunk.
///
/// The check is the safe direction on a filesystem that returns short reads
/// mid-file: such a read is refused as torn and the pass is taken again, never
/// admitted.
fn read_to_end_watching(source: &mut impl io::Read) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut chunk = vec![0_u8; READ_CHUNK];
    let mut came_up_short = false;
    let mut grew = false;
    loop {
        let count = match source.read(&mut chunk) {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                let refusal = Err(error);
                lgwks_std::trace::debug!(
                    error = ?refusal.as_ref().err(),
                    "read_to_end_watching: returning an error to the caller"
                );
                return refusal;
            }
        };
        if count == 0 {
            return Ok((bytes, grew));
        }
        grew = grew || came_up_short;
        came_up_short = count < chunk.len();
        let Some(read) = chunk.get(..count) else {
            let refusal = Err(io::Error::other(
                "a read reported more bytes than the buffer it was given holds",
            ));
            lgwks_std::trace::debug!(
                error = ?refusal.as_ref().err(),
                "read_to_end_watching: returning an error to the caller"
            );
            return refusal;
        };
        bytes.extend_from_slice(read);
    }
}

/// The stable-read loop over one path, called from inside the blocking task.
fn settle_file(path: &Path) -> Result<Reading, ReadFailure> {
    read_stable(|| read_file(path))
}

#[cfg(test)]
mod tests;
