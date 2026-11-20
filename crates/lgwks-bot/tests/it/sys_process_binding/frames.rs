//! The length-prefixed frame grammar over a captured stream: a cut record, a
//! clean end, the capture's ceiling against the reader's, and rot.

use lgwks_bot::rt::process::{FrameRead, ProcessRun, read_frames};
use lgwks_bot::rt::supervise::Supervisor;

use super::{TestResult, captured_shell};

/// The capture ceiling the capture-cut tests share: smaller than any of their
/// streams, so the capture always cuts.
const SMALL_CAPTURE: usize = 8;

/// Run `script` once on a one-slot supervisor, capturing at most `capture` bytes
/// of each stream.
fn run_captured(script: &str, capture: usize) -> Result<ProcessRun, Box<dyn std::error::Error>> {
    let runtime = lgwks_bot::rt::runtime::Runtime::new()?;
    Ok(runtime.block_on(async {
        let mut supervisor = Supervisor::new(1);
        supervisor
            .run_process(&captured_shell(script, capture))
            .await
    })?)
}

/// A script printing `count` whole framed records, each the four-byte prefix
/// `[0, 0, 0, 2]` and the payload `ok`.
fn ok_records(count: usize) -> String {
    format!("printf '{}'", "\\000\\000\\000\\002ok".repeat(count))
}

/// T05, framed half: a record cut off mid-frame is a typed refusal, never a
/// decoded success.
///
/// The child emits one whole length-prefixed record and then a second whose
/// payload stops half-way. A reader that trusted the prefix would report the
/// short payload as the record; this one refuses it, and says how much arrived.
///
/// Read through `ProcessRun::stdout().frames(..)`, the door a caller reads its
/// own child's output through, rather than over a hand-plumbed slice — so what is
/// under test is the path a real caller takes.
#[test]
fn a_framed_record_cut_off_mid_frame_is_a_typed_refusal() -> TestResult {
    // `[0,0,0,4]` then `done`, then `[0,0,0,8]` and only `part`.
    let script = "printf '\\000\\000\\000\\004done\\000\\000\\000\\010part'";
    let runtime = lgwks_bot::rt::runtime::Runtime::new()?;
    let run = runtime.block_on(async {
        let mut supervisor = Supervisor::new(1);
        supervisor.run_process(&captured_shell(script, 4096)).await
    })?;
    assert_eq!(
        run.exit_code(),
        Some(0),
        "the child completed its own work; the framing is what is under test"
    );
    assert!(
        !run.stdout().truncated(),
        "a {}-byte write under a 4096-byte ceiling must retain the whole stream",
        run.stdout().total_bytes()
    );

    let frames = run.stdout().frames(4096);
    assert_eq!(
        frames.records().len(),
        1,
        "only the record that arrived whole is a frame: {:?}",
        frames.records()
    );
    assert_eq!(
        frames.records().first().and_then(|record| record.payload()),
        Some(&b"done"[..]),
        "the whole record decodes to exactly what the child wrote"
    );
    assert_eq!(
        frames.ended(),
        &FrameRead::TruncatedPayload {
            declared: 8,
            partial: b"part".to_vec(),
        },
        "a prefix naming eight bytes that delivered four is a truncation, and it names \
         both the declared length and what arrived"
    );
    assert!(
        !frames.ended().is_frame(),
        "the cut record must never read as a decoded frame"
    );
    assert!(
        frames.ended().payload().is_none(),
        "a refusal carries no payload, so there is no path from a cut record to bytes"
    );
    assert!(
        !frames.is_complete(),
        "a stream whose last record was cut off has no complete reading"
    );
    Ok(())
}

