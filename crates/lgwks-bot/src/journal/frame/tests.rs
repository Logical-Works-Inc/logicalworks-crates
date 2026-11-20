//! Unit tests for the journal frame codec, moved verbatim out of frame.rs.

#[cfg(test)]
use super::{
    Digest, HEAD_BYTES, LENGTH_BYTES, Piece, Prefix, SaturatingFrom, declared_length, encode,
    framed_len, is_possible_length, read_piece, read_prefix, writable_length,
};
use std::io::Cursor;
use std::slice::SliceIndex;

/// These tests mix two error domains — the reader's `io::Error` and the
/// fixture's own `&str` — so they report `Box<dyn Error>` and propagate each
/// with `?` rather than reducing a failed read to a classification that
/// would read as a clean end.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// `items[range]`, or the refusal of a range the fixture did not size.
fn part<T>(items: &[T], range: impl SliceIndex<[T], Output = [T]>) -> Result<&[T], &'static str> {
    items
        .get(range)
        .ok_or("the fixture's range lies outside its bytes")
}

/// The head the first frame of a `chain_of` chained from.
fn first_head(from: &[Digest]) -> Result<&Digest, &'static str> {
    from.first().ok_or("chain_of made no heads")
}

/// A head, for the encode tests. Any 32 bytes; nothing here hashes.
fn head() -> Digest {
    Digest::from_bytes([7u8; 32])
}

/// The bytes a payload frames into.
fn framed(payload: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let length =
        writable_length(payload.len(), usize::MAX).ok_or("the grammar framed this payload")?;
    let out = encode(length, payload, &head());
    assert_eq!(
        usize::saturating_from(framed_len(payload.len())),
        out.len(),
        "a frame's bytes are exactly the length the grammar says a frame is"
    );
    Ok(out)
}

/// A frame round-trips: the prefix names the payload, and the head is last.
#[test]
fn a_frame_round_trips_through_the_shared_grammar() -> TestResult {
    let payload: Vec<u8> = (0..200u16)
        .map(u8::try_from)
        .collect::<Result<Vec<u8>, _>>()?;
    let bytes = framed(&payload)?;
    assert_eq!(part(&bytes, ..LENGTH_BYTES)?, &200u32.to_be_bytes());
    assert_eq!(
        part(&bytes, LENGTH_BYTES..LENGTH_BYTES + payload.len())?,
        &payload[..]
    );
    assert_eq!(part(&bytes, bytes.len() - HEAD_BYTES..)?, head().as_bytes());
    Ok(())
}

/// A clean end of file is `Eof`, and a whole prefix is `Full`.
#[test]
fn a_prefix_classifies_eof_full_and_torn() -> TestResult {
    let payload: Vec<u8> = vec![1u8, 2, 3];
    let bytes = framed(&payload)?;
    let mut clean = Cursor::new(Vec::new());
    let mut buffer = [0u8; LENGTH_BYTES];
    assert_eq!(read_prefix(&mut clean, &mut buffer)?, Prefix::Eof);

    let mut whole = Cursor::new(bytes);
    assert_eq!(read_prefix(&mut whole, &mut buffer)?, Prefix::Full);
    assert_eq!(declared_length(&buffer), 3);

    // Any prefix cut is an interrupted append, including one whose bytes are
    // zero: the count is what decides, never the values.
    for cut in 1..LENGTH_BYTES {
        let mut torn = Cursor::new(vec![0u8; cut]);
        let mut scratch = [0u8; LENGTH_BYTES];
        assert_eq!(read_prefix(&mut torn, &mut scratch)?, Prefix::Torn);
    }
    Ok(())
}

/// A frame cut inside a piece reads as `Interrupted`, whatever the piece is.
#[test]
fn a_frame_cut_inside_a_piece_is_interrupted() -> TestResult {
    let payload: Vec<u8> = vec![9u8; 16];
    let bytes = framed(&payload)?;
    // Each cut leaves fewer bytes than a whole-frame read would ask for, which is
    // what "the source ended before the buffer did" means: the read wanted more
    // than arrived, rather than wanting a fixed eight bytes that happened to fit.
    for cut in [LENGTH_BYTES + 1, LENGTH_BYTES + 8, bytes.len() - HEAD_BYTES] {
        let mut source = Cursor::new(part(&bytes, ..cut)?.to_vec());
        let wanted = bytes.len().saturating_sub(cut).saturating_add(1);
        let mut piece = vec![0u8; wanted];
        assert_eq!(read_piece(&mut source, &mut piece)?, Piece::Interrupted);
    }
    Ok(())
}

/// Only a non-zero length within the ceiling names a frame either store writes.
#[test]
fn only_a_possible_length_is_possible() {
    assert!(is_possible_length(1, 64 * 1024));
    assert!(is_possible_length(64 * 1024, 64 * 1024));
    assert!(!is_possible_length(0, 64 * 1024));
    assert!(!is_possible_length(64 * 1024 + 1, 64 * 1024));
}

/// The framed length is prefix plus payload plus head, and never overflows.
#[test]
fn a_frames_length_accounts_for_every_piece() {
    assert_eq!(framed_len(0), 36);
    assert_eq!(framed_len(100), 136);
    assert_eq!(framed_len(usize::MAX), u64::MAX);
}

