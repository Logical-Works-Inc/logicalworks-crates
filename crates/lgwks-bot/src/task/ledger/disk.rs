//! The ledger on the disk: one entry written and folded under the owner's
//! index, and the replay that rebuilds that index from the file.
//!
//! The write and the replay share [`fold`], so a state reached by committing an
//! entry and the same state reached by reading it back are one value.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

use lgwks_std::hash::Digest;
use lgwks_std::wire::{WireError, from_bytes, to_bytes};

use super::{
    Control, Entry, Index, LEDGER_MAGIC, MAX_LEDGER_RECORD_BYTES, MAX_LEDGER_RECORDS_PER_RUN,
    genesis_head,
};
use crate::journal::frame::{self, Cursor, HEAD_BYTES, SaturatingFrom};
use crate::task::store::{StoreError, StoreLimitKind};

/// The write and the fold, in one ordered step, under the index the owner holds.
pub(super) fn write_entry(
    file: &mut File,
    index: &mut Index,
    entry: &Entry,
) -> Result<(), StoreError> {
    let previous = index.tail;
    let (framed, head) = frame(entry, &previous)?;
    let staged = u64::saturating_from(framed.len());
    let next = index
        .committed
        .checked_add(staged)
        .ok_or(StoreError::Limit {
            kind: StoreLimitKind::StoreBytes,
            requested: u64::MAX,
            limit: crate::task::MAX_STORE_BYTES,
        })?;
    // The length fence, before a byte moves, under the ledger's thread where no
    // other charge can overtake it: the length this handle indexed must be the
    // length on the disk.
    let on_disk = file.metadata().map_err(StoreError::storage)?.len();
    if on_disk != index.committed {
        let refusal = Err(StoreError::Corrupt { at: 0 });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "write_entry: returning an error to the caller");
        return refusal;
    }
    file.write_all(&framed).map_err(StoreError::storage)?;
    file.sync_all().map_err(StoreError::storage)?;
    index.committed = next;
    index.tail = head;
    fold(index, entry);
    Ok(())
}

/// Fold one entry's control state into `index`, creating the run's slot if this
/// is the first entry for it.
///
/// One function because the live write and the replay must agree exactly: a state
/// reached by committing an entry and the same state reached by reading it back
/// have to be the same value, or a ledger's answer would depend on whether the
/// reader crashed. The three counters are assigned rather than accumulated
/// because an entry is the *absolute* state, and the applied-ticket list is
/// extended rather than replaced for the same reason: a ticket applied by an
/// earlier entry is still applied.
fn fold(index: &mut Index, entry: &Entry) {
    let run = index.runs.entry(entry.run).or_insert_with(|| Control {
        tenant: entry.tenant.clone(),
        attempts: 0,
        spend: 0,
        epoch: 0,
        applied: Vec::new(),
    });
    run.tenant.clone_from(&entry.tenant);
    run.attempts = entry.attempts;
    run.spend = entry.spend;
    run.epoch = entry.epoch;
    if let Some(ref ticket) = entry.applied
        && !run.applied.contains(ticket)
    {
        run.applied.push(ticket.clone());
    }
}

/// The frame for one ledger entry: length, payload, head, through the shared
/// grammar.
pub(super) fn frame(entry: &Entry, previous: &Digest) -> Result<(Vec<u8>, Digest), StoreError> {
    frame::frame_record(
        entry,
        previous,
        MAX_LEDGER_RECORD_BYTES,
        |entry| {
            to_bytes::<WireError>(entry)
                .map(|bytes| bytes.as_ref().to_vec())
                .map_err(|cause| StoreError::Encoding { cause })
        },
        |entry, previous, _| entry.head_from(previous),
        |len| StoreError::Limit {
            kind: StoreLimitKind::RecordBytes,
            requested: u64::saturating_from(len),
            limit: u64::saturating_from(MAX_LEDGER_RECORD_BYTES),
        },
    )
}

