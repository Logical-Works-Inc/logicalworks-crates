//! The record a run store archives, how it is framed, and the replay that
//! rebuilds the index from the file.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
#[cfg(feature = "sealed")]
use std::sync::Arc;
#[cfg(feature = "sealed")]
use std::time::Duration;

use lgwks_std::hash::{Digest, Hasher};
use lgwks_std::wire::{WireError, from_bytes, to_bytes};

use crate::effect::RunId;
use crate::journal::frame::{self, Cursor, HEAD_BYTES, SaturatingFrom};
#[cfg(feature = "sealed")]
use crate::sealed::envelope::{NONCE_BYTES, open as envelope_open, seal as envelope_seal};
#[cfg(feature = "sealed")]
use crate::sealed::fold::{Unwrapped, fold_unwrap};
#[cfg(feature = "sealed")]
use crate::sealed::keys::{KeyId, KeyProvider};
#[cfg(feature = "sealed")]
use crate::sealed::record::{DecodedFrame, decode_frame, encode_record, is_sealed_payload};

#[cfg(feature = "sealed")]
use super::SealedWrite;
use super::{
    DefinitionIdentity, Held, Index, MAX_RECORD_BYTES, MAX_RECORDS_PER_RUN, MAX_STORE_BYTES,
    RunIndex, STORE_HEADER, StoreError, StoreLimitKind, check_format_version, genesis_head,
};
use crate::script::approval::{RecordClass, classify_record};

/// Read and validate the file header: the constant prefix every version of
/// this format shares, without which the file was never a run store.
///
/// One definition because the plain and sealed replays refuse a missing
/// header with the same answer, and two spellings would be two chances to
/// name a different length for one check.
fn read_header(file: &mut File) -> Result<[u8; STORE_HEADER.len()], StoreError> {
    let mut header = [0u8; STORE_HEADER.len()];
    if !read_full(file, &mut header)? {
        let refusal = Err(StoreError::NotAStore);
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay: the file holds no header");
        return refusal;
    }
    Ok(header)
}

/// The replay's starting point: how many bytes the file held, the empty
/// index, and the chain head the first frame follows.
///
/// One definition because both replays open the same way — length first,
/// then the header, then the version, then the empty index — and two
/// spellings would be two chances to start one of them past a byte the other
/// still counts.
fn replay_prologue(file: &mut File) -> Result<(u64, Index, Digest), StoreError> {
    let total = file.metadata().map_err(StoreError::storage)?.len();
    file.seek(SeekFrom::Start(0)).map_err(StoreError::storage)?;

    let header = read_header(file)?;
    // The two refusals are separated, and the version one is checked first,
    // because they are different facts and only one of them is a version the
    // writer could have meant. Every version of this format shares the whole
    // magic *except* its last byte, which is that format's version — so a file
    // whose first fifteen bytes match is this store's, at some version, and a
    // file whose first fifteen do not match was never a run store at all. That
    // is the whole reason `STORE_MAGIC` is 15 bytes of constant plus a version
    // byte rather than one 16-byte constant.
    check_format_version(header)?;
    Ok((total, empty_index(), genesis_head()))
}

/// The index a replay starts from: nothing committed past the header, the
/// chain head genesis, and no staged or burned slots.
///
/// One definition because the plain and sealed replays fold into the same
/// empty index, and two spellings would be two chances to start one of them
/// already holding records.
fn empty_index() -> Index {
    Index {
        runs: HashMap::new(),
        committed: u64::saturating_from(STORE_HEADER.len()),
        tail: genesis_head(),
        pending: HashMap::new(),
        staged: HashMap::new(),
        #[cfg(feature = "sealed")]
        frames: 0,
        #[cfg(feature = "sealed")]
        erased: HashMap::new(),
        #[cfg(feature = "sealed")]
        erased_runs: HashMap::new(),
    }
}