/// The same stream read whole is a complete reading, so the refusal above is
/// about the bytes rather than about the reader.
///
/// The control the truncation test needs: without a positive case, "always
/// refuse" would satisfy it. Here every record arrives, the capture held the
/// whole output, and the pass reports the clean end, the whole count, and no
/// truncation.
#[test]
fn a_framed_stream_that_ends_cleanly_is_complete() -> TestResult {
    let script = "printf '\\000\\000\\000\\004done\\000\\000\\000\\002ok'";
    let runtime = lgwks_bot::rt::runtime::Runtime::new()?;
    let run = runtime.block_on(async {
        let mut supervisor = Supervisor::new(1);
        supervisor.run_process(&captured_shell(script, 4096)).await
    })?;

    let frames = run.stdout().frames(4096);
    assert_eq!(
        frames.ended(),
        &FrameRead::EndOfStream,
        "a stream that ended between records is a complete read, not a failure"
    );
    assert!(frames.is_complete(), "the whole stream was decoded");
    assert!(!frames.ended().is_refusal(), "a clean end is not a refusal");
    let payloads: Vec<&[u8]> = frames
        .records()
        .iter()
        .filter_map(FrameRead::payload)
        .collect();
    assert_eq!(
        payloads,
        vec![&b"done"[..], &b"ok"[..]],
        "both records decode to exactly what the child wrote"
    );
    assert_eq!(
        frames.retained_bytes(),
        6,
        "every payload byte is accounted for"
    );
    Ok(())
}

/// D2: the capture's own cut is reported as the ceiling, never as the child's
/// truncation and never as a clean end.
///
/// The child writes three whole records and the capture ceiling holds only enough
/// for the first, so the retained bytes are a prefix **the capture** cut. A
/// framed read of them cannot see the child's whole output however those bytes
/// happen to end — here they end exactly on a record boundary, which is the case
/// that would otherwise read as a complete stream. The ending must therefore be
/// `CeilingReached` carrying the capture's retained capacity, and the pass must
/// not claim completeness.
#[test]
fn a_capture_ceiling_ends_the_framed_read_rather_than_the_child() -> TestResult {
    // Three whole two-byte records: 4 + 3 + 3 = 10 bytes, and a capture of 8
    // stops inside the second record's prefix. The two complete records before
    // the cut are decoded; the third never arrived within the ceiling.
    let script = ok_records(3);
    let run = run_captured(&script, SMALL_CAPTURE)?;
    assert!(
        run.stdout().truncated(),
        "the child wrote more than the {SMALL_CAPTURE}-byte ceiling, so the capture cut it"
    );

    let frames = run.stdout().frames(SMALL_CAPTURE);
    assert_eq!(
        frames.ended(),
        &FrameRead::CeilingReached {
            ceiling: run.stdout().retained_capacity(),
        },
        "the retained bytes are a prefix the capture cut, so the ending is the capture's \
         ceiling and never a child truncation"
    );
    assert_ne!(
        frames.ended(),
        &FrameRead::EndOfStream,
        "a pass over a truncated capture must never claim a clean end"
    );
    assert!(
        !frames.is_complete(),
        "a reading of a capture-cut prefix is not a complete reading of the child"
    );
    assert!(
        !frames.ended().is_truncated(),
        "the capture cut the bytes; the child did not truncate a record"
    );
    let whole = frames
        .records()
        .iter()
        .filter(|record| record.is_frame())
        .count();
    assert_eq!(
        whole,
        1,
        "only whole frames are records, even when the capture cut between them: {:?}",
        frames.records()
    );
    assert!(
        frames
            .records()
            .iter()
            .all(|record| record.payload().is_some()),
        "every retained record is a whole one, so every one has its payload"
    );
    Ok(())
}

