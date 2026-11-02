//! The durable, file-backed per-step record store behind a resumable run.
//!
//! [`HostBuilder::run_store`] installs one
//! of these. A step marked [`Scope::remember`]
//! consults it before running its future and appends to it after; that is the
//! whole of the durability claim, and it covers exactly the steps an author asked
//! to be durable.
//!
//! # Why this is not `journal::FileJournal`
//!
//! The *frame grammar* is the estate's and is shared verbatim: [`journal::frame`]
//! holds the length prefix, the 32-byte head, the torn-tail scan and the refusal
//! of a frame no writer produces, and both this store and `journal::file` call
//! those same functions. What cannot be reused is the *record*: `journal::file`
//! frames [`EffectEvent`] and its head chains effect
//! positions, so a step record smuggled through it would claim to be an effect
//! event and chain against a sequence that has nothing to do with steps. So this
//! module states its own record and reuses the frame grammar; the encoding is
//! `lgwks_std::wire` in both. It shares `journal::file`'s storage-owner thread
//! too, so a durable step's `sync_all` costs the device, not the executor.
//!
//! # What a record is
//!
//! One record is the run, the step's key digest, the step's path, the owning
//! tenant, and the archived bytes of the value the step returned. The path is
//! stored beside the digest so an undecodable value is attributable; the digest
//! is what the lookup is on, so two steps cannot read each other's record even
//! if an author gives them the same name.
//!
//! # Bounds
//!
//! Three ceilings, all declared as constants and all reported as typed refusals
//! rather than enforced by truncation: per-record bytes, records per run, and
//! total file bytes. A refusal never deletes, compacts or rewrites a committed
//! record — INV-BOT-14's rule, applied to this store.

use std::collections::HashMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lgwks_std::hash::{Digest, Hasher};

use crate::journal::frame::SaturatingFrom;
#[cfg(feature = "sealed")]
use crate::sealed::keys::KeyProvider;

use crate::effect::RunId;
use crate::journal::owner::{self, Stage, StorageGate, StorageOwner};
use crate::script::run_store::{Appended, RunRecords, StagedRecord};
use crate::script::{FlowError, StepKey};

use super::definition::{DefinitionIdentity, Drift, STORE_FORMAT};

/// The most bytes one archived step record may occupy.
///
/// A step that returns more than this is refused rather than written: the value
/// belongs in the effect journal or in the caller's own storage, not in a record
/// whose whole purpose is to be replayed into memory on resume.
pub const MAX_RECORD_BYTES: usize = 256 * 1024;

/// The most records one run's store retains.
pub const MAX_RECORDS_PER_RUN: u64 = 65_536;

/// The most bytes one run's store file may hold.
pub const MAX_STORE_BYTES: u64 = 64 * 1024 * 1024;

/// The constant prefix of every run store's file.
///
/// A file that does not begin with it is not this format and is refused rather
/// than scanned: reading a foreign file as records is how a caller is handed a
/// value nobody wrote. The last byte of the header is the format's version, so a
/// future change is a refusal rather than a misreading. `\x01` was the record
/// without a [`DefinitionIdentity`]; a file in that format still *is* a run
/// store, so it is refused as [`StoreError::FormatVersion`] naming both versions
/// rather than as `NotAStore`, which would claim the bytes were never this
/// store's. See [`check_format_version`] for the rule and the `definition` module
/// for why reading one as the unversioned identity would be the worse answer.
const STORE_MAGIC: &[u8; 16] = b"lgwks-runstore\x00\x00";

/// The header as this crate writes it: the constant prefix and the version.
///
/// Built once so the writer and the reader cannot disagree about which byte is
/// which, which is the only thing a hand-written `OpenOptions` chain would get
/// wrong.
const STORE_HEADER: [u8; 16] = {
    let mut header = *STORE_MAGIC;
    header[15] = STORE_FORMAT;
    header
};

/// The index of the version byte in [`STORE_HEADER`].
///
/// Spelled as a name because the writer, the reader and the refusal all have to
/// agree on *which* byte carries the version, and a literal `15` in three places
/// is three chances for one of them to mean a different byte. It is the last
/// byte, which is what leaves the first [`STORE_MAGIC`] free of version bytes.
const VERSION_BYTE: usize = STORE_MAGIC.len() - 1;