/// Count one more committed frame, refusing past the per-run record ceiling.
///
/// One definition because both replays bound the same loop by the same
/// ceiling, and two spellings would be two chances to admit a different
/// count for one file.
fn bump_frame_count(at: u64) -> Result<u64, StoreError> {
    let at = at.saturating_add(1);
    if at > MAX_RECORDS_PER_RUN {
        let refusal = Err(StoreError::Limit {
            kind: StoreLimitKind::Records,
            requested: at,
            limit: MAX_RECORDS_PER_RUN,
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay: the file holds more frames than one run retains");
        return refusal;
    }
    Ok(at)
}

/// Read the next whole frame under the shared torn-tail discipline: a partial
/// length prefix or a payload or head cut short is an append that never
/// finished, so the scan stops and the caller trims to `committed`.
///
/// One definition because both frame readers walk the same grammar with the
/// same limit and the same corruption answer, and two spellings would be two
/// chances to trim what the other refuses.
fn read_frame_at(
    file: &mut File,
    cursor: &Cursor<'_>,
    head_of: impl Fn(&Digest, &[u8]) -> Option<Digest>,
) -> Result<Option<frame::Raw>, StoreError> {
    let at = cursor.at;
    let corrupt = || StoreError::Corrupt { at };
    let Some(raw) = frame::read_raw(
        file,
        cursor,
        MAX_RECORD_BYTES,
        StoreError::storage,
        corrupt,
        head_of,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(raw))
}

/// A store refusal as the device error the owner's one reply channel carries.
///
/// One conversion rather than a `map_err` per arm, because every `?` on a store
/// refusal wraps the same fact — this store said no — and a literal per site would
/// be seven chances to name a different message for one event. The refusal's own
/// `Display` carries the ceiling or the tenant that names it.
pub(super) fn refusal(cause: StoreError) -> std::io::Error {
    std::io::Error::other(cause.to_string())
}

/// The chain head the next frame in this file follows.
///
/// The index carries the tail head beside the committed length, because the head
/// cannot be recomputed from the index: a frame's head is over the *archived*
/// record, and re-archiving a value to hash it would be a second encoding of the
/// same bytes with its own chance of disagreeing with what is on the disk.
pub(super) fn tail_head(index: &Index) -> Digest {
    index.tail
}

/// The record as it is archived and as it is checked.
#[derive(
    Debug, Clone, lgwks_std::wire::Archive, lgwks_std::wire::Serialize, lgwks_std::wire::Deserialize,
)]
#[rkyv(
    attr(non_exhaustive),
    crate = lgwks_std::wire::rkyv,
    compare(PartialEq),
    derive(Debug)
)]
pub(super) struct Stored {
    /// The run this record belongs to.
    #[rkyv(attr(doc = "The run this record belongs to."))]
    pub(super) run: RunId,
    /// The step key's digest.
    #[rkyv(attr(doc = "The step key's digest."))]
    pub(super) key: Vec<u8>,
    /// The tenant that minted the run.
    #[rkyv(attr(doc = "The tenant that minted the run."))]
    pub(super) tenant: String,
    /// The step's path, for attribution.
    #[rkyv(attr(doc = "The step's path, for attribution."))]
    pub(super) path: String,
    /// The definition the record was written under, as its fields.
    ///
    /// Archived as fields rather than as one digest because a drift refusal has
    /// to *name* the two sides: a head over the digest could detect the
    /// disagreement but could not report which axis it was, and the fields are
    /// what a caller reads to decide whether it needs a new run or a migration.
    #[rkyv(attr(doc = "The definition the record was written under, as its fields."))]
    pub(super) definition: StoredDefinition,
    /// The archived value the step returned.
    #[rkyv(attr(doc = "The archived value the step returned."))]
    pub(super) value: Vec<u8>,
}

/// A [`DefinitionIdentity`] in the shape a record archives.
///
/// Named rather than archived directly so the record's on-disk shape is stated
/// once, in the store that writes it, and a future revision of the definition
/// vocabulary is a new struct here rather than a change to a type every consumer
/// of [`Drift`] is already matching.
#[derive(
    Debug, Clone, lgwks_std::wire::Archive, lgwks_std::wire::Serialize, lgwks_std::wire::Deserialize,
)]
#[rkyv(
    attr(non_exhaustive),
    crate = lgwks_std::wire::rkyv,
    compare(PartialEq),
    derive(Debug)
)]
pub(super) struct StoredDefinition {
    /// The task name.
    #[rkyv(attr(doc = "The task name."))]
    name: String,
    /// The declared definition revision.
    #[rkyv(attr(doc = "The declared definition revision."))]
    revision: u64,
    /// The input digest.
    #[rkyv(attr(doc = "The input digest."))]
    input: Vec<u8>,
    /// How many durable steps the definition declares.
    #[rkyv(attr(doc = "How many durable steps the definition declares."))]
    steps: u64,
    /// The declared durable-value schema id.
    #[rkyv(attr(doc = "The declared durable-value schema id."))]
    codec: String,
}

