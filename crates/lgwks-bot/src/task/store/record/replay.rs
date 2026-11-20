//! The replay that rebuilds a run store's index from its file: the plain read
//! and, under `sealed`, the sealed twin that opens every envelope through the
//! key provider.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
#[cfg(feature = "sealed")]
use std::sync::Arc;
#[cfg(feature = "sealed")]
use std::time::Duration;

use lgwks_std::hash::Digest;
use lgwks_std::wire::{WireError, from_bytes};

#[cfg(feature = "sealed")]
use crate::effect::RunId;
use crate::journal::frame::{self, Cursor, HEAD_BYTES, SaturatingFrom};
#[cfg(feature = "sealed")]
use crate::sealed::envelope::{DataKey, NONCE_BYTES, open as envelope_open};
#[cfg(feature = "sealed")]
use crate::sealed::fold::{Unwrapped, fold_unwrap};
#[cfg(feature = "sealed")]
use crate::sealed::keys::{KeyId, KeyProvider};
#[cfg(feature = "sealed")]
use crate::sealed::record::{DecodedFrame, decode_frame, is_sealed_payload};
#[cfg(feature = "sealed")]
use crate::task::store::{ErasedSlot, SealedStep};
use crate::task::store::{
    Held, Index, MAX_RECORD_BYTES, MAX_RECORDS_PER_RUN, RunIndex, STORE_HEADER, StoreError,
    StoreLimitKind, check_format_version, genesis_head,
};

use super::{Stored, count_approval, hex_of};
#[cfg(feature = "sealed")]
use super::{parse_runstore_bind, value_digest};

/// Per sealed frame ordinal: the run, the step key hex and the data key's
/// identity, so a tombstone finds the record it names.
#[cfg(feature = "sealed")]
type Ordinals = HashMap<u64, (RunId, String, [u8; 16])>;

/// The refusal for a committed frame that does not follow, logged with why.
///
/// One definition because every replay refusal answers the same fact: the file
/// holds acknowledged bytes that cannot be what this store wrote.
fn refused(at: u64, why: &'static str) -> StoreError {
    lgwks_std::trace::debug!(at, why, "replay: a committed frame is refused");
    StoreError::Corrupt { at }
}

/// Read and validate the file header: the constant prefix every version of
/// this format shares, without which the file was never a run store.
///
/// One definition because the plain and sealed replays refuse a missing
/// header with the same answer, and two spellings would be two chances to
/// name a different length for one check.
fn read_header(file: &mut File) -> Result<[u8; STORE_HEADER.len()], StoreError> {
    let mut header = [0u8; STORE_HEADER.len()];
    if !read_full(file, &mut header)? {
        lgwks_std::trace::debug!("replay: the file holds no header");
        return Err(StoreError::NotAStore);
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
        lgwks_std::trace::debug!(
            at,
            "replay: the file holds more frames than one run retains"
        );
        return Err(StoreError::Limit {
            kind: StoreLimitKind::Records,
            requested: at,
            limit: MAX_RECORDS_PER_RUN,
        });
    }
    Ok(at)
}

