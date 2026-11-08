//! Reading a record journal on open: every frame judged the way the writer framed it.

use std::fs::File;
use std::io::BufReader;

use super::super::file::{MAX_FRAME_BYTES, verify_frame};
use super::super::frame::{
    HEAD_BYTES, LENGTH_BYTES, Piece, Prefix, SaturatingFrom, cut_holds_acknowledged,
    declared_length, decodable, framed_len, is_possible_length, read_piece, read_prefix,
};
use super::super::{CorruptionKind, JournalError, JournalPosition, chain_over_bytes};
use super::{Durable, RECORD_TAG, SEAL_TAG, bound_events, refuse};

/// The bytes a seal's payload holds before its checkpoint record.
pub(super) const SEAL_HEADER: usize = 1 + 8 + HEAD_BYTES;

/// What one payload says it is.
pub(super) enum Body<'payload> {
    /// A consumer's record.
    Record(&'payload [u8]),
    /// A seal: the position it continues and the checkpoint it carries.
    Seal {
        /// The tail of the file this seal closed.
        predecessor: JournalPosition,
        /// The checkpoint's archived bytes.
        record: &'payload [u8],
    },
}

/// Parse a payload's tag and header, or `None` for bytes no writer laid out.
pub(super) fn parse_body(payload: &[u8]) -> Option<Body<'_>> {
    let (tag, rest) = payload.split_first()?;
    match *tag {
        RECORD_TAG => Some(Body::Record(rest)),
        SEAL_TAG => {
            let (sequence, after) = rest.split_first_chunk::<8>()?;
            let (head, record) = after.split_first_chunk::<HEAD_BYTES>()?;
            Some(Body::Seal {
                predecessor: JournalPosition {
                    sequence: u64::from_be_bytes(*sequence),
                    head: lgwks_std::hash::Digest::from_bytes(*head),
                },
                record,
            })
        }
        _ => None,
    }
}

/// Whether `bytes` are an archive of `R`, read from an aligned copy when they must be.
pub(super) fn record_decodes<R: Durable>(bytes: &[u8]) -> bool {
    R::from_record(decodable(bytes).as_slice()).is_ok()
}

/// Whether `payload` is one this store's writer could have framed.
///
/// The tail search tries every length under the bytes present, so a wrong length
/// has to fail here rather than pay a hash, as the effect journal's search does.
fn frame_decodes<R: Durable>(payload: &[u8]) -> bool {
    match parse_body(payload) {
        Some(Body::Record(bytes) | Body::Seal { record: bytes, .. }) => record_decodes::<R>(bytes),
        None => false,
    }
}

/// How a scan ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stop {
    /// A clean end at this offset.
    Complete(u64),
    /// The bytes from this offset are an append that never finished.
    Torn(u64),
}

/// Where this file's own seal frame lies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SealAt {
    /// The offset of its length prefix.
    pub(super) offset: u64,
    /// The bytes it occupies.
    pub(super) length: u64,
    /// The position the file was at before it.
    pub(super) before: JournalPosition,
}

/// What a scan found.
pub(super) struct Scanned {
    /// Where each entry's frame begins, the carried seal included.
    pub(super) offsets: Vec<u64>,
    /// What this file's first frame chains from.
    pub(super) base: JournalPosition,
    /// Whether the first frame is a predecessor's seal.
    pub(super) carried: bool,
    /// Whether this file's name says it continues a predecessor.
    generation: bool,
    /// The last position verified.
    pub(super) tail: JournalPosition,
    /// This file's closing seal.
    pub(super) seal: Option<SealAt>,
    /// Where the scan stopped.
    pub(super) stop: Stop,
}

impl Scanned {
    /// How many frames have been read, which locates a refusal.
    fn frames(&self) -> u64 {
        u64::saturating_from(self.offsets.len()).saturating_add(u64::from(self.seal.is_some()))
    }