/// The 32 bytes a record archived, as the digest the identity carries back.
///
/// A fixed width rather than a slice, because a record whose input field is not
/// 32 bytes is a corrupt frame and the checked decoder is what says so; the
/// conversion cannot silently truncate, because `as` is forbidden in this
/// workspace and this is the honest alternative to it.
fn digest_from_record(bytes: &[u8]) -> Digest {
    let mut digest = [0u8; 32];
    let copied = bytes.len().min(digest.len());
    digest[..copied].copy_from_slice(&bytes[..copied]);
    Digest::from_bytes(digest)
}

impl StoredDefinition {
    /// The record's archived form of a live identity.
    pub(super) fn of(identity: &DefinitionIdentity) -> Self {
        Self {
            name: identity.name().to_owned(),
            revision: identity.revision(),
            input: identity.input().as_bytes().to_vec(),
            steps: u64::saturating_from(identity.steps()),
            codec: identity.codec().to_owned(),
        }
    }

    /// The live identity this record was written under.
    pub(super) fn to_identity(&self) -> DefinitionIdentity {
        DefinitionIdentity::new(
            &self.name,
            self.revision,
            digest_from_record(&self.input),
            usize::saturating_from(self.steps),
        )
        .with_codec(&self.codec)
    }
}

impl Stored {
    /// This record's chain head over the head before it.
    ///
    /// Framed exactly as the step key is framed, so a record's head cannot be
    /// confused with the digest of the bytes it holds: the run, the key, the
    /// tenant, the path and every definition field are length-framed, and the
    /// value is framed last so no two different records hash the same byte
    /// string. The definition's own bytes go in through the identity's framing
    /// so a head over the record commits to the identity the same way the
    /// identity's digest commits to it.
    pub(super) fn head_from(&self, previous: &Digest, _archived: &[u8]) -> Digest {
        let mut hasher = Hasher::new();
        hasher.write_framed(previous.as_bytes());
        hasher.write_framed(&self.run_key_bytes());
        hasher.write_framed(&self.key);
        hasher.write_framed(self.tenant.as_bytes());
        hasher.write_framed(self.path.as_bytes());
        hasher.write_framed(self.definition.name.as_bytes());
        hasher.write_framed(&self.definition.revision.to_le_bytes());
        hasher.write_framed(&self.definition.input);
        hasher.write_framed(&self.definition.steps.to_le_bytes());
        hasher.write_framed(self.definition.codec.as_bytes());
        hasher.write_framed(&self.value);
        hasher.finalize()
    }

    /// The run id's 16 bytes, as a field the framing can hold.
    ///
    /// The hex spelling rather than the big-endian integer, because
    /// `RunId::to_hex` is the one conversion this crate defines for the role
    /// and a second one here would be a second definition to keep in step.
    fn run_key_bytes(&self) -> Vec<u8> {
        self.run.id().to_hex().into_bytes()
    }
}

/// The frame for one record: length, payload, head.
///
/// The head is stored rather than recomputed on faith. That is what separates a
/// torn tail (dropped, because a write leaves a prefix and a prefix has no head)
/// from a frame that does not follow (refused, because those bytes were
/// acknowledged).
pub(super) fn frame(stored: &Stored, previous: &Digest) -> Result<(Vec<u8>, Digest), StoreError> {
    // Archive, bound, chain and lay out — the same step the effect journal takes.
    // Only the record's own archiving and its step-chaining head are this store's.
    frame::frame_record(
        stored,
        previous,
        MAX_RECORD_BYTES,
        |stored| {
            to_bytes::<WireError>(stored)
                .map(|bytes| bytes.as_ref().to_vec())
                .map_err(|cause| StoreError::Encoding { cause })
        },
        Stored::head_from,
        record_too_large,
    )
}

/// The refusal for a store that has no room left, named by what it would have
/// needed.
///
/// One constructor because both call sites discovered the same fact by different
/// routes — an addition that overflowed and an addition that came out past the
/// ceiling — and a literal per site would be two chances to name a different bound
/// for the same ceiling.
pub(super) fn store_full(requested: u64) -> StoreError {
    StoreError::Limit {
        kind: StoreLimitKind::StoreBytes,
        requested,
        limit: MAX_STORE_BYTES,
    }
}

