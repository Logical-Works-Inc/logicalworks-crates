//! One sealed record's bytes: the frame payload codec.
//!
//! The payload carries the tenant, the key identity, the wrapped data key,
//! the seal nonce, the ciphertext, the expiry and a caller-chosen bind tag
//! together, and the chain head is computed over all of it, so the head
//! commits to ciphertext *and* key identity. Decoding is strict — a wrong
//! magic, a wrong tag, a short buffer, a non-UTF-8 tenant, a bad bind or a
//! trailing byte is `None`, which the caller reports as a corrupt frame — and
//! encoding is infallible, because every length was checked against the frame
//! ceiling before the key was minted.
//!
//! The magic prefix is what tells a sealed payload from a plain record in a
//! file the reader opens: a sealed file holds only sealed payloads (plus the
//! journal's own checkpoint frames), and a plain file never holds one, so a
//! payload that crosses modes is refused rather than decoded as the other
//! kind. The bind tag is plaintext routing the sealed reader needs without
//! the key: the journal binds the BLAKE3 of the sealed event's key, so a
//! shredded event's slot stays burned against re-append, and the run store
//! binds the run and step identity, so a shredded step stays attributed to
//! its run. Both are covered by the chain head with the ciphertext, so a
//! swapped bind is a chain break rather than a misattribution.
//!
//! Crate-internal like the fold: the codec is a step of a sealed frame, not a
//! second reader callers could reach past it.

use std::time::Duration;

use crate::journal::frame::SaturatingFrom;

use super::envelope::NONCE_BYTES;

/// The magic every sealed payload starts with: what makes a sealed frame
/// this codec's, and a plain record never.
pub(crate) const SEALED_MAGIC: &[u8; 16] = b"lgwks-sealed-1\0\0";

/// A record frame's tag in its payload, behind the magic.
pub(crate) const TAG_RECORD: u8 = 1;

/// A tombstone frame's tag in its payload, behind the magic.
pub(crate) const TAG_TOMBSTONE: u8 = 2;

/// One decoded frame payload: a record or a tombstone.
pub(crate) enum DecodedFrame {
    /// A sealed record with everything a read needs.
    Record {
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
    },
    /// A tombstone naming a shredded record: its frame ordinal and its key
    /// identity. Both travel so a replay that finds one without the other
    /// refuses the file rather than guessing which record was shredded.
    Tombstone {
        /// The shredded record's frame ordinal.
        index: u64,
        /// The shredded record's key identity.
        key_id: [u8; 16],
    },
}

/// Whether `payload` is a sealed frame of either tag: the magic and a shaped
/// body, without the key. Used where no provider is at hand — the torn-tail
/// search and the plain readers' mode guard — so acknowledged sealed bytes
/// are never trimmed and plain readers never decode them as records.
pub(crate) fn is_sealed_payload(payload: &[u8]) -> bool {
    decode_frame(payload).is_some()
}

/// A cursor over a frame payload that refuses to run past its end.
///
/// Every multi-byte read goes through here, so a truncated payload is a
/// refusal rather than a panic: there is no indexing in the decoder beside
/// this one reader.
struct PayloadReader<'a> {
    /// The payload bytes still unread.
    rest: &'a [u8],
}

impl<'a> PayloadReader<'a> {
    /// Read over `bytes`.
    fn over(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }

    /// Take `count` bytes, or `None` when fewer remain.
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.rest.split_at_checked(count)?;
        self.rest = tail;
        Some(head)
    }

    /// Take one tag byte, or `None` at the end.
    fn tag(&mut self) -> Option<u8> {
        let (head, tail) = self.rest.split_first()?;
        self.rest = tail;
        Some(*head)
    }

    /// Whether every byte was consumed: a payload with trailing bytes is a
    /// shape this store never wrote.
    fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }
}

/// Take the sealed magic, or `None` when `reader` does not start with it.
fn take_magic(reader: &mut PayloadReader<'_>) -> Option<()> {
    let magic = reader.take(SEALED_MAGIC.len())?;
    if magic == SEALED_MAGIC.as_slice() {
        Some(())
    } else {
        None
    }
}

/// Take a big-endian `u16` length prefix, or `None` when fewer remain.
fn take_u16(reader: &mut PayloadReader<'_>) -> Option<usize> {
    let bytes = reader.take(2)?;
    let mut prefixed = [0_u8; 2];
    prefixed.copy_from_slice(bytes);
    Some(usize::from(u16::from_be_bytes(prefixed)))
}

/// Take a big-endian `u32` length prefix, or `None` when fewer remain.
fn take_u32(reader: &mut PayloadReader<'_>) -> Option<usize> {
    let bytes = reader.take(4)?;
    let mut prefixed = [0_u8; 4];
    prefixed.copy_from_slice(bytes);
    match u32::from_be_bytes(prefixed) {
        // `u32::MAX` is the saturation the encoder writes past its ceiling,
        // never a length it wrote: refusing it here keeps a saturated prefix
        // from asking for four gigabytes.
        u32::MAX => None,
        len => usize::try_from(len).ok(),
    }
}