    /// Decide one whole frame, and fold it in.
    fn admit<R: Durable>(
        &mut self,
        payload: &[u8],
        head: &[u8; HEAD_BYTES],
        offset: u64,
    ) -> Result<(), JournalError> {
        let index = self.frames();
        if self.seal.is_some() {
            return refuse(
                index,
                CorruptionKind::Sealed,
                "a frame follows this file's own seal",
            );
        }
        match parse_body(payload) {
            Some(Body::Record(bytes)) if record_decodes::<R>(bytes) => {
                self.tail = verify_frame(payload, head, self.tail, index)?;
                self.offsets.push(offset);
                Ok(())
            }
            Some(Body::Seal {
                predecessor,
                record,
            }) if record_decodes::<R>(record) => {
                self.admit_seal(payload, head, offset, predecessor, index)
            }
            Some(_) | None => refuse(
                index,
                CorruptionKind::Undecodable,
                "the payload is no record",
            ),
        }
    }

    /// Fold a seal, which is legal first in a generation, as its predecessor's, or last,
    /// as this file's. A base file's first frame can be its own seal, when an empty
    /// journal sealed, so the file's name decides and the position alone cannot.
    fn admit_seal(
        &mut self,
        payload: &[u8],
        head: &[u8; HEAD_BYTES],
        offset: u64,
        predecessor: JournalPosition,
        index: u64,
    ) -> Result<(), JournalError> {
        if index == 0 && self.generation {
            self.tail = verify_frame(payload, head, predecessor, index)?;
            self.base = predecessor;
            self.carried = true;
            self.offsets.push(offset);
            return Ok(());
        }
        if predecessor != self.tail {
            return refuse(
                index,
                CorruptionKind::Sealed,
                "a seal names a tail that is not this file's",
            );
        }
        let before = self.tail;
        self.tail = verify_frame(payload, head, before, index)?;
        self.seal = Some(SealAt {
            offset,
            length: framed_len(payload.len()),
            before,
        });
        Ok(())
    }
}

/// Read every frame from `reader`, stopping at the first that was never finished.
///
/// A partial length prefix is an append that never finished. A complete prefix
/// naming a frame no writer produces is refused, since the bytes behind it may be
/// acknowledged. An end of file inside a declared frame is decided by the stored
/// heads, through the shared grammar, and a frame that authenticates is refused
/// rather than trimmed. The loop is bounded by the file: every turn consumes a frame.
pub(super) fn scan<R: Durable>(
    reader: &mut BufReader<&mut File>,
    generation: bool,
) -> Result<Scanned, JournalError> {
    let mut scanned = Scanned {
        offsets: Vec::new(),
        base: JournalPosition::genesis(),
        carried: false,
        generation,
        tail: JournalPosition::genesis(),
        seal: None,
        stop: Stop::Complete(0),
    };
    let mut offset = 0_u64;
    loop {
        let mut prefix = [0u8; LENGTH_BYTES];
        match read_prefix(reader, &mut prefix).map_err(JournalError::Storage)? {
            Prefix::Eof => {
                scanned.stop = Stop::Complete(offset);
                return Ok(scanned);
            }
            Prefix::Torn => {
                scanned.stop = Stop::Torn(offset);
                return Ok(scanned);
            }
            Prefix::Full => bound_events(scanned.offsets.len(), 1)?,
        }
        let declared = declared_length(&prefix);
        if !is_possible_length(declared, MAX_FRAME_BYTES) {
            return refuse(
                scanned.frames(),
                CorruptionKind::Framed,
                "a length no writer produces",
            );
        }
        let mut payload = vec![0u8; declared];
        let mut head = [0u8; HEAD_BYTES];
        for piece in [&mut payload[..], &mut head[..]] {
            if let Piece::Interrupted = read_piece(reader, piece).map_err(JournalError::Storage)? {
                let lied = cut_holds_acknowledged(
                    reader,
                    offset,
                    &scanned.tail.head(),
                    MAX_FRAME_BYTES,
                    JournalError::Storage,
                    |previous, candidate| {
                        frame_decodes::<R>(candidate).then(|| chain_over_bytes(previous, candidate))
                    },
                )?;
                if lied {
                    return refuse(
                        scanned.frames(),
                        CorruptionKind::Framed,
                        "a length lies over an acknowledged frame",
                    );
                }
                scanned.stop = Stop::Torn(offset);
                return Ok(scanned);
            }
        }
        scanned.admit::<R>(&payload, &head, offset)?;
        offset = offset.saturating_add(framed_len(declared));
    }
}