/// The refusal for a record whose archived bytes are past this store's ceiling.
///
/// One constructor, because the message and the two numbers are the same fact
/// whichever call site discovers it, and a literal per site would be two chances to
/// name a different bound for the same ceiling.
fn record_too_large(len: usize) -> StoreError {
    StoreError::Limit {
        kind: StoreLimitKind::RecordBytes,
        requested: u64::saturating_from(len),
        limit: u64::saturating_from(MAX_RECORD_BYTES),
    }
}

/// Read `buf` in full, reporting whether all of it arrived.
fn read_full(reader: &mut impl Read, buf: &mut [u8]) -> Result<bool, StoreError> {
    let mut filled = 0usize;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => return Ok(false),
            Ok(read) => filled = filled.saturating_add(read),
            Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(cause) => {
                let refusal = Err(StoreError::storage(cause));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "read_full: returning an error to the caller");
                return refusal;
            }
        }
    }
    Ok(true)
}

/// One complete, decoded frame read from the store.
struct Framed {
    /// The record the frame's payload decoded to.
    stored: Stored,
    /// The payload bytes, which the chain head is computed over.
    payload: Vec<u8>,
    /// The chain head the frame recorded for itself.
    head: [u8; HEAD_BYTES],
    /// The payload length the frame declared.
    declared: usize,
}

/// Read the frame at ordinal `at`, or `None` where the file stops holding a
/// whole frame.
///
/// A partial length prefix, or a payload or head cut short, is an append that
/// never finished: it was never anyone's answer, so the scan stops and the
/// caller trims to `committed`. A complete prefix naming a frame this store
/// never writes (a write leaves a prefix of a length it did finish computing,
/// and that length was always legal) or a payload that does not decode cannot
/// be an interrupted append and is refused. So is a cut whose bytes hold a frame
/// this store acknowledged under another length: `cursor` is where the frame
/// starts and the head it chains from, which is what the stored head is checked
/// against (#262).
fn next_frame(file: &mut File, cursor: &Cursor<'_>) -> Result<Option<Framed>, StoreError> {
    let Some(raw) = read_frame_at(file, cursor, |previous, payload| {
        let aligned = frame::decodable(payload);
        from_bytes::<Stored, WireError>(aligned.as_slice())
            .ok()
            .map(|stored| stored.head_from(previous, payload))
    })?
    else {
        return Ok(None);
    };
    #[cfg(feature = "sealed")]
    if is_sealed_payload(&raw.payload) {
        lgwks_std::trace::debug!(
            at = cursor.at,
            "next_frame: a plain replay holds a sealed frame"
        );
        return Err(StoreError::Corrupt { at: cursor.at });
    }
    let stored = from_bytes::<Stored, WireError>(&raw.payload).map_err(|error| {
        lgwks_std::trace::debug!(
            ?error,
            at = cursor.at,
            "next_frame: the payload did not decode"
        );
        StoreError::Corrupt { at: cursor.at }
    })?;
    Ok(Some(Framed {
        stored,
        declared: raw.payload.len(),
        payload: raw.payload,
        head: raw.head,
    }))
}

/// Read every committed frame, returning the index it implies.
///
/// The scan is bounded by the file's own length: every iteration consumes at
/// least one byte, and a file that grows underneath the scan cannot extend it,
/// because the reader stops at the length it saw when it began. A frame whose
/// stored head does not follow is [`StoreError::Corrupt`] and is not truncated.
pub(super) fn replay(file: &mut File) -> Result<Index, StoreError> {
    let (total, mut index, mut previous) = replay_prologue(file)?;
    let mut at = 0u64;
    while let Some(Framed {
        stored,
        payload,
        head,
        declared,
    }) = next_frame(file, &Cursor::new(at, index.committed, &previous))?
    {
        if stored.head_from(&previous, &payload) != Digest::from_bytes(head) {
            let refusal = Err(StoreError::Corrupt { at });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay: returning an error to the caller");
            return refusal;
        }
        let frame_len = frame::framed_len(declared);
        // A frame that runs past the end of the file is an interrupted append.
        // Readable as a whole above, so this only catches a header that lied
        // about a frame already acknowledged, which is rot rather than a crash.
        if index
            .committed
            .checked_add(frame_len)
            .is_none_or(|end| end > total)
        {
            break;
        }
        index.committed = index.committed.saturating_add(frame_len);
        index.tail = stored.head_from(&previous, &payload);
        previous = stored.head_from(&previous, &payload);
        at = bump_frame_count(at)?;
        let run = index.runs.entry(stored.run).or_insert_with(|| RunIndex {
            tenant: stored.tenant.clone(),
            definition: stored.definition.to_identity(),
            steps: HashMap::new(),
        });
        run.steps.insert(
            hex_of(&stored.key),
            Held {
                path: stored.path.clone(),
                bytes: stored.value.clone(),
                #[cfg(feature = "sealed")]
                sealed: None,
            },
        );
        count_approval(&mut index, &stored.tenant, &stored.value);
    }
    Ok(index)
}