/// Refuse a header whose format this build does not read.
///
/// Two refusals, and the order is the argument: a header whose *first* bytes are
/// not this store's magic was never a run store ([`StoreError::NotAStore`]),
/// while a header whose first bytes match and whose version byte does not is a
/// run store this build cannot read ([`StoreError::FormatVersion`], naming both
/// versions). Reporting the second as the first would tell an operator their
/// data was never theirs, when in fact it was written by an earlier version of
/// this very crate — which is the one conclusion that must never be drawn from a
/// refusal, because it is what makes someone delete the file.
///
/// [`STORE_MAGIC`] is constant across versions and the version byte is not, so
/// "the first bytes match" is exactly "this is some version of this format",
/// and the one comparison below is what keeps those two facts from collapsing
/// into one check.
///
/// # Errors
///
/// [`StoreError::NotAStore`] when the magic does not match, and
/// [`StoreError::FormatVersion`] naming the found and expected versions when it
/// does.
fn check_format_version(header: [u8; STORE_HEADER.len()]) -> Result<(), StoreError> {
    if header[..VERSION_BYTE] != STORE_MAGIC[..VERSION_BYTE] {
        let refusal = Err(StoreError::NotAStore);
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "check_format_version: returning an error to the caller");
        return refusal;
    }
    let found = header[VERSION_BYTE];
    if found == STORE_FORMAT {
        return Ok(());
    }
    Err(StoreError::FormatVersion {
        found,
        expected: STORE_FORMAT,
    })
}

/// The chain digest a store starts from: 32 zero bytes, hashed through the same
/// framing as any record so the first head is a real chain step and not a
/// constant every file shares.
fn genesis_head() -> Digest {
    let mut hasher = Hasher::new();
    hasher.write_framed(b"lgwks-runstore/genesis");
    hasher.finalize()
}

mod error;
mod record;
mod records;
#[cfg(feature = "sealed")]
mod sealed_impl;
#[cfg(feature = "sealed")]
mod sealed_shred;
#[cfg(test)]
mod tests;
pub use error::{StoreError, StoreLimitKind};
use record::{
    Stored, StoredDefinition, count_approval, hex_of, refusal, replay, store_full, tail_head,
};

// ── The store ───────────────────────────────────────────────────────────────
use records::SealedStoreOpen;

/// One durable, file-backed set of per-step records.
///
/// Cloning shares the file and the index through an `Arc`, so the host's handles
/// see one store rather than one per clone — the rule the admission budget
/// follows, and for the same reason: a second store over one file would be a
/// second writer.
#[derive(Clone)]
pub struct RunStore {
    /// Where the store lives, and the index every clone shares.
    inner: Arc<StoreInner>,
}

/// The one state every clone of a store handle shares, plus the fault the
/// read-failure injector arms.
struct StoreInner {
    /// The thread that holds the file, and the one door an append reaches the
    /// disk through. Its writes are `write_all` plus `sync_all`, so they belong
    /// off the executor — INV-BOT-50.
    owner: StorageOwner<Arc<Mutex<Index>>, Appended>,
    /// Where the file is, for diagnostics.
    path: PathBuf,
    /// The index every clone reads, and the one the owner folds into: per run, per
    /// step key, the record.
    index: Arc<Mutex<Index>>,
    /// Set once by [`RunStore::fail_next_index_read`], and taken by the first
    /// *record* read that observes it.
    ///
    /// `AtomicBool` rather than a `Mutex<Option<..>>` so arming and firing are
    /// two words wide and a read takes no second lock: the read path must not gain
    /// a lock acquisition purely so a fault can be scheduled, and the injection is
    /// a bounded one-shot — `swap`, not `load` — so two racing reads cannot both
    /// take it and the injector cannot leave the store armed forever for a caller
    /// who never expected a fault.
    ///
    /// Taken by [`Self::step_readable`] alone, and never by [`Self::index`]: the
    /// host's own admission reads (`tenant_of`, `definition_of`) run before the
    /// body and would otherwise spend the fault on themselves, which would test
    /// the host's pre-flight rather than the durable step's replay check. A fault
    /// that fires on the wrong read is a fault that proved the wrong thing.
    unreadable: AtomicBool,
    /// Who mints, wraps, opens and destroys this file's data keys, when the
    /// store was opened sealed. `None` for every plain open, which leaves
    /// every existing behaviour unchanged.
    #[cfg(feature = "sealed")]
    sealed: Option<Arc<dyn KeyProvider>>,
}