/// Take the expiry flag and body, or `None` for any shape but absent or a
/// `u64` nanos count.
fn take_expiry(reader: &mut PayloadReader<'_>) -> Option<Option<Duration>> {
    match reader.tag()? {
        0 => Some(None),
        1 => {
            let nanos = reader.take(8)?;
            let mut prefixed = [0_u8; 8];
            prefixed.copy_from_slice(nanos);
            Some(Some(Duration::from_nanos(u64::from_be_bytes(prefixed))))
        }
        _ => None,
    }
}

/// Decode one frame payload into a record or a tombstone.
///
/// `None` for any shape this codec never writes: a missing magic, a wrong
/// tag, a short buffer, a saturated length, a non-UTF-8 tenant, a trailing
/// byte. The caller reports the frame as corrupt; the shape carries no error
/// of its own because a damaged frame has no facts worth naming.
pub(crate) fn decode_frame(payload: &[u8]) -> Option<DecodedFrame> {
    let mut reader = PayloadReader::over(payload);
    take_magic(&mut reader)?;
    match reader.tag()? {
        TAG_RECORD => {
            let tenant_len = take_u16(&mut reader)?;
            let tenant = reader.take(tenant_len)?;
            let tenant = String::from_utf8(tenant.to_vec()).ok()?;
            let key_id = <[u8; 16]>::try_from(reader.take(16)?).ok()?;
            let wrapped_len = take_u32(&mut reader)?;
            let wrapped = reader.take(wrapped_len)?.to_vec();
            let nonce = <[u8; NONCE_BYTES]>::try_from(reader.take(NONCE_BYTES)?).ok()?;
            let ciphertext_len = take_u32(&mut reader)?;
            let ciphertext = reader.take(ciphertext_len)?.to_vec();
            let expires_at = take_expiry(&mut reader)?;
            let bind_len = take_u16(&mut reader)?;
            let bind = reader.take(bind_len)?.to_vec();
            if !reader.is_empty() {
                return None;
            }
            Some(DecodedFrame::Record {
                tenant,
                key_id,
                wrapped,
                nonce,
                ciphertext,
                expires_at,
                bind,
            })
        }
        TAG_TOMBSTONE => {
            let index = reader.take(8)?;
            let mut prefixed = [0_u8; 8];
            prefixed.copy_from_slice(index);
            let index = u64::from_be_bytes(prefixed);
            let key_id = <[u8; 16]>::try_from(reader.take(16)?).ok()?;
            if !reader.is_empty() {
                return None;
            }
            Some(DecodedFrame::Tombstone { index, key_id })
        }
        _ => None,
    }
}

/// Encode one record payload. Infallible: every length here was checked
/// against the frame ceiling before the key was minted, so nothing below can
/// fail — and infallible code has no refusal to forget to log.
///
/// Lengths saturate through the shared [`SaturatingFrom`] step rather than
/// truncating: saturation names the ceiling the frame bound then refuses,
/// where a truncated `as` cast would admit a small number nobody chose.
pub(crate) fn encode_record(
    tenant: &str,
    key_id: &[u8; 16],
    wrapped: &[u8],
    nonce: &[u8; NONCE_BYTES],
    ciphertext: &[u8],
    expires_at: Option<Duration>,
    bind: &[u8],
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(
        ciphertext
            .len()
            .saturating_add(wrapped.len())
            .saturating_add(bind.len())
            .saturating_add(128),
    );
    payload.extend_from_slice(SEALED_MAGIC.as_slice());
    payload.push(TAG_RECORD);
    payload.extend_from_slice(&u16::saturating_from(tenant.len()).to_be_bytes());
    payload.extend_from_slice(tenant.as_bytes());
    payload.extend_from_slice(key_id);
    payload.extend_from_slice(&u32::saturating_from(wrapped.len()).to_be_bytes());
    payload.extend_from_slice(wrapped);
    payload.extend_from_slice(nonce);
    payload.extend_from_slice(&u32::saturating_from(ciphertext.len()).to_be_bytes());
    payload.extend_from_slice(ciphertext);
    match expires_at {
        None => payload.push(0),
        Some(at) => {
            payload.push(1);
            payload.extend_from_slice(&u64::saturating_from(at.as_nanos()).to_be_bytes());
        }
    }
    payload.extend_from_slice(&u16::saturating_from(bind.len()).to_be_bytes());
    payload.extend_from_slice(bind);
    payload
}

/// Encode one tombstone payload: the magic, the tag, the shredded record's
/// frame ordinal, and its key identity.
pub(crate) fn encode_tombstone(index: u64, key_id: &[u8; 16]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(SEALED_MAGIC.len().saturating_add(25));
    payload.extend_from_slice(SEALED_MAGIC.as_slice());
    payload.push(TAG_TOMBSTONE);
    payload.extend_from_slice(&index.to_be_bytes());
    payload.extend_from_slice(key_id);
    payload
}