/// Read every committed sealed frame, returning the index it implies.
///
/// The sealed twin of [`replay`]: the loop is the same torn-tail discipline
/// over the same frame grammar, but every record payload is an envelope the
/// provider opens. A destroyed key folds its slot to erased; any other
/// provider refusal fails the open, because the replay cannot attribute a
/// frame it cannot read, and inventing an attribution would be worse. A live
/// key decrypts into the index. Tombstones move their record to the burn
/// set. A plain payload in a sealed file, a foreign tenant, a broken bind or
/// a head that does not follow is [`StoreError::Corrupt`] and is not trimmed.
///
/// # Errors
///
/// [`StoreError::Storage`] when the device refuses; [`StoreError::NotAStore`]
/// when the file is not a run store; [`StoreError::Corrupt`] when a committed
/// frame does not follow from the ones before it.
#[cfg(feature = "sealed")]
pub(super) fn replay_sealed(
    file: &mut File,
    provider: &Arc<dyn KeyProvider>,
) -> Result<Index, StoreError> {
    let (total, mut index, mut previous) = replay_prologue(file)?;
    let mut at = 0u64;
    let mut ordinals: HashMap<u64, (RunId, String, [u8; 16])> = HashMap::new();
    while let Some(raw) = next_sealed_frame(file, &Cursor::new(at, index.committed, &previous))? {
        let frame_len = frame::framed_len(raw.declared);
        if index
            .committed
            .checked_add(frame_len)
            .is_none_or(|end| end > total)
        {
            break;
        }
        if crate::journal::chain_over_bytes(&previous, &raw.payload) != Digest::from_bytes(raw.head)
        {
            let refusal = Err(StoreError::Corrupt { at });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay_sealed: a frame does not follow the chain");
            return refusal;
        }
        let Some(decoded) = decode_frame(&raw.payload) else {
            let refusal = Err(StoreError::Corrupt { at });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay_sealed: a committed frame is not a sealed record");
            return refusal;
        };
        match decoded {
            DecodedFrame::Record {
                tenant,
                key_id,
                wrapped,
                nonce,
                ciphertext,
                expires_at,
                bind,
            } => {
                let (run_hex, key_hex) = match parse_runstore_bind(&bind) {
                    Some(pair) => pair,
                    None => {
                        let refusal = Err(StoreError::Corrupt { at });
                        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay_sealed: a frame carries no routing tag");
                        return refusal;
                    }
                };
                let run = match RunId::from_hex(&run_hex) {
                    Ok(run) => run,
                    Err(error) => {
                        lgwks_std::trace::debug!(at, error = ?error, "replay_sealed: a frame names no run");
                        return Err(StoreError::Corrupt { at });
                    }
                };
                let key = KeyId::new(tenant.clone(), key_id);
                if let Err(error) = provider.note_wrapped(&key, &wrapped) {
                    lgwks_std::trace::debug!(at, error = ?error, "replay_sealed: a live record's key does not open");
                    return Err(StoreError::SealedKey { source: error });
                }
                match fold_unwrap(provider.unwrap_key(&key, &wrapped)) {
                    Ok(Unwrapped::Live(data_key)) => {
                        let plaintext = match envelope_open(&data_key, &nonce, &ciphertext) {
                            Ok(plaintext) => plaintext,
                            Err(error) => {
                                lgwks_std::trace::debug!(at, error = ?error, "replay_sealed: a live key with an unopenable envelope");
                                return Err(StoreError::Corrupt { at });
                            }
                        };
                        let stored = match from_bytes::<Stored, WireError>(&plaintext) {
                            Ok(stored) => stored,
                            Err(error) => {
                                lgwks_std::trace::debug!(
                                    at,
                                    ?error,
                                    "replay_sealed: a committed frame does not decode"
                                );
                                return Err(StoreError::Corrupt { at });
                            }
                        };
                        if stored.tenant != tenant
                            || stored.run != run
                            || hex_of(&stored.key) != key_hex
                        {
                            lgwks_std::trace::debug!(
                                at,
                                "replay_sealed: a frame's routing does not match its record"
                            );
                            return Err(StoreError::Corrupt { at });
                        }
                        index.committed = index.committed.saturating_add(frame_len);
                        index.tail = Digest::from_bytes(raw.head);
                        previous = Digest::from_bytes(raw.head);
                        ordinals.insert(at, (run, key_hex.clone(), key_id));
                        let run_index = index.runs.entry(run).or_insert_with(|| RunIndex {
                            tenant: tenant.clone(),
                            definition: stored.definition.to_identity(),
                            steps: HashMap::new(),
                        });
                        run_index.steps.insert(
                            key_hex,
                            Held {
                                path: stored.path.clone(),
                                bytes: Vec::new(),
                                sealed: Some(crate::task::store::SealedStep {
                                    key_id: key.clone(),
                                    wrapped,
                                    nonce,
                                    ciphertext,
                                    digest: value_digest(&stored.value),
                                    expires_at,
                                    ordinal: at,
                                }),
                            },
                        );
                        count_approval(&mut index, &tenant, &stored.value);
                    }
                    Ok(Unwrapped::Erased) => {
                        index.committed = index.committed.saturating_add(frame_len);
                        index.tail = Digest::from_bytes(raw.head);
                        previous = Digest::from_bytes(raw.head);
                        ordinals.insert(at, (run, key_hex.clone(), key_id));
                        insert_erased(&mut index, run, key_hex, key_id, tenant, expires_at);
                    }
                    Err(error) => {
                        lgwks_std::trace::debug!(at, error = ?error, "replay_sealed: the provider cannot answer");
                        return Err(StoreError::SealedKey { source: error });
                    }
                }
            }
            DecodedFrame::Tombstone {
                index: target,
                key_id,
            } => {
                index.committed = index.committed.saturating_add(frame_len);
                index.tail = Digest::from_bytes(raw.head);
                previous = Digest::from_bytes(raw.head);
                let (run, key_hex) = match ordinals.get(&target) {
                    Some(found) if found.2 == key_id => (found.0, found.1.clone()),
                    _ => {
                        let refusal = Err(StoreError::Corrupt { at });
                        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay_sealed: a tombstone names no sealed record");
                        return refusal;
                    }
                };
                if index.erased.contains_key(&(run, key_hex.clone())) {
                    // A second tombstone for the same record: already erased.
                } else {
                    let step = match index
                        .runs
                        .get_mut(&run)
                        .and_then(|run_index| run_index.steps.remove(&key_hex))
                    {
                        Some(step) => step,
                        None => {
                            let refusal = Err(StoreError::Corrupt { at });
                            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay_sealed: a tombstone names an absent step");
                            return refusal;
                        }
                    };
                    let (tenant, sealed) = match (index.runs.get(&run), step.sealed) {
                        (Some(run_index), Some(sealed)) => (run_index.tenant.clone(), sealed),
                        _ => {
                            let refusal = Err(StoreError::Corrupt { at });
                            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay_sealed: a tombstone names a plain step");
                            return refusal;
                        }
                    };
                    if sealed.key_id.id() != key_id {
                        let refusal = Err(StoreError::Corrupt { at });
                        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay_sealed: a tombstone names the wrong key");
                        return refusal;
                    }
                    insert_erased(
                        &mut index,
                        run,
                        key_hex,
                        sealed.key_id.id(),
                        tenant,
                        sealed.expires_at,
                    );
                }
            }
        }
        at = bump_frame_count(at)?;
    }
    index.frames = at;
    Ok(index)
}