/// The index, and the committed length it accounts for.
#[derive(Debug)]
struct Index {
    /// Per run: the owning tenant and its steps.
    runs: HashMap<RunId, RunIndex>,
    /// The file length every indexed record accounts for.
    ///
    /// Held beside the index rather than recomputed from it, because the append
    /// fence compares *bytes* against the device and a sum over the index would
    /// have to reproduce the frame overhead exactly — a second definition of the
    /// file's length that could drift from the first.
    committed: u64,
    /// The chain head the next frame follows.
    ///
    /// Carried beside the index because it cannot be recovered from it: a head is
    /// over the *archived* record, so recomputing one would re-archive a value
    /// and risk hashing something other than the bytes on the disk.
    tail: Digest,
    /// Per tenant, how many parked approvals are undecided.
    ///
    /// Maintained at fold time — replay and every committed append — so the
    /// park ceiling is one probe rather than a scan: a park that scanned the
    /// whole store would cost the store's size on every park. A decision
    /// decrements, and a lapse records a decision, so only live approvals are
    /// counted.
    pending: HashMap<String, u64>,
    /// Per run and step key hex: the bytes of a record written in the batch
    /// now flushing, not yet folded into `runs`.
    ///
    /// The duplicate check reads it beside `runs`: without it, two appends to
    /// one key in one batch both found the key absent, both wrote, and both
    /// answered `Recorded`, so two racing decisions each saw their own bytes
    /// and both won. The first staged write wins; the settle moves it into
    /// `runs`. A failed flush leaves its entries here harmlessly, because
    /// `committed` already moved and the length fence refuses every later
    /// append.
    staged: HashMap<(RunId, String), Vec<u8>>,
    /// Frames committed through this file, which is the next sealed record's
    /// ordinal: advanced by every sealed frame and every tombstone, so a
    /// tombstone's back-reference matches the replay's file order on every
    /// reopen.
    #[cfg(feature = "sealed")]
    frames: u64,
    /// Shredded steps by run and step key hex: the burn set. A re-append
    /// under a burned slot is refused, and a read folds to erased, so a
    /// restart past a shred cannot duplicate the step the index forgot. The
    /// index keeps the key identity and the expiry, never the value.
    #[cfg(feature = "sealed")]
    erased: HashMap<(RunId, String), ErasedSlot>,
    /// Per-run shredded counts and owning tenants, so the counts and the
    /// ownership answer read one probe rather than scanning the burn set.
    #[cfg(feature = "sealed")]
    erased_runs: HashMap<RunId, (u64, String)>,
}

/// One run's records.
#[derive(Debug)]
struct RunIndex {
    /// The tenant that minted the run, and the only one allowed to read it.
    tenant: String,
    /// The definition every record under this run was written under.
    ///
    /// One per run rather than one per record because a run is one attempt to
    /// execute one definition: a record written under a different definition
    /// than the run's first is not a step that changed, it is a different run
    /// that arrived under the same id, and the first record is what the rest are
    /// compared against.
    definition: DefinitionIdentity,
    /// Per step key hex: the step's path and its archived value.
    steps: HashMap<String, Held>,
}

/// One step's record as the index holds it.
#[derive(Debug)]
struct Held {
    /// The step path, kept so a decode failure is attributable.
    path: String,
    /// The archived value, kept so a resume does not re-read the file per step.
    /// Empty for sealed steps: the value lives only in the sealed frame, and
    /// the index keeps the envelope material instead.
    bytes: Vec<u8>,
    /// What seals this step, when the store was opened sealed. `None` for
    /// every plain record.
    #[cfg(feature = "sealed")]
    sealed: Option<SealedStep>,
}

/// Sealed types live in [`sealed_impl`](sealed_impl): the envelope material,
/// the write carrier, the public append and the burn-set slot. Re-exported
/// here so the index, the staged append and the record module keep naming
/// the same paths.
#[cfg(feature = "sealed")]
pub use sealed_impl::SealedAppend;
#[cfg(feature = "sealed")]
pub(super) use sealed_impl::{ErasedSlot, SealedStep, SealedWrite};

impl fmt::Debug for RunStore {
    /// The path and the indexed run count. The values are not rendered: a report
    /// or a log line must not carry a step's output into a reader that asked
    /// where the store is.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A poisoned index renders as `None` rather than as an empty store, so the
        // debug line never claims a count nobody read.
        let (runs, committed) = match self.inner.index.lock() {
            Ok(index) => (Some(index.runs.len()), Some(index.committed)),
            Err(_) => (None, None),
        };
        formatter
            .debug_struct("RunStore")
            .field("path", &self.inner.path)
            .field("runs", &runs)
            .field("committed", &committed)
            .finish()
    }
}

