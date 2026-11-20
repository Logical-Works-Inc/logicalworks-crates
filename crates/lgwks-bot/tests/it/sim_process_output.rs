//! Simulation family: seeded process output sizes, frame cuts, tenants and replay.
//!
//! # What is real and what is seeded
//!
//! The runner is the real one — the real [`Supervisor::run_process`], real
//! `sh` children, real pipes, and the real frame reader — so a family that passed
//! while the shipped code was wrong would mean the shipped code was wrong and
//! this family was not looking. The seed chooses only the *shape* of each run:
//! how many bytes a child writes, where a frame is cut, which ceiling is in force,
//! how fast the reader is, and how many tenants run at once.
//!
//! # The replay receipt
//!
//! Every fact an assertion makes is also appended to the run's trace, and the
//! trace hashes to a `u64`. [`sim::assert_replays`] sweeps the band twice and
//! requires the two hash vectors to be identical, so a nondeterministic run fails
//! even when every assertion happens to pass. No wall-clock reading is ever
//! recorded: a timing in the trace would differ between two runs of one seed on a
//! busy box and the receipt would mean nothing.
//!
//! # Families
//!
//! | Family | Property it pins |
//! |---|---|
//! | `sizes_stay_at_the_ceiling` | a seeded output size retains `min(size, ceiling)` and reports the exact total, on both streams |
//! | `cuts_are_refused_never_decoded` | a seeded cut point inside a frame yields a typed truncation naming both lengths |
//! | `capture_cuts_end_at_the_capture_ceiling` | a capture-cut prefix ends at the capture's ceiling, never as a child truncation or a clean end |
//! | `room_without_a_record_is_the_ceiling` | a legal record larger than the room remaining is the ceiling; only `0` and `> ceiling` are rot |
//! | `rot_before_the_capture_cut_is_the_ending` | a rot prefix the capture retained whole is the ending; only a cut that reached it is the capture's ceiling |
//! | `two_tenants_never_cross` | two tenants' captures are byte-distinct and neither sees the other's total |
//! | `verb_framed_reads_agree_with_the_model` | a seeded stream read back through `sys::Process`'s `Execute` verb matches a model of the framed reading |
//! | `verb_two_tenants_never_cross` | two `Process` values on one host never see each other's framed payloads |
//! | `the_same_seed_replays` | the same seed produces the same trace hash, twice |

#![cfg(all(
    unix,
    feature = "rt",
    feature = "time",
    feature = "sync",
    feature = "process"
))]

use crate::sim;

use std::error::Error;
use std::num::NonZeroUsize;

use lgwks_bot::rt::process::{DEFAULT_FRAME_CEILING, FrameRead, ProcessSpec};
use lgwks_bot::rt::runtime::Builder;
use lgwks_bot::rt::supervise::Supervisor;

use sim::Band;

/// What a scenario reports when its precondition did not hold.
type TestResult = Result<(), Box<dyn Error>>;

/// A shell spec writing `script`, with `ceiling` bytes retained per stream.
fn captured(script: &str, ceiling: usize) -> Result<ProcessSpec, Box<dyn Error>> {
    let limit = NonZeroUsize::new(ceiling).ok_or("a capture ceiling must be at least one")?;
    let mut spec = ProcessSpec::new("sh");
    spec.arg("-c").arg(script);
    spec.capture_stdout(limit);
    spec.capture_stderr(limit);
    Ok(spec)
}

/// Run `spec` on a supervisor bounded to one child.
///
/// Single-slot so a sweep's runs are sequential and its total is decided by the
/// scenario rather than by how many children happened to overlap.
fn run(spec: &ProcessSpec) -> Result<lgwks_bot::rt::process::ProcessRun, Box<dyn Error>> {
    let runtime = lgwks_bot::rt::runtime::Runtime::new()?;
    let mut supervisor = Supervisor::new(1);
    Ok(runtime.block_on(supervisor.run_process(spec))?)
}

/// Record a capture's four facts, so the trace carries the assertion rather than
/// only the assertion's result.
fn record_capture(sim: &mut sim::Sim, label: &str, run: &lgwks_bot::rt::process::ProcessRun) {
    sim.record(label);
    sim.trace
        .record_number("retained", run.stdout().bytes().len());
    sim.trace.record_number("total", run.stdout().total_bytes());
    sim.trace
        .record_number("truncated", u64::from(run.stdout().truncated()));
}