/// Whether a frame of `frame_len` bytes runs past the end of the file: an
/// interrupted append, so the scan stops and the caller trims to `committed`.
fn runs_past(index: &Index, frame_len: u64, total: u64) -> bool {
    index
        .committed
        .checked_add(frame_len)
        .is_none_or(|end| end > total)
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

/// Read `buf` in full, reporting whether all of it arrived.
fn read_full(reader: &mut impl Read, mut buf: &mut [u8]) -> Result<bool, StoreError> {
    while !buf.is_empty() {
        match reader.read(buf) {
            Ok(0) => return Ok(false),
            Ok(read) => buf = buf.split_at_mut(read.min(buf.len())).1,
            Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(cause) => {
                lgwks_std::trace::debug!(error = ?cause, "read_full: returning an error to the caller");
                return Err(StoreError::storage(cause));
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
        match from_bytes::<Stored, WireError>(aligned.as_slice()) {
            Ok(stored) => Some(stored.head_from(previous, payload)),
            Err(error) => {
                lgwks_std::trace::debug!(
                    ?error,
                    "next_frame: the head search skips a cut that does not decode"
                );
                None
            }
        }
    })?
    else {
        return Ok(None);
    };
    #[cfg(feature = "sealed")]
    if is_sealed_payload(&raw.payload) {
        let refusal = Err(refused(
            cursor.at,
            "next_frame: a plain replay holds a sealed frame",
        ));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "next_frame: returning an error to the caller");
        return refusal;
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
pub(in crate::task::store) fn replay(file: &mut File) -> Result<Index, StoreError> {
    let (total, mut index, mut previous) = replay_prologue(file)?;
    let mut at = 0u64;
    while let Some(Framed {
        stored,
        payload,
        head,
        declared,
    }) = next_frame(file, &Cursor::new(at, index.committed, &previous))?
    {
        let chained = stored.head_from(&previous, &payload);
        if chained != Digest::from_bytes(head) {
            let refusal = Err(refused(at, "replay: a frame does not follow the chain"));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay: returning an error to the caller");
            return refusal;
        }
        let frame_len = frame::framed_len(declared);
        // A frame that runs past the end of the file is an interrupted append.
        // Readable as a whole above, so this only catches a header that lied
        // about a frame already acknowledged, which is rot rather than a crash.
        if runs_past(&index, frame_len, total) {
            break;
        }
        index.committed = index.committed.saturating_add(frame_len);
        index.tail = chained;
        previous = chained;
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

/// A sealed record frame's fields, as the frame stored them.
#[cfg(feature = "sealed")]
struct SealedRecord {
    /// The tenant the record was sealed for.
    tenant: String,
    /// The data key's identity.
    key_id: [u8; 16],
    /// The wrapped data key.
    wrapped: Vec<u8>,
    /// The seal nonce.
    nonce: [u8; NONCE_BYTES],
    /// The ciphertext.
    ciphertext: Vec<u8>,
    /// When the record expires, if it does.
    expires_at: Option<Duration>,
    /// The plaintext routing tag, covered by the chain with the rest.
    bind: Vec<u8>,
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
pub(in crate::task::store) fn replay_sealed(
    file: &mut File,
    provider: &Arc<dyn KeyProvider>,
) -> Result<Index, StoreError> {
    let (total, mut index, mut previous) = replay_prologue(file)?;
    let mut at = 0u64;
    let mut ordinals = Ordinals::new();
    while let Some(raw) = next_sealed_frame(file, &Cursor::new(at, index.committed, &previous))? {
        let frame_len = frame::framed_len(raw.declared);
        if runs_past(&index, frame_len, total) {
            break;
        }
        let head = Digest::from_bytes(raw.head);
        if crate::journal::chain_over_bytes(&previous, &raw.payload) != head {
            let refusal = Err(refused(
                at,
                "replay_sealed: a frame does not follow the chain",
            ));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay_sealed: returning an error to the caller");
            return refusal;
        }
        match decode_frame(&raw.payload) {
            None => {
                let refusal = Err(refused(
                    at,
                    "replay_sealed: a committed frame is not a sealed record",
                ));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay_sealed: returning an error to the caller");
                return refusal;
            }
            Some(DecodedFrame::Record {
                tenant,
                key_id,
                wrapped,
                nonce,
                ciphertext,
                expires_at,
                bind,
            }) => {
                let record = SealedRecord {
                    tenant,
                    key_id,
                    wrapped,
                    nonce,
                    ciphertext,
                    expires_at,
                    bind,
                };
                fold_record(&mut index, &mut ordinals, provider, at, record)?;
            }
            Some(DecodedFrame::Tombstone {
                index: target,
                key_id,
            }) => fold_tombstone(&mut index, &ordinals, at, target, key_id)?,
        }
        index.committed = index.committed.saturating_add(frame_len);
        index.tail = head;
        previous = head;
        at = bump_frame_count(at)?;
    }
    index.frames = at;
    Ok(index)
}

/// Open a live record's envelope and check it is the record its frame names.
#[cfg(feature = "sealed")]
fn unseal(
    data_key: &DataKey,
    record: &SealedRecord,
    key_hex: &str,
    run: RunId,
    at: u64,
) -> Result<Stored, StoreError> {
    let plaintext = envelope_open(data_key, &record.nonce, &record.ciphertext).map_err(|error| {
        lgwks_std::trace::debug!(at, error = ?error, "replay_sealed: a live key with an unopenable envelope");
        StoreError::Corrupt { at }
    })?;
    let stored = from_bytes::<Stored, WireError>(&plaintext).map_err(|error| {
        lgwks_std::trace::debug!(
            at,
            ?error,
            "replay_sealed: a committed frame does not decode"
        );
        StoreError::Corrupt { at }
    })?;
    if stored.tenant != record.tenant || stored.run != run || hex_of(&stored.key) != key_hex {
        let refusal = Err(refused(
            at,
            "replay_sealed: a frame's routing does not match its record",
        ));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "unseal: returning an error to the caller");
        return refusal;
    }
    Ok(stored)
}

/// Fold one sealed record frame into the index: a live key decrypts into a
/// step, a destroyed key folds its slot to erased, and any other provider
/// refusal fails the replay.
#[cfg(feature = "sealed")]
fn fold_record(
    index: &mut Index,
    ordinals: &mut Ordinals,
    provider: &Arc<dyn KeyProvider>,
    at: u64,
    record: SealedRecord,
) -> Result<(), StoreError> {
    let Some((run_hex, key_hex)) = parse_runstore_bind(&record.bind) else {
        let refusal = Err(refused(at, "replay_sealed: a frame carries no routing tag"));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fold_record: returning an error to the caller");
        return refusal;
    };
    let run = RunId::from_hex(&run_hex).map_err(|error| {
        lgwks_std::trace::debug!(at, error = ?error, "replay_sealed: a frame names no run");
        StoreError::Corrupt { at }
    })?;
    let key = KeyId::new(record.tenant.clone(), record.key_id);
    provider.note_wrapped(&key, &record.wrapped).map_err(|error| {
        lgwks_std::trace::debug!(at, error = ?error, "replay_sealed: a live record's key does not open");
        StoreError::SealedKey { source: error }
    })?;
    match fold_unwrap(provider.unwrap_key(&key, &record.wrapped)) {
        Ok(Unwrapped::Live(data_key)) => {
            let stored = unseal(&data_key, &record, &key_hex, run, at)?;
            ordinals.insert(at, (run, key_hex.clone(), record.key_id));
            let run_index = index.runs.entry(run).or_insert_with(|| RunIndex {
                tenant: record.tenant.clone(),
                definition: stored.definition.to_identity(),
                steps: HashMap::new(),
            });
            run_index.steps.insert(
                key_hex,
                Held {
                    path: stored.path.clone(),
                    bytes: Vec::new(),
                    sealed: Some(SealedStep {
                        key_id: key,
                        wrapped: record.wrapped,
                        nonce: record.nonce,
                        ciphertext: record.ciphertext,
                        digest: value_digest(&stored.value),
                        expires_at: record.expires_at,
                        ordinal: at,
                    }),
                },
            );
            count_approval(index, &record.tenant, &stored.value);
            Ok(())
        }
        Ok(Unwrapped::Erased) => {
            ordinals.insert(at, (run, key_hex.clone(), record.key_id));
            insert_erased(
                index,
                run,
                key_hex,
                record.key_id,
                record.tenant,
                record.expires_at,
            );
            Ok(())
        }
        Err(error) => {
            lgwks_std::trace::debug!(at, error = ?error, "replay_sealed: the provider cannot answer");
            Err(StoreError::SealedKey { source: error })
        }
    }
}

/// Fold one tombstone frame into the index: the record it names moves to the
/// burn set, and a second tombstone for the same record changes nothing.
#[cfg(feature = "sealed")]
fn fold_tombstone(
    index: &mut Index,
    ordinals: &Ordinals,
    at: u64,
    target: u64,
    key_id: [u8; 16],
) -> Result<(), StoreError> {
    let Some((run, key_hex)) = ordinals
        .get(&target)
        .filter(|found| found.2 == key_id)
        .map(|found| (found.0, found.1.clone()))
    else {
        let refusal = Err(refused(
            at,
            "replay_sealed: a tombstone names no sealed record",
        ));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fold_tombstone: returning an error to the caller");
        return refusal;
    };
    if index.erased.contains_key(&(run, key_hex.clone())) {
        // A second tombstone for the same record: already erased.
        return Ok(());
    }
    let Some(step) = index
        .runs
        .get_mut(&run)
        .and_then(|run_index| run_index.steps.remove(&key_hex))
    else {
        let refusal = Err(refused(
            at,
            "replay_sealed: a tombstone names an absent step",
        ));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fold_tombstone: returning an error to the caller");
        return refusal;
    };
    let (Some(run_index), Some(sealed)) = (index.runs.get(&run), step.sealed) else {
        let refusal = Err(refused(at, "replay_sealed: a tombstone names a plain step"));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fold_tombstone: returning an error to the caller");
        return refusal;
    };
    let tenant = run_index.tenant.clone();
    if sealed.key_id.id() != key_id {
        let refusal = Err(refused(
            at,
            "replay_sealed: a tombstone names the wrong key",
        ));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fold_tombstone: returning an error to the caller");
        return refusal;
    }
    insert_erased(
        index,
        run,
        key_hex,
        sealed.key_id.id(),
        tenant,
        sealed.expires_at,
    );
    Ok(())
}

/// Fold one shredded step into the burn set: the slot the reads refuse and
/// the sweep skips, with the per-run count the reads report.
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
        ErasedSlot {
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
        is_sealed_payload(payload).then(|| crate::journal::chain_over_bytes(previous, payload))
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

#[cfg(test)]
mod tests {
    use super::bump_frame_count;
    use crate::task::store::{MAX_RECORDS_PER_RUN, StoreError, StoreLimitKind};

    /// The per-run record ceiling admits exactly `MAX_RECORDS_PER_RUN` frames:
    /// the last one fits, the next is refused naming the count it reached.
    #[test]
    fn replay_frame_count_admits_the_ceiling_and_refuses_past_it() -> Result<(), StoreError> {
        assert_eq!(bump_frame_count(0)?, 1, "the first frame counts as one");
        let last = MAX_RECORDS_PER_RUN.saturating_sub(1);
        assert_eq!(
            bump_frame_count(last)?,
            MAX_RECORDS_PER_RUN,
            "the ceiling itself fits"
        );
        assert!(
            matches!(
                bump_frame_count(MAX_RECORDS_PER_RUN),
                Err(StoreError::Limit {
                    kind: StoreLimitKind::Records,
                    requested,
                    limit,
                }) if requested == MAX_RECORDS_PER_RUN.saturating_add(1) && limit == MAX_RECORDS_PER_RUN
            ),
            "one frame past the ceiling is refused"
        );
        Ok(())
    }
}