/// Rot the capture retained stands: a capture's cut never launders a child's
/// malformed prefix into the capture's own ceiling.
///
/// The child writes a **zero-length prefix** — a length no writer of this grammar
/// produces — and then 32 bytes, against an 8-byte capture. Only eight of those
/// bytes were retained, so *some* of what a reader sees is a prefix the capture
/// cut; the first four are not. They are a whole prefix that named a zero-length
/// record, and that is rot in the child's output, decided entirely from bytes the
/// capture did retain. An implementation that reports every ending as the
/// capture's ceiling replaces that rot with a bound the caller did not hit, which
/// is the fail-open direction: a caller looking for corruption would see a
/// ceiling and go looking for a large record instead.
#[test]
fn a_rot_prefix_before_the_capture_cut_stays_refused() -> TestResult {
    // `\000\000\000\000` is the prefix `[0, 0, 0, 0]` — a declared length of zero.
    // 32 more bytes follow, so the 8-byte capture certainly cut.
    let script = "printf '\\000\\000\\000\\000'; head -c 32 /dev/zero | tr '\\0' 'x'";
    let run = run_captured(script, SMALL_CAPTURE)?;
    assert!(
        run.stdout().truncated(),
        "36 bytes into an 8-byte capture, so the retained bytes are a prefix the capture cut"
    );
    assert_eq!(
        run.stdout().bytes().len(),
        SMALL_CAPTURE,
        "the whole 8-byte window is retained, so every prefix inside it was read whole"
    );

    let frames = run.stdout().frames(4096);
    assert_eq!(
        frames.ended(),
        &FrameRead::MalformedPrefix {
            declared: 0,
            ceiling: 4096,
        },
        "the capture cut *later* bytes; the zero-length prefix before the cut was read whole \
         and is rot in the child's output, so it stands"
    );
    assert!(
        !frames.is_complete(),
        "a reading of a capture-cut prefix is never a complete reading of the child"
    );
    Ok(())
}

/// The same cut, with a reader ceiling the reader itself reached first: the
/// reader's own `CeilingReached` stands rather than the capture's.
///
/// The stream is four whole records of a two-byte payload, so three of them fit
/// inside the capture and charge the reader exactly its 6-byte ceiling — the
/// ceiling counts payload bytes, not stream bytes — while the fourth is cut. The
/// reader therefore stops on its own bound, having read only bytes the capture
/// retained in full, and never reaches the prefix the cut fell in the middle of.
/// Which ceiling stopped the pass is the fact a caller acts on, and a capture cut
/// that happened to truncate as well must not replace the reader's answer with
/// the capture's capacity.
#[test]
fn a_reader_ceiling_over_a_truncated_capture_is_the_readers_own() -> TestResult {
    const CAPTURE: usize = 20;
    const READER: usize = 6;
    // `[0,0,0,2] ok` six bytes each, four of them: 24 bytes against a 20-byte
    // capture. The three whole records inside the capture are exactly the reader's
    // ceiling of six payload bytes, and the fourth is the cut.
    let script = ok_records(4);
    let run = run_captured(&script, CAPTURE)?;
    assert!(
        run.stdout().truncated(),
        "24 bytes into a 20-byte capture, so the capture did cut its retained prefix"
    );
    assert!(
        run.stdout().retained_capacity() > READER,
        "the capture's own capacity must exceed the reader's, or the two ceilings could \
         never be told apart"
    );

    let frames = run.stdout().frames(READER);
    assert_eq!(
        frames.ended(),
        &FrameRead::CeilingReached { ceiling: READER },
        "the reader charged its own ceiling and stopped before the capture's cut could matter"
    );
    assert_eq!(
        frames.retained_bytes(),
        READER,
        "every byte the reader retained is accounted for at its own ceiling"
    );
    Ok(())
}

/// The control for the case above: a capture that retained the child's whole
/// output reports the child's own truncation, not the capture's ceiling.
///
/// The two tests differ in one fact only — whether the capture was truncated —
/// and the readings must differ accordingly. Without this, an implementation that
/// always answered `CeilingReached` would satisfy the test above.
#[test]
fn an_untruncated_capture_reports_the_child_own_truncation() -> TestResult {
    let script = "printf '\\000\\000\\000\\004done\\000\\000\\000\\010part'";
    let runtime = lgwks_bot::rt::runtime::Runtime::new()?;
    let run = runtime.block_on(async {
        let mut supervisor = Supervisor::new(1);
        supervisor.run_process(&captured_shell(script, 4096)).await
    })?;
    assert!(
        !run.stdout().truncated(),
        "the whole 16-byte write fits a 4096-byte ceiling, so the capture did not cut it"
    );

    let frames = run.stdout().frames(4096);
    assert_eq!(
        frames.ended(),
        &FrameRead::TruncatedPayload {
            declared: 8,
            partial: b"part".to_vec(),
        },
        "with the whole stream retained, the ending is the truncation the child performed"
    );
    assert_ne!(
        frames.ended(),
        &FrameRead::CeilingReached {
            ceiling: run.stdout().retained_capacity()
        },
        "an untruncated capture must not report the capture's ceiling"
    );
    Ok(())
}