/// Fold one shredded step into the burn set: the slot the reads refuse and
/// the sweep skips, with the per-run count the reports read.
#[cfg(feature = "sealed")]
fn insert_erased(
    index: &mut Index,
    run: RunId,
    key_hex: String,
    key_id: [u8; 16],
    tenant: String,
    expires_at: Option<Duration>,
) {
    let counts = index
        .erased_runs
        .entry(run)
        .or_insert_with(|| (0, tenant.clone()));
    counts.0 = counts.0.saturating_add(1);
    index.erased.insert(
        (run, key_hex),
        crate::task::store::ErasedSlot {
            key_id: KeyId::new(tenant.clone(), key_id),
            tenant,
            expires_at,
        },
    );
}

/// One sealed frame read from the file: its payload, its stored head, and
/// the payload length the prefix declared.
#[cfg(feature = "sealed")]
struct SealedRaw {
    /// The frame's payload bytes: the sealed envelope.
    payload: Vec<u8>,
    /// The chain head the frame recorded for itself.
    head: [u8; 32],
    /// The payload length the frame declared.
    declared: usize,
}

/// Read one sealed frame at `cursor`, or `None` where the file stops holding
/// a whole frame.
///
/// The torn-tail discipline is the shared grammar's: a partial length prefix
/// or a payload cut short is an interrupted append, while a complete frame
/// whose head does not follow is refused. The head search only authenticates
/// sealed payloads, so acknowledged sealed bytes are never trimmed and plain
/// bytes never verify as sealed.
#[cfg(feature = "sealed")]
fn next_sealed_frame(
    file: &mut File,
    cursor: &Cursor<'_>,
) -> Result<Option<SealedRaw>, StoreError> {
    let Some(raw) = read_frame_at(file, cursor, |previous, payload| {
        if is_sealed_payload(payload) {
            Some(crate::journal::chain_over_bytes(previous, payload))
        } else {
            None
        }
    })?
    else {
        return Ok(None);
    };
    let declared = raw.payload.len();
    Ok(Some(SealedRaw {
        payload: raw.payload,
        head: raw.head,
        declared,
    }))
}
///
/// Pending parks increment their tenant; decisions decrement, saturating, so
/// a decision that arrives with no park counted — an app park recorded
/// elsewhere, a torn write that never folded — cannot borrow below zero.
/// Anything else leaves the count alone: the ceiling is over approvals, not
/// over records.
pub(super) fn count_approval(index: &mut Index, tenant: &str, bytes: &[u8]) {
    match classify_record(bytes) {
        RecordClass::Pending(_) => {
            let held = index.pending.entry(tenant.to_owned()).or_insert(0);
            *held = held.saturating_add(1);
        }
        RecordClass::Decision(_) => {
            if let Some(held) = index.pending.get_mut(tenant) {
                *held = held.saturating_sub(1);
            }
        }
        RecordClass::Other => {}
    }
}