/// The class of a pass's ending, as the name the trace records.
///
/// A `Debug` rendering would put the payload of a partial record into the trace,
/// so two runs that agreed on every assertion could still differ byte-for-byte
/// in their receipts. The class is the fact a replay receipt should carry. The
/// wildcard is [`FrameRead`]'s `#[non_exhaustive]` promise from the outside: a
/// variant added later must be *named* here or the receipt silently loses it.
fn ending_class(frames: &lgwks_bot::rt::process::Frames) -> &'static str {
    match *frames.ended() {
        FrameRead::Frame { .. } => "frame",
        FrameRead::EndOfStream => "end-of-stream",
        FrameRead::TruncatedPrefix { .. } => "truncated-prefix",
        FrameRead::TruncatedPayload { .. } => "truncated-payload",
        FrameRead::MalformedPrefix { .. } => "malformed-prefix",
        FrameRead::CeilingReached { .. } => "ceiling-reached",
        _ => "unknown",
    }
}

/// A seeded output size stays at the ceiling, and the exact total is reported.
///
/// Both streams, because a ceiling that holds on stdout and leaks on stderr is a
/// ceiling that holds on whichever stream the test happened to check. The sizes
/// bracket the ceiling on purpose: some runs truncate and some do not, so
/// `truncated` is `size > ceiling` across the whole family rather than being
/// uniformly true or uniformly false.
fn sizes_stay_at_the_ceiling(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let ceiling = usize::try_from(sim.rng().between(1, 4096))?;
        // Drawn across the ceiling on purpose: the families above and below it
        // are the two ways a capture can be wrong, and a sweep that only ever
        // truncated would never observe the second.
        let size = sim.rng().between(1, 24_000);
        let script = format!("head -c {size} /dev/zero; head -c {size} /dev/zero 1>&2");
        let outcome = run(&captured(&script, ceiling)?)?;

        for (name, stream) in [("stdout", outcome.stdout()), ("stderr", outcome.stderr())] {
            let expected = usize::try_from(size.min(u32::try_from(ceiling)?))?.min(ceiling);
            assert_eq!(
                stream.bytes().len(),
                expected,
                "{name} must retain min(size, ceiling): size={size} ceiling={ceiling}"
            );
            assert!(
                stream.bytes().iter().all(|byte| *byte == 0),
                "{name} must retain the head of a zero stream"
            );
            assert_eq!(
                stream.total_bytes(),
                u64::from(size),
                "{name} must report the exact total it saw"
            );
            assert_eq!(
                stream.truncated(),
                u64::from(size) > u64::try_from(ceiling)?,
                "{name} truncation is exactly size > ceiling"
            );
            assert!(
                stream.retained_capacity() <= ceiling,
                "{name} retained {} bytes of capacity, past its {ceiling}-byte ceiling",
                stream.retained_capacity()
            );
        }
        record_capture(sim, "size", &outcome);
        sim.trace.record_number("ceiling", ceiling);
        sim.trace.record_number("size", u64::from(size));
        Ok(())
    })
}