impl RunStore {
    /// Open (or create) the store at `path`.
    ///
    /// The file is created with a header if it does not exist, and replayed if
    /// it does. A torn final record is dropped, because an interrupted append is
    /// a prefix cut short and was never anyone's answer; every earlier record is
    /// retained.
    ///
    /// # Errors
    ///
    /// [`StoreError::Storage`] when the device refuses, or when a torn tail
    /// cannot be trimmed; [`StoreError::NotAStore`] when `path` exists and is
    /// not a run store; [`StoreError::Corrupt`] when a committed frame does not
    /// follow from the ones before it.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        Self::open_impl(path.into(), false, None)
    }

    /// Open a store whose device does not answer a flush until its gate is
    /// released.
    ///
    /// A fault injector, and public for the reason
    /// [`FileJournal::open_with_stalled_storage`](crate::journal::FileJournal::open_with_stalled_storage)
    /// is: "what does this run do while its record store has stopped answering" is
    /// a question an operator has to be able to ask on a real process, and a probe
    /// that only exists inside the crate's own test binary cannot answer it. The
    /// bytes written are real and the frames are the ones the store always writes;
    /// only the device's answer is held back, which is exactly what a stalled
    /// device does. Nothing is acknowledged until the release.
    ///
    /// # Errors
    ///
    /// As [`RunStore::open`].
    pub fn open_with_stalled_device(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        Self::open_impl(path.into(), true, None)
    }

    /// One constructor for both: a stalled device is a property of the storage
    /// owner, not a second way to open a store.
    ///
    /// # Errors
    ///
    /// As [`RunStore::open`].
    fn open_impl(
        path: PathBuf,
        stalled: bool,
        sealed: Option<SealedStoreOpen>,
    ) -> Result<Self, StoreError> {
        let existed = path.exists();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(StoreError::storage)?;
        let header = u64::saturating_from(STORE_HEADER.len());
        if !existed || file.metadata().map_err(StoreError::storage)?.len() == 0 {
            file.write_all(&STORE_HEADER)
                .and_then(|()| file.sync_all())
                .map_err(StoreError::storage)?;
        }
        #[cfg(feature = "sealed")]
        let index = match sealed.as_ref() {
            Some(open) => record::replay_sealed(&mut file, &open.provider)?,
            None => replay(&mut file)?,
        };
        #[cfg(not(feature = "sealed"))]
        let index = {
            let _ = sealed.as_ref();
            replay(&mut file)?
        };
        // Trim the incomplete tail rather than appending after it: a write past
        // an interrupted frame leaves a file whose prefix is a prefix of a
        // record and whose middle is another record, which no reader could fold.
        if file.metadata().map_err(StoreError::storage)?.len() != index.committed {
            file.set_len(index.committed).map_err(StoreError::storage)?;
            file.sync_all().map_err(StoreError::storage)?;
        }
        debug_assert!(
            index.committed >= header,
            "the header is part of the committed length"
        );
        // The index is shared, not copied: the owner folds each committed record
        // into it inside the same ordered step as the write, and a lookup reads it
        // from an executor thread. That is not a lock across an `await` — a lookup
        // takes it for a hash-map probe and gives it straight back, and the write
        // it can wait for is the *file's*, which is what the owner thread exists to
        // absorb. Parking on a mutex a few hundred nanoseconds wide is not
        // parking on an `fsync`; it is the second thing the fix is about.
        let index = Arc::new(Mutex::new(index));
        let owner =
            StorageOwner::spawn(file, Arc::clone(&index), stalled).map_err(StoreError::storage)?;
        #[cfg(feature = "sealed")]
        let sealed = sealed.map(|open| open.provider);
        Ok(Self {
            inner: Arc::new(StoreInner {
                owner,
                path,
                index,
                unreadable: AtomicBool::new(false),
                #[cfg(feature = "sealed")]
                sealed,
            }),
        })
    }

    // Sealed opens live in `sealed_impl`: `open_sealed` and `open_in_sealed`
    // reuse this file's fence, chain and torn-tail repair, so they are thin
    // constructors over `open_impl`, not a second open path.
    /// Make the next *durable step's* record read fail, as an unreadable device would.
    ///
    /// A fault injector, and public for the reason
    /// [`RunStore::open_with_stalled_device`](Self::open_with_stalled_device) is:
    /// "what does a durable step do when its store cannot be read" is a question
    /// INV-BOT-7 answers in a fault nobody can schedule on a real process, and a
    /// probe that exists only inside the crate's own test binary cannot answer it.
    /// The records themselves are untouched — only the *read* of the in-memory
    /// index fails — so the store is exactly one it holds records for, and the
    /// question under test is what the step does with an answer it did not get.
    ///
    /// Aimed at the step rather than at every reader, because the host reads the
    /// same index before the body runs: a fault spent on the host's admission
    /// pre-flight would report `Disposition::Refused` at admission and would never
    /// reach the check whose behaviour this exists to observe.
    ///
    /// One-shot, and bounded by construction: the first step read that observes it
    /// takes it with a `swap`, so a caller that arms it and never reads is not left
    /// with a store that fails forever, and two concurrent reads cannot both take
    /// the same fault.
    pub fn fail_next_index_read(&self) {
        self.inner.unreadable.store(true, Ordering::SeqCst);
    }

    /// A handle that can release this store's parked device, independently of
    /// the store.
    ///
    /// Returned rather than only as [`Self::release_device`] because the caller
    /// that most needs to un-stick the device is the one awaiting a record on it,
    /// and that caller holds the store's borrow for the whole wait.
    #[must_use]
    pub fn storage_gate(&self) -> StorageGate {
        self.inner.owner.gate()
    }

    /// Let the outstanding flush proceed, and every flush after it.
    ///
    /// After this the handle is an ordinary store: the stall is over and is not
    /// re-armed. A caller awaiting a record on this store cannot reach this
    /// method — the borrow the append holds is the same one — and needs the gate
    /// [`Self::storage_gate`] hands back instead.
    pub fn release_device(&self) {
        self.storage_gate().release();
    }

    /// Arm a refusal of the next batch's covering `sync_all`.
    ///
    /// The seam the storage owner's `fail_next_flush` reaches through, so the
    /// all-or-nothing answer of a failed group commit — every member refused, no
    /// fold run, one poison latched, and a reopen that reads the file rather than
    /// the handle — is proved on the shipped store rather than a copy of it. A
    /// test-only door for the same reason `StorageOwner::fail_next_commit` is: no
    /// filesystem refuses a flush on demand.
    #[cfg(test)]
    pub(crate) fn fail_next_flush(&self) {
        self.inner.owner.fail_next_flush();
    }

    /// Open a store under `dir`, named by the tenant that owns it.
    ///
    /// The form [`HostBuilder::run_store`](crate::task::HostBuilder::run_store)
    /// uses. The tenant is part of the file name, so two tenants pointed at one
    /// directory never share a file — and a store installed under two names is
    /// two stores, which is why the cross-tenant refusal below is enforced on the
    /// record rather than on the path.
    ///
    /// # Errors
    ///
    /// As [`RunStore::open`].
    pub fn open_in(dir: &Path, tenant: &str) -> Result<Self, StoreError> {
        std::fs::create_dir_all(dir).map_err(StoreError::storage)?;
        Self::open(dir.join(format!("{tenant}.runstore")))
    }

    /// Where this store lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// How many records `run` holds.
    #[must_use]
    pub fn record_count(&self, run: RunId) -> usize {
        match self.index().runs.get(&run) {
            Some(held) => held.steps.len(),
            None => 0,
        }
    }

    /// The archived bytes committed for `key` under `run`, or `None` when there
    /// are none.
    ///
    /// The reader's door onto a durable step's value, and the other half of
    /// [`RunStore::record_count`]: a resumed run needs to read back *what* a
    /// previous instance recorded, not merely *that* it recorded something, and
    /// without this the only way to see a record's contents would be to re-run the
    /// step that wrote it.
    ///
    /// `Err` is a read failure — a run another tenant owns — never a miss, so a
    /// caller never reads a refusal as absence (INV-BOT-7).
    ///
    /// # Errors
    ///
    /// [`FlowError`] when `run` belongs to a tenant other than `tenant`.
    pub fn lookup(
        &self,
        tenant: &str,
        run: RunId,
        key: StepKey,
    ) -> Result<Option<Vec<u8>>, FlowError> {
        Ok(RunRecords::lookup(self, tenant, run, key)?.map(|stored| stored.into_bytes()))
    }

    /// The bytes this store has committed, header included.
    #[must_use]
    pub fn committed_bytes(&self) -> u64 {
        self.index().committed
    }

    /// How many `fsync`s this store has paid, and how many records it has staged.
    ///
    /// The evidence that a group commit formed rather than a claim that one could:
    /// the first number counts the `sync_all` calls the storage owner actually
    /// made, the second counts the records it actually staged. A store that flushed
    /// once per record has a ratio of one; a store that batched has a ratio below
    /// one, and the smaller it is the more records each flush carried.
    ///
    /// Both are counted on the owner thread rather than by the callers, so a caller
    /// that gave up on its answer is still counted as a staged record — leaving it
    /// out would report a better batching factor than the store achieved.
    #[must_use]
    pub fn flush_counts(&self) -> (u64, u64) {
        self.inner.owner.flush_counts()
    }

    /// Whether this store has a record under `run`.
    ///
    /// The check `Host::resume` makes before it admits a run: a run id this
    /// store never wrote is one it cannot attribute to a tenant, and resuming it
    /// would be recording another host's run under this host's tenant.
    #[must_use]
    pub fn knows_run(&self, run: RunId) -> bool {
        self.index().runs.contains_key(&run)
    }

    /// How many parked approvals of `tenant` are undecided.
    ///
    /// The ceiling an `await approval` is admitted against, read as one probe
    /// rather than a scan: the count is folded at replay and at every
    /// committed append, so a park costs a hash-map probe however large the
    /// store is.
    pub(crate) fn pending_for(&self, tenant: &str) -> u64 {
        // A tenant the index has never folded a park for has none undecided.
        match self.index().pending.get(tenant) {
            Some(count) => *count,
            None => 0,
        }
    }

    // The pending listing lives in `sealed_impl`: the scan decrypts sealed
    // steps there and lists plain ones the same way.
    /// Who minted `run`, when the store knows.
    ///
    /// The answer a caller checks before deciding a run id belongs to someone
    /// else, so it names the tenant from the record rather than from the path.
    #[must_use]
    pub fn tenant_of(&self, run: RunId) -> Option<String> {
        let index = self.index();
        index.runs.get(&run).map(|held| held.tenant.clone())
    }

    /// The definition `run`'s records were written under.
    ///
    /// The answer a resume compares its own identity against, through
    /// [`Self::check_definition`]. `None` for a run this store never wrote, which
    /// is not a refusal to read — a process killed before its first record left
    /// nothing to disagree with.
    #[must_use]
    pub fn definition_of(&self, run: RunId) -> Option<DefinitionIdentity> {
        self.index()
            .runs
            .get(&run)
            .map(|held| held.definition.clone())
    }

    /// Refuse `identity` for `run` if the records disagree with it.
    ///
    /// The pre-flight a resume makes before admission: a run whose recorded
    /// definition, input, step count or value schema is not the one being resumed
    /// is refused with [`StoreError::Incompatible`] and nothing is written, so no
    /// step body is polled and no record is added. Called both at the host, before
    /// the body is ever constructed, and by every durable step before it replays a
    /// value — the second because a host with no store is not the only door, and a
    /// store handed a run from elsewhere must not answer from another definition's
    /// records either.
    ///
    /// # Errors
    ///
    /// [`StoreError::Incompatible`] naming the axis that disagrees. A run this
    /// store has no record for is not refused.
    pub fn check_definition(
        &self,
        run: RunId,
        identity: &DefinitionIdentity,
    ) -> Result<(), StoreError> {
        let Some(recorded) = self.definition_of(run) else {
            return Ok(());
        };
        match identity.drift_from(&recorded) {
            None => Ok(()),
            Some(drift) => Err(StoreError::Incompatible {
                run: run.id().to_hex(),
                drift,
            }),
        }
    }

    /// Turn a borrowed record into one that can cross onto the storage owner's
    /// thread.
    ///
    /// The stored record owns its tenant and path, so the hop is the only part that
    /// has to be `'static`, and it takes the record by value rather than borrowing
    /// the caller's strings for the length of a flush.
    fn owned(&self, record: StagedRecord<'_>) -> Staged {
        Staged {
            record: Stored {
                run: record.run(),
                key: record.key().as_bytes().to_vec(),
                tenant: record.tenant().to_owned(),
                path: record.path().to_owned(),
                definition: StoredDefinition::of(record.definition()),
                value: record.bytes().to_vec(),
            },
            key: record.key(),
            #[cfg(feature = "sealed")]
            sealed: self.inner.sealed.as_ref().map(|provider| SealedWrite {
                provider: Arc::clone(provider),
                expires_at: None,
            }),
        }
    }

    // Sealed expiry staging lives in `sealed_impl`: `with_expiry` travels
    // beside the provider there, not here.
    /// The index's lock, recovering a poison.
    ///
    /// Recovering is right here for the reason it is right in `journal::owner`: no
    /// arm in the append path panics while the lock is held — every one returns its
    /// `Result` first — so a poison is a bug in an unrelated thread, and
    /// propagating it would brick a store that is still perfectly readable.
    fn index(&self) -> MutexGuard<'_, Index> {
        owner::lock(&self.inner.index)
    }

    /// Refuse a durable step's read when the injector is armed.
    ///
    /// The injector is aimed at the [`RunRecords`] doors rather than at the index
    /// itself, and that placement is the whole design. Both the host's admission
    /// pre-flight and the durable step's replay check read the same in-memory
    /// index through [`Self::check_definition`], so a fault aimed at "the index"
    /// would be spent by whichever of the two ran first — which for a resumed run
    /// is always the host, and a fault the host absorbs tests the host's refusal
    /// rather than the step's, proving nothing about `remember_at`. The
    /// [`RunRecords`] trait is reached only from `Scope::remember`, so arming it
    /// here makes the fault land on the check under test whatever else the run
    /// path has already read.
    ///
    /// One-shot, and bounded by construction: the first step read that observes it
    /// takes it with a `swap`, so a caller that arms it and never reads is not left
    /// with a store that fails forever, and two concurrent reads cannot both take
    /// the same fault.
    fn step_readable(&self) -> Result<(), StoreError> {
        if self.inner.unreadable.swap(false, Ordering::SeqCst) {
            let refusal = Err(StoreError::storage(std::io::Error::other(
                "the run store's device refused a read of its index",
            )));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "step_readable: returning an error to the caller");
            return refusal;
        }
        Ok(())
    }
}
// The burn-set slot lives in `sealed_impl` beside the shred path that folds it.
/// One staged append: the record to write and the step key it is for.
///
/// Its own type so the two append doors build the record once between them, and
/// the key travels with it rather than being threaded beside it at each call site.
struct Staged {
    /// The record as it will be archived.
    record: Stored,
    /// The step key the already-recorded and conflict checks are on.
    key: StepKey,
    /// Who wraps this record's data key and when it expires, when the store
    /// was opened sealed. `None` for every plain append.
    #[cfg(feature = "sealed")]
    sealed: Option<SealedWrite>,
}