/// One complete, decoded frame read from the ledger.
struct Framed {
    /// The entry the frame's payload decoded to.
    entry: Entry,
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
/// caller trims to `committed`. A complete prefix that names a frame this
/// ledger never writes, or a payload that does not decode, cannot be an
/// interrupted append and is refused, and so is a cut whose bytes hold an entry
/// this ledger acknowledged under another length (#262).
fn next_frame(file: &mut File, cursor: &Cursor<'_>) -> Result<Option<Framed>, StoreError> {
    let at = cursor.at;
    let corrupt = || StoreError::Corrupt { at };
    let ceiling = MAX_LEDGER_RECORD_BYTES;
    let entry_head = |previous: &Digest, payload: &[u8]| {
        let aligned = frame::decodable(payload);
        from_bytes::<Entry, WireError>(aligned.as_slice())
            .ok()
            .map(|entry| entry.head_from(previous))
    };
    let Some(raw) = frame::read_raw(
        file,
        cursor,
        ceiling,
        StoreError::storage,
        corrupt,
        entry_head,
    )?
    else {
        return Ok(None);
    };
    let entry = from_bytes::<Entry, WireError>(&raw.payload).map_err(|error| {
        lgwks_std::trace::debug!(?error, at, "next_frame: the payload did not decode");
        StoreError::Corrupt { at }
    })?;
    Ok(Some(Framed {
        entry,
        declared: raw.payload.len(),
        head: raw.head,
    }))
}

/// Read every committed entry, returning the index it implies.
pub(super) fn replay(file: &mut File) -> Result<Index, StoreError> {
    let total = file.metadata().map_err(StoreError::storage)?.len();
    file.seek(SeekFrom::Start(0)).map_err(StoreError::storage)?;
    let mut header = [0u8; LEDGER_MAGIC.len()];
    if !read_full(file, &mut header)? || header != *LEDGER_MAGIC {
        let refusal = Err(StoreError::NotAStore);
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay: returning an error to the caller");
        return refusal;
    }
    let mut index = Index {
        runs: HashMap::new(),
        committed: u64::saturating_from(LEDGER_MAGIC.len()),
        tail: genesis_head(),
    };
    let mut previous = genesis_head();
    let mut at = 0u64;
    while let Some(Framed {
        entry,
        head,
        declared,
    }) = next_frame(file, &Cursor::new(at, index.committed, &previous))?
    {
        if entry.head_from(&previous) != Digest::from_bytes(head) {
            let refusal = Err(StoreError::Corrupt { at });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay: returning an error to the caller");
            return refusal;
        }
        let frame_len = frame::framed_len(declared);
        if index
            .committed
            .checked_add(frame_len)
            .is_none_or(|end| end > total)
        {
            break;
        }
        index.committed = index.committed.saturating_add(frame_len);
        index.tail = entry.head_from(&previous);
        previous = index.tail;
        at = bump_entry_count(at)?;
        fold(&mut index, &entry);
    }
    Ok(index)
}

/// Count one more replayed entry, refusing past the per-run ledger ceiling.
fn bump_entry_count(at: u64) -> Result<u64, StoreError> {
    let at = at.saturating_add(1);
    if at > MAX_LEDGER_RECORDS_PER_RUN {
        let refusal = Err(StoreError::Limit {
            kind: StoreLimitKind::Records,
            requested: at,
            limit: MAX_LEDGER_RECORDS_PER_RUN,
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "replay: returning an error to the caller");
        return refusal;
    }
    Ok(at)
}

/// Read `buf` in full, reporting whether all of it arrived.
fn read_full(reader: &mut impl Read, mut buf: &mut [u8]) -> Result<bool, StoreError> {
    while !buf.is_empty() {
        match reader.read(buf) {
            Ok(0) => return Ok(false),
            Ok(read) => buf = buf.split_at_mut(read.min(buf.len())).1,
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::Write;

    use super::super::{Entry, Index, LEDGER_MAGIC, MAX_LEDGER_RECORDS_PER_RUN, genesis_head};
    use super::{bump_entry_count, replay, write_entry};
    use crate::effect::RunId;
    use crate::journal::frame::SaturatingFrom;
    use crate::journal::frame::probe::Scratch;
    use crate::task::store::StoreError;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A fresh ledger file holding only the magic, its scratch guard, and its
    /// empty index.
    fn ledger() -> Result<(File, Scratch, Index), Box<dyn std::error::Error>> {
        let guard = Scratch::new("ledger-disk")?;
        let mut file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(guard.path())?;
        file.write_all(LEDGER_MAGIC)?;
        let index = Index {
            runs: HashMap::new(),
            committed: u64::saturating_from(LEDGER_MAGIC.len()),
            tail: genesis_head(),
        };
        Ok((file, guard, index))
    }

    /// One entry for `run`, owned by `acme`.
    fn entry(run: RunId, attempts: u64, applied: Option<&[u8]>) -> Entry {
        Entry {
            run,
            tenant: "acme".to_owned(),
            attempts,
            spend: attempts.saturating_mul(10),
            epoch: u64::from(applied.is_some()),
            applied: applied.map(<[u8]>::to_vec),
        }
    }

    /// What the live write folds, the replay rebuilds: the same control state,
    /// the same committed length, the same tail, and an applied ticket kept once.
    #[test]
    fn ledger_disk_replay_rebuilds_what_the_write_folded() -> TestResult {
        let (mut file, _guard, mut index) = ledger()?;
        let run = RunId::from_hex(&format!("4{}", "0".repeat(31)))?;
        write_entry(&mut file, &mut index, &entry(run, 1, None))?;
        write_entry(&mut file, &mut index, &entry(run, 2, Some(b"ticket")))?;
        write_entry(&mut file, &mut index, &entry(run, 3, Some(b"ticket")))?;
        let replayed = replay(&mut file)?;
        assert_eq!(
            replayed.committed, index.committed,
            "the replay accounts for every byte"
        );
        assert_eq!(replayed.committed, file.metadata()?.len());
        assert_eq!(
            replayed.tail, index.tail,
            "the replay ends on the written tail"
        );
        assert_eq!(replayed.runs.get(&run), index.runs.get(&run));
        let control = replayed.runs.get(&run).ok_or("the run was folded")?;
        assert_eq!(control.attempts, 3, "the last entry is the absolute state");
        assert_eq!(
            control.applied,
            vec![b"ticket".to_vec()],
            "a ticket is applied once"
        );
        Ok(())
    }

    /// The ledger ceiling admits exactly `MAX_LEDGER_RECORDS_PER_RUN` entries:
    /// the last one fits, the next is refused naming the count it reached.
    #[test]
    fn ledger_disk_entry_count_admits_the_ceiling_and_refuses_past_it() -> TestResult {
        assert_eq!(bump_entry_count(0)?, 1, "the first entry counts as one");
        let last = MAX_LEDGER_RECORDS_PER_RUN.saturating_sub(1);
        assert_eq!(
            bump_entry_count(last)?,
            MAX_LEDGER_RECORDS_PER_RUN,
            "the ceiling fits"
        );
        assert!(
            matches!(
                bump_entry_count(MAX_LEDGER_RECORDS_PER_RUN),
                Err(StoreError::Limit { requested, limit, .. })
                    if requested == MAX_LEDGER_RECORDS_PER_RUN.saturating_add(1)
                        && limit == MAX_LEDGER_RECORDS_PER_RUN
            ),
            "one entry past the ceiling is refused"
        );
        Ok(())
    }

    /// A torn tail is trimmed rather than read, and a file without the magic is
    /// no ledger at all.
    #[test]
    fn ledger_disk_trims_a_torn_tail_and_refuses_a_foreign_file() -> TestResult {
        let (mut file, _guard, mut index) = ledger()?;
        let run = RunId::from_hex(&format!("5{}", "0".repeat(31)))?;
        write_entry(&mut file, &mut index, &entry(run, 1, None))?;
        file.write_all(&[0, 0])?;
        let replayed = replay(&mut file)?;
        assert_eq!(
            replayed.committed, index.committed,
            "the torn bytes are not committed"
        );
        let (mut foreign, _foreign_guard, _) = ledger()?;
        foreign.set_len(0)?;
        foreign.write_all(b"not a run ledger!")?;
        assert!(
            matches!(replay(&mut foreign), Err(StoreError::NotAStore)),
            "a file without the magic is refused"
        );
        Ok(())
    }
}