/// A frame's bytes are exactly prefix, payload and head — the property both
/// stores' readers rely on when they compute a frame's length from its parts.
#[test]
fn a_frames_bytes_are_prefix_payload_and_head() -> TestResult {
    for size in [1usize, 64, 4096] {
        let payload = vec![7u8; size];
        let bytes = framed(&payload)?;
        assert_eq!(bytes.len(), LENGTH_BYTES + size + HEAD_BYTES);
        assert_eq!(
            part(&bytes, LENGTH_BYTES..LENGTH_BYTES + size)?,
            &payload[..]
        );
        assert_eq!(
            part(&bytes, bytes.len() - HEAD_BYTES..)?,
            head().as_bytes(),
            "the stored head is the frame's last {HEAD_BYTES} bytes"
        );
    }
    Ok(())
}

/// The head a store of plain chaining would store: a hash of the previous head
/// and the payload, over the bytes as written.
fn chained(previous: &Digest, payload: &[u8]) -> Digest {
    let mut hasher = lgwks_std::hash::Hasher::new();
    hasher.update(previous.as_bytes());
    hasher.update(payload);
    hasher.finalize()
}

/// Whether the bytes behind a prefix hold an acknowledged frame of the plain chain.
fn holds(behind_prefix: &[u8], previous: &Digest) -> bool {
    super::holds_acknowledged_frame(behind_prefix, previous, CEILING, |before, payload| {
        Some(chained(before, payload))
    })
}

/// `count` chained frames laid out end to end, and the head each chained from.
fn chain_of(payloads: &[&[u8]]) -> Result<(Vec<u8>, Vec<Digest>), Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    let mut previous = Digest::from_bytes([0u8; HEAD_BYTES]);
    let mut chained_from = Vec::new();
    for payload in payloads {
        let head = chained(&previous, payload);
        let length =
            writable_length(payload.len(), CEILING).ok_or("the grammar framed this payload")?;
        bytes.extend_from_slice(&encode(length, payload, &head));
        chained_from.push(previous);
        previous = head;
    }
    Ok((bytes, chained_from))
}

const CEILING: usize = 64 * 1024;

/// The cut frame under its true length authenticates, so the prefix lied; every
/// shorter cut of the same bytes is a prefix of an append and holds nothing.
#[test]
fn the_cut_frame_authenticates_only_when_it_is_whole_behind_a_lying_prefix() -> TestResult {
    let payload: Vec<u8> = (0u8..100).collect();
    let (bytes, from) = chain_of(&[&payload])?;
    let previous = *first_head(&from)?;
    let behind_prefix = part(&bytes, LENGTH_BYTES..)?;
    assert!(holds(behind_prefix, &previous));
    for cut in 0..behind_prefix.len() {
        assert!(
            !holds(part(behind_prefix, ..cut)?, &previous),
            "a frame cut {cut} bytes in, short of its head, is an interrupted append"
        );
    }
    Ok(())
}

/// A frame cut off from its head by one or more bytes, with its payload whole,
/// holds nothing: the head that would authenticate it is not there.
#[test]
fn a_whole_payload_with_a_short_head_authenticates_nothing() -> TestResult {
    let payload = vec![9u8; 64];
    let (bytes, from) = chain_of(&[&payload])?;
    let behind_prefix = part(&bytes, LENGTH_BYTES..)?;
    for missing in 1..=HEAD_BYTES {
        let cut = behind_prefix.len() - missing;
        assert!(!holds(part(behind_prefix, ..cut)?, first_head(&from)?));
    }
    Ok(())
}

/// With the cut frame itself damaged, a later whole frame still authenticates
/// against the head stored just before it.
#[test]
fn a_later_frame_authenticates_when_the_cut_frame_cannot() -> TestResult {
    let (bytes, from) = chain_of(&[&[1u8; 40], &[2u8; 50], &[3u8; 60]])?;
    let mut behind_prefix = part(&bytes, LENGTH_BYTES..)?.to_vec();
    *behind_prefix
        .get_mut(5)
        .ok_or("the frame has a sixth byte")? ^= 0x80;
    assert!(holds(&behind_prefix, first_head(&from)?));
    // The same bytes with the later frames cut away leave nothing to find.
    let first_only = part(&behind_prefix, ..40 + HEAD_BYTES)?;
    assert!(!holds(first_only, first_head(&from)?));
    Ok(())
}

/// With the cut frame's own head damaged and exactly one frame behind it, that
/// frame still chains from the head the cut frame's payload implies.
#[test]
fn a_frame_behind_a_damaged_head_chains_from_the_head_its_payload_implies() -> TestResult {
    let (bytes, from) = chain_of(&[&[1u8; 40], &[2u8; 50]])?;
    let mut behind_prefix = part(&bytes, LENGTH_BYTES..)?.to_vec();
    *behind_prefix
        .get_mut(40 + 7)
        .ok_or("the frame has a payload and a head")? ^= 0x01;
    assert!(holds(&behind_prefix, first_head(&from)?));
    Ok(())
}

/// Bytes that never framed anything hold nothing, at the ceiling and empty, and
/// the search answers in bounded time for the longest tail a short read can leave.
#[test]
fn noise_and_the_longest_possible_tail_hold_nothing() -> TestResult {
    let previous = Digest::from_bytes([5u8; HEAD_BYTES]);
    assert!(!holds(&[], &previous));
    // Noise, not a frame: every third byte is 250, which no `u32` length this
    // grammar writes could produce in that position.
    let longest: Vec<u8> = (0..CEILING + HEAD_BYTES)
        .map(|index| u8::saturating_from(index % 251))
        .collect();
    assert!(!holds(&longest, &previous));
    // A head function that refuses every payload can authenticate nothing.
    let (bytes, from) = chain_of(&[&[7u8; 30]])?;
    assert!(!super::holds_acknowledged_frame(
        part(&bytes, LENGTH_BYTES..)?,
        first_head(&from)?,
        CEILING,
        |_previous, _payload| None
    ));
    Ok(())
}