/// D3: a legal record that does not fit the remaining room is the ceiling, not
/// rot, while a length past the ceiling in force is still refused.
///
/// Both halves in one test because they are the same decision seen from two
/// sides: a well-formed 40 KiB record is a record, and only once 40 of the 64 KiB
/// are spent does the second one have nowhere to go. Reading that as
/// `MalformedPrefix` would report a child's legal record as corruption.
#[test]
fn a_legal_record_without_room_is_the_ceiling_and_rot_is_still_refused() -> TestResult {
    const CEILING: usize = 64 * 1024;
    const RECORD: usize = 40 * 1024;
    // Two valid records of 40 KiB each: the first fits, the second has 24 KiB of
    // room left and declares 40, so it is a well-formed record with nowhere to go.
    let mut bytes: Vec<u8> = Vec::new();
    for _ in 0..2 {
        bytes.extend_from_slice(&u32::try_from(RECORD)?.to_be_bytes());
        bytes.extend(std::iter::repeat_n(7_u8, RECORD));
    }
    let mut stream: &[u8] = &bytes;
    let frames = read_frames(&mut stream, CEILING)?;
    assert_eq!(
        frames.records().len(),
        1,
        "the first whole record is decoded; the second is refused before its payload"
    );
    assert_eq!(
        frames
            .records()
            .first()
            .and_then(|record| record.payload())
            .map(<[u8]>::len),
        Some(RECORD),
        "the decoded record is exactly the length its prefix declared"
    );
    assert_eq!(
        frames.ended(),
        &FrameRead::CeilingReached { ceiling: CEILING },
        "a legal record larger than the room remaining is the ceiling, not malformed"
    );
    assert_eq!(
        frames.retained_bytes(),
        RECORD,
        "a record stopped by the ceiling is never charged: its payload was never read"
    );

    // The two lengths that name no record this grammar writes: zero, and one past
    // the ceiling in force.
    for (declared, why) in [
        (0_usize, "a zero-length record is never written"),
        (
            CEILING + 1,
            "a length past the ceiling names no record this reader writes",
        ),
    ] {
        let mut rot: &[u8] = &u32::try_from(declared)?.to_be_bytes();
        let refused = read_frames(&mut rot, CEILING)?;
        assert_eq!(
            refused.ended(),
            &FrameRead::MalformedPrefix {
                declared,
                ceiling: CEILING
            },
            "{why}, so it is refused before a payload byte is read"
        );
        assert_eq!(
            refused.retained_bytes(),
            0,
            "{why}: a refused prefix is never charged"
        );
    }
    Ok(())
}

/// A prefix declaring a length past the reader's ceiling is refused, and never
/// allocated for.
///
/// The half of the framing contract that is about this crate rather than the
/// child: a complete prefix naming more than the caller declared is refused
/// before a byte of it is read, so a stream cannot ask for an allocation by
/// claiming a large record.
#[test]
fn a_prefix_past_the_ceiling_is_refused_before_it_is_allocated() -> TestResult {
    let mut stream: &[u8] = &[0, 0, 0, 8, 1, 2, 3, 4, 5, 6, 7, 8];
    let frames = read_frames(&mut stream, 4)?;
    assert_eq!(
        frames.ended(),
        &FrameRead::MalformedPrefix {
            declared: 8,
            ceiling: 4,
        },
        "a complete prefix naming more than the ceiling is refused, with both numbers"
    );
    assert_eq!(
        frames.retained_bytes(),
        0,
        "a refused record is never charged, because it was never read"
    );
    Ok(())
}
