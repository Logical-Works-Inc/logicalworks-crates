//! The capture-cut families: a cut prefix ends at the capture ceiling, room without a record is the ceiling, rot before the cut is the ending, and the tier sweep.

use super::*;

/// A framed read of a capture-cut prefix ends at the capture's ceiling, never at
/// a truncation the child performed and never at a clean end.
///
/// The child writes whole length-prefixed records whose total is drawn past the
/// capture ceiling, so the retained bytes are a prefix **the capture** cut. The
/// cut point is swept across every position inside a record — prefix, payload and
/// the exact boundary between two records — because the boundary is the case that
/// would otherwise read as a complete stream: the retained bytes end exactly
/// where a record does, and only the capture's own flag says the child wrote more.
///
/// Two tenants run the same shape with their own payloads, so a crossed pipe
/// would show up as the wrong bytes rather than as a wrong count.
fn capture_cuts_end_at_the_capture_ceiling(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        const PAYLOAD: usize = 16;
        const FRAME: usize = 4 + PAYLOAD;
        // Six frames = 120 bytes against a ceiling drawn from 20..60, so the
        // capture always cuts and the cut lands somewhere inside the stream. The
        // ceiling is drawn above the payload deliberately: a ceiling *below* it
        // would make the very first prefix declare a length past the reader's
        // ceiling, which is `MalformedPrefix` — a different fact about the same
        // bytes, and one this family is not about.
        let ceiling = usize::try_from(sim.rng().between(20, 60))?;
        let tenant: u32 = sim.rng().below(2);
        let byte = u8::try_from(tenant)?.wrapping_add(b'a');

        let outcome = run(&framed_capture(byte, PAYLOAD, 6, ceiling)?)?;
        assert!(
            outcome.stdout().truncated(),
            "a 6-frame {FRAME}-byte-per-frame write must overrun a {ceiling}-byte capture ceiling"
        );
        assert!(
            outcome.stdout().retained_capacity() <= ceiling,
            "the retained buffer must not grow past its own ceiling"
        );

        let frames = outcome.stdout().frames(ceiling);
        assert_eq!(
            frames.ended(),
            &FrameRead::CeilingReached {
                ceiling: outcome.stdout().retained_capacity()
            },
            "a capture-cut prefix ends at the capture's own ceiling, whatever its bytes \
             happen to end on: tenant={tenant} ceiling={ceiling}"
        );
        assert_eq!(
            ending_class(&frames),
            "ceiling-reached",
            "the ending's class is what a caller branches on, and a cut prefix is the ceiling"
        );
        assert!(
            !frames.is_complete(),
            "tenant={tenant} ceiling={ceiling}: a reading of a capture-cut prefix is not a \
             complete reading of the child's output"
        );
        assert!(
            !frames.ended().is_truncated(),
            "the capture cut the bytes; the child did not truncate a record"
        );
        // Only whole records are records, and only the frames that fit entirely
        // inside the retained prefix are among them. The model counts them from
        // the bytes rather than from the reader, so the two are checked against
        // each other.
        let retained = outcome.stdout().bytes().len();
        let expected = whole_frames(retained, FRAME);
        assert_eq!(
            frames.records().len(),
            expected,
            "tenant={tenant} ceiling={ceiling}: {retained} retained bytes hold {expected} \
             whole frames of {FRAME} bytes, got {:?}",
            frames.records()
        );
        for (index, record) in frames.records().iter().enumerate() {
            assert_eq!(
                record.payload(),
                Some(
                    std::iter::repeat_n(byte, PAYLOAD)
                        .collect::<Vec<_>>()
                        .as_slice()
                ),
                "tenant={tenant}: record {index} must decode to this tenant's own payload"
            );
        }
        sim.record("capture-cut");
        sim.trace.record_number("tenant", u64::from(tenant));
        sim.trace.record_number("ceiling", ceiling);
        sim.trace.record_number("retained", retained);
        sim.trace.record_number("frames", frames.records().len());
        sim.record(ending_class(&frames));
        Ok(())
    })
}