/// The in-file chain head of one run, held with the index.
impl RunStore {
    /// The chain step of `stored`: the head of the whole file, which every run
    /// shares, so a frame's head cannot be valid under one run's history and
    /// invalid under the file's.
    ///
    /// One chain for the file rather than one per run, deliberately. A per-run
    /// chain would need the head to be re-found by replaying only that run's
    /// frames, and a caller that read one run's records could then not verify
    /// them without reading every other tenant's records too. One file, one
    /// chain, and the tenant check above is what keeps the reads separated.
    ///
    /// The whole of this runs on the storage owner's thread — the in-memory checks,
    /// the fence, the write, the `sync_all` and the fold into the index — so two
    /// appends to this store are two ordered steps rather than two racing ones,
    /// and none of them parks the executor the `sync_all` does. INV-BOT-50.
    fn commit(&self, staged: Staged) -> Result<Appended, StoreError> {
        self.inner
            .owner
            .submit(move |file, index| append_on_owner(file, index, &staged))
            .map_err(StoreError::from)
    }

    /// The same append, awaited rather than sat through.
    ///
    /// The one a durable step takes, because a step's body is a future and the
    /// `sync_all` its record costs is the device's, not the executor's.
    fn commit_async<'a>(
        &'a self,
        staged: Staged,
    ) -> crate::BoxFuture<'a, Result<Appended, StoreError>> {
        Box::pin(async move {
            self.inner
                .owner
                .submit_async(move |file, index| append_on_owner(file, index, &staged))
                .await
                .map_err(StoreError::from)
        })
    }

    // Sealed appends live in `sealed_impl`: `append_with_expiry` seals
    // through the same commit path here.
}