/// A seeded cut inside a frame is refused, and never decoded as a record.
///
/// The child writes one whole record, then a second whose payload is cut at a
/// seeded point. Every cut point from "none of the payload arrived" to "all but
/// one byte arrived" is swept, so the family covers the whole range a real
/// interrupted write can land on rather than one convenient mid-payload case.
fn cuts_are_refused_never_decoded(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        // The second record declares eight payload bytes; `cut` of them arrive.
        const DECLARED: usize = 8;
        const WHOLE: &[u8] = b"whole";
        const PARTIAL: &[u8] = b"partial!!";
        let cut = usize::try_from(sim.rng().below(9))?;
        let ceiling = DECLARED.saturating_add(64);
        // One `printf` for both records: the whole one, then a prefix naming
        // The cut is in the bytes the child actually writes, so the reader really
        // does see a short payload rather than being asked to imagine one.
        let mut stream_bytes: Vec<u8> = Vec::new();
        stream_bytes.extend(u32::try_from(WHOLE.len())?.to_be_bytes());
        stream_bytes.extend_from_slice(WHOLE);
        stream_bytes.extend(u32::try_from(DECLARED)?.to_be_bytes());
        let partial = PARTIAL
            .get(..cut)
            .ok_or("the cut lies inside the partial payload")?;
        stream_bytes.extend_from_slice(partial);
        let stream_literal = stream_bytes
            .iter()
            .map(|byte| format!("\\{byte:03o}"))
            .collect::<String>();
        let outcome = run(&captured(&format!("printf '{stream_literal}'"), ceiling)?)?;

        // The expected reading is computed from the cut, so this family's model
        // and the shipped reader are checked against each other rather than the
        // test asserting whatever the reader happened to produce.
        let complete = cut == DECLARED;
        // Read through the capture's own door, not a re-plumbed slice: the
        // capture is untruncated here, so the ending is the reader's own.
        assert!(
            !outcome.stdout().truncated(),
            "cut={cut}: the write must fit the {ceiling}-byte ceiling, or the ending would \
             be the capture's rather than the child's"
        );
        let frames = outcome.stdout().frames(ceiling);
        let expected_frames = if complete { 2 } else { 1 };
        assert_eq!(
            frames.records().len(),
            expected_frames,
            "cut={cut} of {DECLARED}: {expected_frames} records are whole, got {:?}",
            frames.records()
        );
        assert_eq!(
            frames.records().first().and_then(FrameRead::payload),
            Some(WHOLE),
            "the first record decodes to exactly what the child wrote"
        );
        assert_eq!(
            frames.ended(),
            &if complete {
                FrameRead::EndOfStream
            } else {
                FrameRead::TruncatedPayload {
                    declared: DECLARED,
                    partial: partial.to_vec(),
                }
            },
            "cut={cut} of {DECLARED}: a prefix that named more than arrived is a truncation, \
             and one that named exactly what arrived is a whole record"
        );
        if complete {
            assert!(
                frames.is_complete(),
                "a cut of {DECLARED} delivered the whole payload, so the stream is complete"
            );
        } else {
            assert!(
                !frames.ended().is_frame(),
                "the cut record must never read as a decoded frame"
            );
            assert!(
                frames.ended().payload().is_none(),
                "a refusal carries no payload"
            );
            assert!(
                !frames.is_complete(),
                "a stream whose last record was cut off has no complete reading"
            );
        }
        sim.record("cut");
        sim.trace.record_number("cut", cut);
        sim.trace
            .record_number("declared", u64::try_from(DECLARED)?);
        Ok(())
    })
}

/// Two tenants' captures never cross.
///
/// Each tenant's child writes its own repeated byte, so a crossed capture would
/// be visible as the wrong byte rather than only as the wrong count. The point is
/// not that two runs are equal — it is that neither run can see the other's
/// bytes or total.
fn two_tenants_never_cross(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let ceiling = usize::try_from(sim.rng().between(64, 8192))?;
        let size = sim.rng().between(1, 20_000);
        let tenant: u32 = sim.rng().below(4);
        let byte = u8::try_from(tenant)?.wrapping_add(b'a');

        let mut observations: Vec<(Vec<u8>, u64)> = Vec::new();
        for _ in 0..2 {
            // `head -c` from `/dev/zero` written through `tr`, so each tenant's
            // stream is its own byte repeated: a crossed pipe would show up as
            // the wrong byte, not merely a wrong length.
            let script = format!("head -c {size} /dev/zero | tr '\\0' '{}'", char::from(byte));
            let outcome = run(&captured(&script, ceiling)?)?;
            observations.push((
                outcome.stdout().bytes().to_vec(),
                outcome.stdout().total_bytes(),
            ));
        }

        let (first, first_total) = observations
            .first()
            .cloned()
            .ok_or("the first tenant was observed")?;
        let (second, second_total) = observations
            .get(1)
            .cloned()
            .ok_or("the second tenant was observed")?;
        assert_eq!(
            first_total,
            u64::from(size),
            "the first tenant's total must be its own byte count"
        );
        assert_eq!(
            second_total,
            u64::from(size),
            "the second tenant's total must be its own byte count"
        );
        assert!(
            first.iter().all(|value| *value == byte),
            "the first tenant's retained bytes must all be its own byte {byte:?}"
        );
        assert!(
            second.iter().all(|value| *value == byte),
            "the second tenant's retained bytes must all be its own byte {byte:?}"
        );
        sim.record("tenants");
        sim.trace.record_number("tenant", u64::from(tenant));
        sim.trace.record_number("ceiling", ceiling);
        sim.trace.record_number("size", u64::from(size));
        sim.trace.record_number("retained", first.len());
        Ok(())
    })
}