/// A legal record with no room left is the ceiling; only `0` and a length past the
/// ceiling are rot.
///
/// Swept across the boundary a real stream lands on: the ceiling is drawn and the
/// first record takes a seeded share of it, so the second record is sometimes
/// legal-and-fitting, sometimes legal-and-too-large, and the ending must be a
/// whole frame, the ceiling, or rot accordingly — never rot for the middle case.
fn room_without_a_record_is_the_ceiling(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        const CEILING: usize = 64 * 1024;
        // The *capture* ceiling is deliberately larger than the reader's. The two
        // are different bounds, and conflating them would make this family
        // observe the capture's cut instead of the reader's room: with a capture
        // at 64 KiB, a second record too large for the remaining room also
        // overran the capture, and the ending would be the capture's ceiling by
        // the rule `capture_cuts_end_at_the_capture_ceiling` pins. A capture that
        // always holds the whole stream leaves the reader's ceiling the only one
        // in play, which is what this row is about.
        const CAPTURE: usize = 128 * 1024;
        let first = usize::try_from(sim.rng().between(1024, 40 * 1024))?;
        // The second record is drawn against the room the first one leaves, so
        // both the fitting and the too-large cases are swept by construction.
        let room = CEILING.saturating_sub(first);
        let second = usize::try_from(sim.rng().between(1024, 48 * 1024))?;

        // One prefix per record, in stream order. They are literals because the four
        // bytes each names are the thing under test; the payloads come from
        // `head`, because two 40 KiB records spelled as octal escapes would be a
        // half-megabyte single argument, past what an `execve` accepts, and the
        // child would fail to start rather than exercise the reader.
        let prefix_of = |length: usize| -> Result<String, std::num::TryFromIntError> {
            Ok(u32::try_from(length)?
                .to_be_bytes()
                .iter()
                .map(|value| format!("\\{value:03o}"))
                .collect())
        };
        let script = format!(
            "printf '{first_prefix}'; head -c {first} /dev/zero | tr '\\0' '9'; \
             printf '{second_prefix}'; head -c {second} /dev/zero | tr '\\0' '9'",
            first_prefix = prefix_of(first)?,
            second_prefix = prefix_of(second)?,
        );
        let outcome = run(&captured(&script, CAPTURE)?)?;
        assert!(
            !outcome.stdout().truncated(),
            "both records must fit a {CAPTURE}-byte capture, or the ending would be the \
             capture's rather than the reader's"
        );
        // The child must have written exactly the two records the seed asked for.
        // A `head` or `tr` that delivered fewer bytes would read as a truncation
        // and be reported as one, so the count is pinned before the framing is.
        assert_eq!(
            outcome.stdout().total_bytes(),
            u64::try_from(first.saturating_add(second).saturating_add(8))?,
            "first={first} second={second}: the child wrote exactly two length-prefixed records"
        );

        let frames = outcome.stdout().frames(CEILING);
        let fits = second <= room;
        assert_eq!(
            frames.records().len(),
            if fits { 2 } else { 1 },
            "first={first} second={second} room={room}: {fits} decides whether the second \
             record is decoded at all"
        );
        assert_eq!(
            frames.ended(),
            if fits {
                &FrameRead::EndOfStream
            } else {
                &FrameRead::CeilingReached { ceiling: CEILING }
            },
            "first={first} second={second} room={room}: a legal record larger than the room \
             remaining is the ceiling, never malformed"
        );
        assert_eq!(
            frames.retained_bytes(),
            if fits {
                first.saturating_add(second)
            } else {
                first
            },
            "a record stopped by the ceiling is never charged: its payload was never read"
        );
        assert!(
            !matches!(frames.ended(), FrameRead::MalformedPrefix { .. }),
            "a well-formed record must never be reported as rot: first={first} second={second}"
        );

        // The two lengths no writer of this grammar produces, swept as a pair:
        // zero, and one past the ceiling. A reader that folded either into the
        // ceiling would accept a record the frame grammar refuses.
        for declared in [0_usize, CEILING + 1] {
            let mut rot: &[u8] = &u32::try_from(declared)?.to_be_bytes();
            let refused = lgwks_bot::rt::process::read_frames(&mut rot, CEILING)?;
            assert_eq!(
                refused.ended(),
                &FrameRead::MalformedPrefix {
                    declared,
                    ceiling: CEILING
                },
                "declared={declared} names no record this grammar writes, so it is rot"
            );
            assert_eq!(
                refused.retained_bytes(),
                0,
                "declared={declared}: a refused prefix is never charged"
            );
        }
        sim.record("room");
        sim.trace.record_number("first", first);
        sim.trace.record_number("second", second);
        sim.trace.record_number("room", room);
        sim.record(ending_class(&frames));
        Ok(())
    })
}