/// Lowercase hex, for a digest used as a map key.
///
/// `lgwks_std::hash::Digest::to_hex` takes a `Digest`, and this is the same 32
/// bytes before the head has been computed; formatting them here rather than
/// reaching into the digest type keeps the key exactly the bytes on the disk.
pub(super) fn hex_of(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = Vec::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes.iter().copied() {
        hex.push(DIGITS[usize::from(byte >> 4) & 0x0f]);
        hex.push(DIGITS[usize::from(byte) & 0x0f]);
    }
    // Every byte pushed above is an ASCII digit, so this is well-formed UTF-8
    // and the conversion cannot fail; `String::from_utf8` reports the invariant
    // rather than trusting it.
    hex.into_iter().map(char::from).collect()
}

/// The BLAKE3 of a plaintext step value: what the duplicate check compares
/// for a sealed step, so two identical appends still answer `AlreadyRecorded`
/// without the index holding any secret.
#[cfg(feature = "sealed")]
pub(super) fn value_digest(value: &[u8]) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(value);
    *hasher.finalize().as_bytes()
}

/// What a sealed frame the write path just framed carries for the fold.
#[cfg(feature = "sealed")]
pub(super) struct SealedFramed {
    /// The data key's identity.
    pub(super) key_id: KeyId,
    /// The wrapped data key, as the frame will store it.
    pub(super) wrapped: Vec<u8>,
    /// The seal nonce, as the frame will store it.
    pub(super) nonce: [u8; NONCE_BYTES],
    /// The ciphertext, as the frame will store it.
    pub(super) ciphertext: Vec<u8>,
    /// The BLAKE3 of the plaintext value, for the duplicate check.
    pub(super) digest: [u8; 32],
    /// When the record expires, as the frame will store it.
    pub(super) expires_at: Option<Duration>,
}

/// Destroy the orphan key `id` minted for an append that then refused:
/// a key no frame names must not outlive the call that minted it.
#[cfg(feature = "sealed")]
fn abandon_sealed(provider: &Arc<dyn KeyProvider>, id: &KeyId) {
    if let Err(error) = provider.destroy(id) {
        lgwks_std::trace::debug!(error = ?error, "sealed-frame: the orphan key could not be destroyed");
    }
}