/// The ordered append, over the file and the index it folds into.
///
/// A free function taking the shared state rather than a method on `&self`, because
/// it runs on the storage owner's thread with a closure that must own everything it
/// touches: the `Arc` the store's state lives in and the record itself.
///
/// The lock is taken for the stage half only. The settle half is a fold into the
/// same index, and it runs on this same thread under the owner's ordering — after
/// the batch's one `sync_all`, never before it — so no append can observe a
/// half-committed index and no `.await` is ever reached holding it.
///
/// # Errors
///
/// Every [`StoreError`] an append can produce, carried as the device's own error so
/// the owner's one reply channel serves both stores.
fn append_on_owner(
    file: &mut File,
    shared: &Arc<Mutex<Index>>,
    staged: &Staged,
) -> std::io::Result<Stage<Appended, Arc<Mutex<Index>>>> {
    let mut index = owner::lock(shared);
    decide_and_write(file, &mut index, staged)
}

/// The decision, the write, and the fold this record owes its batch's flush.
///
/// Every check that can refuse in memory runs before a byte moves, and the length
/// fence runs inside the owner's ordered step where no other append can overtake it
/// — which is why the file's `metadata` is read here rather than through the
/// read-only view: the authoritative answer is the one taken at the moment of the
/// write.
///
/// The three ceilings and the tenant and duplicate checks are decided *here*, at
/// stage time, exactly as they were before group commit: a refusal is the same
/// refusal and refuses the same single record. The fold into the index is what the
/// settle half does, after the batch's one `sync_all` returns, so an index never
/// names a record the device did not take.
///
/// Under a sealed write the duplicate check compares the value's digest rather
/// than the value, and the frame seals under a fresh data key: the archived
/// bytes go through the envelope with that key, and the chain closure hashes
/// the ciphertext together with the key identity.
///
/// # Errors
///
/// Every [`StoreError`] an append can produce. Nothing is written by any of them,
/// and a refusal leaves the index and the file byte-identical.
fn decide_and_write(
    file: &mut File,
    index: &mut Index,
    staged: &Staged,
) -> std::io::Result<Stage<Appended, Arc<Mutex<Index>>>> {
    let stored = &staged.record;
    let key = staged.key;
    #[cfg(feature = "sealed")]
    let sealed = staged.sealed.as_ref();
    let owned = index.runs.get(&stored.run);
    if let Some(owned) = owned {
        if owned.tenant != stored.tenant {
            let refusal = Err(refusal(StoreError::ForeignTenant {
                owner: owned.tenant.clone(),
                asked: stored.tenant.clone(),
            }));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decide_and_write: returning an error to the caller");
            return refusal;
        }
        if let Some(step) = owned.steps.get(&key.to_hex()) {
            // Already recorded. An identical record is the same fact seen
            // twice — a step whose value was returned and whose caller
            // recorded it again — and a different one is a conflict. Neither
            // writes a byte. The digest stands in for the value, so the
            // comparison never retains a secret.
            #[cfg(feature = "sealed")]
            if let Some(sealed) = step.sealed.as_ref() {
                let answer = sealed_impl::sealed_duplicate(sealed, &stored.value);
                return Ok(Stage::Settled(Ok(answer)));
            }
            return Ok(Stage::Settled(Ok(if step.bytes == stored.value {
                Appended::AlreadyRecorded
            } else {
                Appended::Conflicting
            })));
        }
        #[cfg(feature = "sealed")]
        if let Some(burned) = sealed_impl::refuse_if_burned(index, stored.run, &key.to_hex()) {
            let refusal = Err(refusal(burned));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decide_and_write: the step's slot is burned");
            return refusal;
        }
        if u64::saturating_from(owned.steps.len()) >= MAX_RECORDS_PER_RUN {
            let refusal = Err(refusal(StoreError::Limit {
                kind: StoreLimitKind::Records,
                requested: u64::saturating_from(owned.steps.len()).saturating_add(1),
                limit: MAX_RECORDS_PER_RUN,
            }));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decide_and_write: returning an error to the caller");
            return refusal;
        }
    }

    // A record for this key staged earlier in the same batch is already the
    // answer: the same bytes are the same fact, other bytes a conflict. The
    // staged map holds the plaintext only until the batch's flush folds it —
    // the durable index never does for a sealed step.
    let staged_key = (stored.run, key.to_hex());
    if let Some(bytes) = index.staged.get(&staged_key) {
        return Ok(Stage::Settled(Ok(if *bytes == stored.value {
            Appended::AlreadyRecorded
        } else {
            Appended::Conflicting
        })));
    }

    let previous = tail_head(index);
    #[cfg(feature = "sealed")]
    let (framed, head, sealed_slot) =
        sealed_impl::frame_stored(stored, &previous, sealed).map_err(refusal)?;
    #[cfg(not(feature = "sealed"))]
    let (framed, head) = record::frame(stored, &previous).map_err(refusal)?;
    #[cfg(not(feature = "sealed"))]
    let _sealed_slot: Option<()> = None;

    let staged = u64::saturating_from(framed.len());
    let next = index
        .committed
        .checked_add(staged)
        .ok_or_else(|| refusal(store_full(u64::MAX)))?;
    if next > MAX_STORE_BYTES {
        let refusal = Err(refusal(store_full(next)));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decide_and_write: returning an error to the caller");
        return refusal;
    }

    // The fence, checked before a byte is written: the length this handle
    // indexed must be the length on the disk, so a file another writer moved
    // under it is refused here rather than forked. It runs on the owner thread,
    // so the check and the write it guards cannot be split by another append.
    let on_disk = file
        .metadata()
        .map_err(StoreError::storage)
        .map_err(refusal)?
        .len();
    if on_disk != index.committed {
        let held = match index.runs.get(&stored.run) {
            Some(run) => run.steps.len(),
            None => 0,
        };
        let refusal = Err(refusal(StoreError::Corrupt {
            at: u64::saturating_from(held),
        }));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "decide_and_write: returning an error to the caller");
        return refusal;
    }

    // The chain head and the committed length move *before* the write, and are the
    // batch's own bookkeeping rather than a claim the device took anything: the next
    // member of this batch must chain over this record, and the next batch's length
    // fence must expect these bytes. A head that lagged a write would fork the chain.
    index.committed = next;
    index.tail = head;
    index.staged.insert(staged_key, stored.value.clone());

    file.write_all(&framed)
        .map_err(StoreError::storage)
        .map_err(refusal)?;

    // The readable half of the fold waits for the flush: a record a lookup can find
    // is a record whose bytes the device has acknowledged, so publishing it earlier
    // would let a concurrent reader see a value a crash could take back.
    // A sealed step folds its envelope material instead of its value: the
    // index keeps the routing and the ciphertext, never the plaintext, and
    // the approval count folds from the staged plaintext the settle still
    // holds for this one flush.
    #[cfg(feature = "sealed")]
    let held = sealed_impl::held_for_settle(stored, sealed_slot);
    #[cfg(not(feature = "sealed"))]
    let held = Held {
        path: stored.path.clone(),
        bytes: stored.value.clone(),
    };
    let key = hex_of(&stored.key);
    let run = stored.run;
    let tenant = stored.tenant.clone();
    let definition = stored.definition.to_identity();
    #[cfg(feature = "sealed")]
    let count_bytes = stored.value.clone();
    Ok(Stage::Unsynced {
        answer: Appended::Recorded,
        bytes: framed.len(),
        settle: Box::new(move |index: &mut Arc<Mutex<Index>>| {
            #[cfg(feature = "sealed")]
            let mut held = held;
            let mut index = owner::lock(index);
            index.staged.remove(&(run, key.clone()));
            #[cfg(feature = "sealed")]
            count_approval(&mut index, &tenant, &count_bytes);
            #[cfg(not(feature = "sealed"))]
            count_approval(&mut index, &tenant, &held.bytes);
            #[cfg(feature = "sealed")]
            let ordinal = {
                let ordinal = index.frames;
                index.frames = index.frames.saturating_add(1);
                ordinal
            };
            let entry = index.runs.entry(run).or_insert_with(|| RunIndex {
                tenant: tenant.clone(),
                definition: definition.clone(),
                steps: HashMap::new(),
            });
            #[cfg(feature = "sealed")]
            if let Some(sealed) = held.sealed.as_mut() {
                sealed.ordinal = ordinal;
            }
            entry.steps.insert(key, held);
        }),
    })
}