// Each band is a written-out `#[test]` rather than a macro declaration. The
// `invariants` lane resolves every name an invariant cites by scanning the
// sources for `fn` definitions, and a macro-generated test has no `fn` there — so
// a family declared through a macro runs but cannot be cited as evidence. The
// `simulation-evidence` lane reads the same way, counting `#[test]` attributes.
// `tests/sim_run_boundaries.rs` records the same constraint from the other side.

/// A seeded sweep of `capture_cuts_end_at_the_capture_ceiling` over seeds 72..80.
#[test]
fn capture_cuts_end_at_the_capture_ceiling_band_00() -> TestResult {
    capture_cuts_end_at_the_capture_ceiling(Band::new(72, 8))
}

/// A seeded sweep of `capture_cuts_end_at_the_capture_ceiling` over seeds 80..88.
#[test]
fn capture_cuts_end_at_the_capture_ceiling_band_01() -> TestResult {
    capture_cuts_end_at_the_capture_ceiling(Band::new(80, 8))
}

/// A seeded sweep of `room_without_a_record_is_the_ceiling` over seeds 88..96.
#[test]
fn room_without_a_record_is_the_ceiling_band_00() -> TestResult {
    room_without_a_record_is_the_ceiling(Band::new(88, 8))
}

/// A seeded sweep of `room_without_a_record_is_the_ceiling` over seeds 96..104.
#[test]
fn room_without_a_record_is_the_ceiling_band_01() -> TestResult {
    room_without_a_record_is_the_ceiling(Band::new(96, 8))
}

/// A seeded sweep of `rot_before_the_capture_cut_is_the_ending` over seeds
/// 104..112.
#[test]
fn rot_before_the_capture_cut_is_the_ending_band_00() -> TestResult {
    rot_before_the_capture_cut_is_the_ending(Band::new(104, 8))
}

/// A seeded sweep of `rot_before_the_capture_cut_is_the_ending` over seeds
/// 112..120.
#[test]
fn rot_before_the_capture_cut_is_the_ending_band_01() -> TestResult {
    rot_before_the_capture_cut_is_the_ending(Band::new(112, 8))
}