/// The plaintext routing tag of a sealed step: the run and step hex beside
/// a zero byte, covered by the chain head with the ciphertext, so a sealed
/// frame names its own slot without the key.
#[cfg(feature = "sealed")]
pub(super) fn runstore_bind(run_hex: &str, key_hex: &str) -> Vec<u8> {
    let mut bind = Vec::with_capacity(
        run_hex
            .len()
            .saturating_add(key_hex.len())
            .saturating_add(1),
    );
    bind.extend_from_slice(run_hex.as_bytes());
    bind.push(0);
    bind.extend_from_slice(key_hex.as_bytes());
    bind
}

/// Split a routing tag back into its run and step hex, or `None` for a shape
/// this store never wrote.
#[cfg(feature = "sealed")]
pub(super) fn parse_runstore_bind(bind: &[u8]) -> Option<(String, String)> {
    let at = bind.iter().position(|byte| *byte == 0)?;
    let (run, key) = bind.split_at(at);
    let key = key.get(1..)?;
    Some((
        String::from_utf8(run.to_vec()).ok()?,
        String::from_utf8(key.to_vec()).ok()?,
    ))
}

/// Frame one step record sealed: burn nothing here — the duplicate check
/// already ran on the index — mint a fresh key, seal, wrap, and frame
/// through the shared grammar with the chain over the sealed bytes.
///
/// Every failure after the provision destroys the orphan key, so a refused
/// append leaves no managed key behind.
///
/// # Errors
///
/// [`StoreError::SealedKey`] when the provider refuses; [`StoreError::Limit`]
/// when the sealed frame exceeds the ceiling. Nothing is written by any of
/// them.
#[cfg(feature = "sealed")]
pub(super) fn frame_sealed(
    stored: &Stored,
    previous: &Digest,
    write: &SealedWrite,
) -> Result<(Vec<u8>, Digest, SealedFramed), StoreError> {
    let digest = value_digest(&stored.value);
    let archived = to_bytes::<WireError>(stored).map_err(|cause| StoreError::Encoding { cause })?;
    let archived = archived.as_ref().to_vec();
    let (key_id, data_key) = write.provider.provision(&stored.tenant).map_err(|error| {
        lgwks_std::trace::debug!(error = ?error, "frame_sealed: the provider refused to mint a key");
        StoreError::SealedKey { source: error }
    })?;
    let destroy_orphan = |provider: &Arc<dyn KeyProvider>, id: &KeyId| {
        abandon_sealed(provider, id);
    };
    let nonce: [u8; NONCE_BYTES] = match lgwks_std::random::bytes::<NONCE_BYTES>() {
        Ok(nonce) => nonce,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "frame_sealed: no entropy for the seal nonce");
            destroy_orphan(&write.provider, &key_id);
            return Err(StoreError::SealedKey {
                source: crate::sealed::keys::KeyError::Transient {
                    reason: "no entropy for the seal nonce",
                },
            });
        }
    };
    let ciphertext = match envelope_seal(&data_key, &nonce, &archived) {
        Ok(ciphertext) => ciphertext,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "frame_sealed: the seal step refused");
            destroy_orphan(&write.provider, &key_id);
            return Err(StoreError::Storage {
                cause: std::io::Error::other(error.to_string()),
            });
        }
    };
    drop(data_key);
    let wrapped = match write.provider.wrap_key(&key_id) {
        Ok(wrapped) => wrapped,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "frame_sealed: the wrap step refused");
            destroy_orphan(&write.provider, &key_id);
            return Err(StoreError::SealedKey { source: error });
        }
    };
    let bind = runstore_bind(&stored.run.id().to_hex(), &hex_of(&stored.key));
    let payload = encode_record(
        stored.tenant.as_str(),
        &key_id.id(),
        &wrapped,
        &nonce,
        &ciphertext,
        write.expires_at,
        &bind,
    );
    let (framed, head) = frame::frame_record(
        &payload,
        previous,
        MAX_RECORD_BYTES,
        |bytes| Ok(bytes.clone()),
        |_, prior, sealed| crate::journal::chain_over_bytes(prior, sealed),
        record_too_large,
    )
    .inspect_err(|_| {
        destroy_orphan(&write.provider, &key_id);
    })?;
    Ok((
        framed,
        head,
        SealedFramed {
            key_id,
            wrapped,
            nonce,
            ciphertext,
            digest,
            expires_at: write.expires_at,
        },
    ))
}