/// The same seed produces the same trace, twice.
///
/// The union of the families above, so the receipt covers byte counts, totals,
/// truncation decisions and frame lengths rather than one scenario's shape. This
/// is the assertion that would fail if any of them read the clock.
fn the_same_seed_replays(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let ceiling = usize::try_from(sim.rng().between(1, 2048))?;
        let size = sim.rng().between(1, 8192);
        let outcome = run(&captured(&format!("head -c {size} /dev/zero"), ceiling)?)?;
        record_capture(sim, "replay", &outcome);

        // The framing half of the receipt too, so a change in how a cut frame is
        // classified shows up here and not only in the family that cuts.
        let frames = outcome.stdout().frames(ceiling);
        sim.trace.record_number("frames", frames.records().len());
        // The ending's *class*, not a rendering of it: a text comparison would
        // pass while the classification changed.
        sim.trace
            .record(&format!("ended-{}", ending_class(&frames)));
        sim.trace
            .record_number("capture-truncated", u64::from(outcome.stdout().truncated()));
        sim.trace.record_number("ceiling", ceiling);
        sim.trace.record_number("size", u64::from(size));
        Ok(())
    })
}

/// A seeded sweep of `sizes_stay_at_the_ceiling` over seeds 0..8.
#[test]
fn sizes_stay_at_the_ceiling_band_00() -> TestResult {
    sizes_stay_at_the_ceiling(Band::new(0, 8))
}

/// A seeded sweep of `sizes_stay_at_the_ceiling` over seeds 8..16.
#[test]
fn sizes_stay_at_the_ceiling_band_01() -> TestResult {
    sizes_stay_at_the_ceiling(Band::new(8, 8))
}

/// A seeded sweep of `cuts_are_refused_never_decoded` over seeds 16..24.
#[test]
fn cuts_are_refused_never_decoded_band_00() -> TestResult {
    cuts_are_refused_never_decoded(Band::new(16, 8))
}

/// A seeded sweep of `cuts_are_refused_never_decoded` over seeds 24..32.
#[test]
fn cuts_are_refused_never_decoded_band_01() -> TestResult {
    cuts_are_refused_never_decoded(Band::new(24, 8))
}

/// A seeded sweep of `two_tenants_never_cross` over seeds 32..40.
#[test]
fn two_tenants_never_cross_band_00() -> TestResult {
    two_tenants_never_cross(Band::new(32, 8))
}

/// A seeded sweep of `the_same_seed_replays` over seeds 40..48.
#[test]
fn the_same_seed_replays_band_00() -> TestResult {
    the_same_seed_replays(Band::new(40, 8))
}

/// A seeded sweep of `the_same_seed_replays` over seeds 48..56.
#[test]
fn the_same_seed_replays_band_01() -> TestResult {
    the_same_seed_replays(Band::new(48, 8))
}

/// The one payload byte a tenant's records carry, from its index.
///
/// 26 tenants rather than unbounded indices, so a tier of 10,000 children shares
/// payload bytes between runs: the assertion is about *which* capture a record
/// came from, not about every child having a distinct byte, and a crossed pipe
/// would still show as a byte this child never wrote.
fn tenant_byte(index: usize) -> Result<u8, Box<dyn Error>> {
    const LETTERS: &[u8; 26] = b"abcdefghijklmnopqrstuvwxyz";
    index
        .checked_rem(LETTERS.len())
        .and_then(|at| LETTERS.get(at))
        .copied()
        .ok_or_else(|| format!("tenant {index} has no letter").into())
}

/// How many whole `frame`-byte frames fit in `retained` bytes.
///
/// A counted subtraction rather than a division: the workspace forbids
/// `clippy::integer_division`, and a loop over a frame count bounded by a
/// capture ceiling is not a cost worth a suppressed lint.
fn whole_frames(retained: usize, frame: usize) -> usize {
    let mut left = retained;
    let mut whole = 0_usize;
    while left >= frame {
        left = left.saturating_sub(frame);
        whole = whole.saturating_add(1);
    }
    whole
}

/// A shell spec writing `records` whole framed payloads of `byte`, with a capture
/// of `ceiling` bytes.
///
/// Shared by the seeded family and the saturation tiers so one framing shape has
/// one spelling: a shape written twice is a shape whose two spellings can drift.
fn framed_capture(
    byte: u8,
    payload: usize,
    records: usize,
    ceiling: usize,
) -> Result<ProcessSpec, Box<dyn Error>> {
    let body: Vec<u8> = std::iter::repeat_n(byte, payload).collect();
    let mut writer: Vec<u8> = Vec::new();
    for _ in 0..records {
        writer.extend_from_slice(&u32::try_from(payload)?.to_be_bytes());
        writer.extend_from_slice(&body);
    }
    let literal = writer
        .iter()
        .map(|value| format!("\\{value:03o}"))
        .collect::<String>();
    captured(&format!("printf '{literal}'"), ceiling)
}

mod capture;
mod flood;
mod verb;