/// A sweep of `capture_cuts_end_at_the_capture_ceiling` at 100, 1 000 and 10 000
/// concurrent captures, and the named replay receipt over the tier sweep.
///
/// Each tier drives `min(requested, CAPTURE_CEILING)` **real** children at once,
/// each with its own supervisor and its own capture. The requested, reached and
/// ceiling levels are all reported (the INV-BOT-16 rule), so a tier this host
/// cannot reach says so rather than passing quietly: ten thousand real `sh`
/// children is a fork storm, and the honest claim is the tier that was reached.
#[test]
fn capture_cuts_saturate_at_the_declared_tiers() -> TestResult {
    const PAYLOAD: usize = 16;
    const RECORDS: usize = 6;
    const FRAME: usize = 4 + PAYLOAD;
    const CEILING: usize = 32;
    // The tier this host is asked to drive. Every child is a real process on a
    // real pipe, so the ceiling is what the OS will bear rather than an arbitrary
    // number; a reader that shared state across captures would fail here at any
    // tier, which is what makes the reached level sufficient evidence.
    const CAPTURE_CEILING: usize = 256;
    let expected_frames = whole_frames(CEILING, FRAME);

    for requested in [100_usize, 1_000, 10_000] {
        let level = requested.min(CAPTURE_CEILING);
        let specs: Vec<ProcessSpec> = (0..level)
            .map(|index| framed_capture(tenant_byte(index)?, PAYLOAD, RECORDS, CEILING))
            .collect::<Result<Vec<_>, _>>()?;
        let runtime = lgwks_bot::rt::runtime::Runtime::new()?;
        // Each child gets its own `Supervisor::new(1)` *inside its own async
        // block*, because `run_process` takes `&mut self`: a future that
        // borrowed a supervisor from an enclosing scope could not be stored in a
        // `Vec` and joined, and one supervisor shared across children would
        // serialise them — the opposite of the tier under test.
        let observations = runtime.block_on(async {
            let runs: Vec<_> = specs
                .iter()
                .map(|spec| async move {
                    let mut supervisor = Supervisor::new(1);
                    supervisor.run_process(spec).await
                })
                .collect();
            lgwks_std::task::join_all(runs).await
        });

        assert_eq!(
            observations.len(),
            level,
            "requested={requested} reached={level} ceiling={CAPTURE_CEILING}: every concurrent \
             run reported, so the tier really ran"
        );
        for (index, outcome) in observations.into_iter().enumerate() {
            let outcome = outcome?;
            let read = outcome.stdout().frames(CEILING);
            assert_eq!(
                read.ended(),
                &FrameRead::CeilingReached {
                    ceiling: outcome.stdout().retained_capacity()
                },
                "requested={requested} reached={level} child {index}: a capture-cut prefix ends \
                 at the capture's ceiling"
            );
            assert_eq!(
                read.records().len(),
                expected_frames,
                "requested={requested} child {index}: only the {expected_frames} whole frames \
                 inside a {CEILING}-byte capture are records"
            );
            let byte = tenant_byte(index)?;
            for (record, payload) in read
                .records()
                .iter()
                .filter_map(FrameRead::payload)
                .enumerate()
            {
                assert_eq!(
                    payload,
                    std::iter::repeat_n(byte, PAYLOAD)
                        .collect::<Vec<_>>()
                        .as_slice(),
                    "requested={requested} child {index}: record {record} must be this child's \
                     own payload byte, never another child's"
                );
            }
        }
    }
    Ok(())
}

/// Rot before the capture cut is the ending; the capture's ceiling only ends what
/// the cut itself reached.
///
/// Two tenants run the same shape with their own rot, so a crossed pipe shows up
/// as the wrong bytes rather than a wrong count. Per tenant the stream is: `pad`
/// whole records, then one **rot** record whose prefix declares a length no writer
/// of this grammar produces, then `tail` more whole records, so the capture cuts
/// somewhere in the stream. The seed picks whether the rot prefix itself was
/// retained whole, and the ending follows from that and from nothing else: the
/// child's rot when the capture kept all four of its bytes, otherwise the
/// capture's own ceiling.
///
/// The rot is drawn from the two lengths the grammar refuses — `0`, and one past
/// the reader's ceiling — because a *legal* record larger than the room remaining
/// is `room_without_a_record_is_the_ceiling`'s case and would make this family's
/// ending indistinguishable from a bound.
///
/// The cut and the reader's ceiling are the *same* number here, deliberately:
/// they are drawn together so the one thing this family separates — the rot the
/// capture retained from the bound it stopped at — is not confused by which of the
/// two happened to be smaller.
fn rot_before_the_capture_cut_is_the_ending(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        const PAYLOAD: usize = 8;
        const FRAME: usize = 4 + PAYLOAD;
        let pad = usize::try_from(sim.rng().below(3))?;
        let tenant: u32 = sim.rng().below(2);
        let byte = u8::try_from(tenant)?.wrapping_add(b'a');
        // Drawn *after* `pad` so that drawing it does not renumber the seeds, and
        // above both the rot's position and one whole record — so whether the rot
        // was retained whole is a real draw across the sweep rather than an
        // artefact of a bound smaller than the prefix.
        let ceiling = usize::try_from(sim.rng().between(24, 48))?;
        // Enough records after the rot that the cut lands past it whenever it
        // lands past it at all.
        let tail = usize::try_from(sim.rng().between(1, 4))?;
        let rot_at = pad.saturating_mul(FRAME).saturating_add(4);
        let declared = if sim.rng().chance(500) {
            0
        } else {
            ceiling.saturating_add(1)
        };

        let mut writer: Vec<u8> = Vec::new();
        let push_record =
            |writer: &mut Vec<u8>, declared: usize| -> Result<(), std::num::TryFromIntError> {
                writer.extend_from_slice(&u32::try_from(declared)?.to_be_bytes());
                writer.extend(std::iter::repeat_n(byte, declared));
                Ok(())
            };
        for _ in 0..pad {
            push_record(&mut writer, PAYLOAD)?;
        }
        push_record(&mut writer, declared)?;
        for _ in 0..tail {
            push_record(&mut writer, PAYLOAD)?;
        }
        let literal = writer
            .iter()
            .map(|value| format!("\\{value:03o}"))
            .collect::<String>();
        let outcome = run(&captured(&format!("printf '{literal}'"), ceiling)?)?;
        let retained = outcome.stdout().bytes().len();
        let capture_cut = outcome.stdout().truncated();

        // The model: the rot prefix is whole exactly when the capture retained all
        // four of its bytes. When it was, the pass stopped on it — the capture's
        // cut, if there was one, lies after it and cannot change that. When it was
        // not, the cut came first and the pass ran out of retained bytes instead.
        let rot_whole = retained >= rot_at;
        let frames = outcome.stdout().frames(ceiling);
        if rot_whole {
            assert_eq!(
                frames.ended(),
                &FrameRead::MalformedPrefix { declared, ceiling },
                "tenant={tenant} ceiling={ceiling} retained={retained}: the rot prefix at byte \
                 {rot_at} was read whole, so it is the child's own rot and the capture's cut \
                 after it does not replace it"
            );
            assert_eq!(
                frames.records().len(),
                pad,
                "tenant={tenant}: the {pad} records before the rot are whole, and the rot is \
                 never one of them"
            );
        } else {
            assert!(
                capture_cut,
                "tenant={tenant} ceiling={ceiling} retained={retained}: the model says the cut \
                 reached the pass rather than the rot, so the capture must have cut"
            );
            assert_eq!(
                frames.ended(),
                &FrameRead::CeilingReached {
                    ceiling: outcome.stdout().retained_capacity()
                },
                "tenant={tenant} ceiling={ceiling} retained={retained}: the cut fell short of \
                 the rot prefix, so the ending is the capture's own ceiling"
            );
        }
        assert!(
            !frames.is_complete(),
            "tenant={tenant} ceiling={ceiling}: the pass stopped short of the end whatever it \
             stopped on"
        );
        assert!(
            !frames.ended().payload().is_some(),
            "an ending the pass stopped on carries no payload"
        );
        sim.record("rot");
        sim.trace.record_number("tenant", u64::from(tenant));
        sim.trace.record_number("ceiling", ceiling);
        sim.trace.record_number("retained", retained);
        sim.trace.record_number("pad", pad);
        sim.trace.record_number("tail", tail);
        sim.trace.record_number("rot-at", rot_at);
        sim.trace.record_number("declared", declared);
        sim.trace
            .record_number("capture-cut", u64::from(capture_cut));
        sim.record(ending_class(&frames));
        Ok(())
    })
}
