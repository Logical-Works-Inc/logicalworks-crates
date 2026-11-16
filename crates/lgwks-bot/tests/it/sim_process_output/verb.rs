//! The sys-verb families: a seeded framed stream read back through `Process`'s `Execute` verb, two tenants on one host, and the declared tiers.

use super::*;
use lgwks_bot::cap::{Auth, Cap};
use lgwks_bot::domain::sys::{Process, ProcessState};
use lgwks_bot::gate::GrantSet;
use lgwks_bot::verb::Execute;

// ── The sys domain's verbs: the production door to the frame grammar ────────

/// The `Auth` a `sys::Process` verb call needs.
fn sys_auth() -> Result<Auth, Box<dyn Error>> {
    Ok(GrantSet::empty().grant(Cap::sys()).issue(&[Cap::sys()])?)
}

/// A `Process` for `spec` whose verbs report a framed stdout reading.
fn process_with_frames(spec: ProcessSpec) -> Result<Process, Box<dyn Error>> {
    let ceiling =
        NonZeroUsize::new(DEFAULT_FRAME_CEILING).ok_or("a frame ceiling of at least one")?;
    Ok(Process::from_spec(spec).frame_stdout(ceiling))
}

/// Run `process` once through the `Execute` verb on a fresh runtime.
fn execute(process: &Process) -> Result<ProcessState, Box<dyn Error>> {
    let auth = sys_auth()?;
    let runtime = lgwks_bot::rt::runtime::Runtime::new()?;
    Ok(runtime.block_on(process.execute_action((auth, &())))?)
}

/// A seeded framed stream read back through the verb a caller calls.
///
/// The capability this pins is a caller's: a `Process` built with
/// `frame_stdout` reports the child's stdout as framed records on the state its
/// verb returns, so `CapturedStream::frames` runs on the real run path rather
/// than only inside a test. The seed draws the payload size, the record count
/// and the payload byte; the assertion checks the reading against a model built
/// from those draws, so the shipped reader and the model are compared rather
/// than the test asserting whatever the reader produced.
fn verb_framed_reads_agree_with_the_model(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let payload = usize::try_from(sim.rng().between(1, 32))?;
        let records = usize::try_from(sim.rng().between(1, 8))?;
        let byte = u8::try_from(sim.rng().below(26))?.wrapping_add(b'a');
        // A capture ceiling generous enough that the child's whole output is
        // retained, so the framed ending is the child's own and not the
        // capture's cut.
        let process = process_with_frames(framed_capture(byte, payload, records, 4096)?)?;
        let state = execute(&process)?;
        assert_eq!(state.exit_code, Some(0), "the child ran to a zero exit");
        assert!(
            !state.stdout_truncated,
            "the whole framed stream fits the capture ceiling"
        );
        let frames = state
            .stdout_frames()
            .ok_or("a domain built with frame_stdout must report frames")?;
        assert_eq!(
            frames.records().len(),
            records,
            "the verb must report exactly the {records} whole records the child wrote"
        );
        let expected: Vec<u8> = std::iter::repeat_n(byte, payload).collect();
        for (index, record) in frames.records().iter().enumerate() {
            assert_eq!(
                record.payload(),
                Some(expected.as_slice()),
                "record {index} must decode to this tenant's own payload byte"
            );
        }
        assert_eq!(
            frames.ended(),
            &FrameRead::EndOfStream,
            "a stream that ended between records is a complete read"
        );
        assert!(frames.is_complete(), "every record was decoded whole");
        sim.record("verb-framed");
        sim.trace.record_number("payload", payload);
        sim.trace.record_number("records", records);
        sim.trace.record_number("frames", frames.records().len());
        Ok(())
    })
}

/// Two tenants' framed readings on one host never cross.
///
/// Two `Process` values run their own children on one runtime, each built with
/// its own payload byte. A crossed capture or a shared reader would show up as
/// the wrong bytes rather than only as a wrong count, so the reading is checked
/// against each tenant's own byte.
fn verb_two_tenants_never_cross(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let payload = usize::try_from(sim.rng().between(1, 16))?;
        let records = usize::try_from(sim.rng().between(1, 5))?;
        let runtime = lgwks_bot::rt::runtime::Runtime::new()?;
        let auth = sys_auth()?;
        let first = process_with_frames(framed_capture(b'a', payload, records, 4096)?)?;
        let second = process_with_frames(framed_capture(b'b', payload, records, 4096)?)?;
        let (first_state, second_state) = runtime.block_on(async {
            let first_state = first.execute_action((auth.clone(), &())).await?;
            let second_state = second.execute_action((auth, &())).await?;
            Ok::<_, lgwks_bot::app_error::BotError>((first_state, second_state))
        })?;

        for (label, state, byte) in [
            ("first", &first_state, b'a'),
            ("second", &second_state, b'b'),
        ] {
            assert_eq!(
                state.exit_code,
                Some(0),
                "{label} tenant ran to a zero exit"
            );
            let frames = state
                .stdout_frames()
                .ok_or("a domain built with frame_stdout must report frames")?;
            assert_eq!(
                frames.records().len(),
                records,
                "{label} tenant: exactly its own {records} records"
            );
            let expected: Vec<u8> = std::iter::repeat_n(byte, payload).collect();
            for record in frames.records() {
                assert_eq!(
                    record.payload(),
                    Some(expected.as_slice()),
                    "{label} tenant must never see the other tenant's payload"
                );
            }
        }
        assert_eq!(
            first_state.stdout_total_bytes, second_state.stdout_total_bytes,
            "both tenants wrote the same shape, so isolation is about content not count"
        );
        sim.record("verb-tenants");
        sim.trace.record_number("payload", payload);
        sim.trace.record_number("records", records);
        sim.trace
            .record_number("first-total", first_state.stdout_total_bytes);
        sim.trace
            .record_number("second-total", second_state.stdout_total_bytes);
        Ok(())
    })
}

/// A seeded sweep of the verb's framed reading over seeds 120..128.
#[test]
fn verb_framed_reads_agree_with_the_model_band_00() -> TestResult {
    verb_framed_reads_agree_with_the_model(Band::new(120, 8))
}

/// A seeded sweep of the verb's framed reading over seeds 128..136.
#[test]
fn verb_framed_reads_agree_with_the_model_band_01() -> TestResult {
    verb_framed_reads_agree_with_the_model(Band::new(128, 8))
}

/// A seeded sweep of two tenants' verb readings over seeds 136..144.
#[test]
fn verb_two_tenants_never_cross_band_00() -> TestResult {
    verb_two_tenants_never_cross(Band::new(136, 8))
}

/// A seeded sweep of two tenants' verb readings over seeds 144..152.
#[test]
fn verb_two_tenants_never_cross_band_01() -> TestResult {
    verb_two_tenants_never_cross(Band::new(144, 8))
}

/// The verb path holds at the declared concurrency tiers.
///
/// Each tier drives `min(requested, TIER_CEILING)` concurrent `Process` values,
/// each running its own framed child through `execute_action`. The requested,
/// reached and ceiling levels are all reported (the INV-BOT-16 rule), so a tier
/// this host cannot reach says so rather than passing quietly: ten thousand real
/// `sh` children is a fork storm, and the honest claim is the tier reached.
#[test]
fn verb_framed_reads_saturate_at_the_declared_tiers() -> TestResult {
    const PAYLOAD: usize = 8;
    const RECORDS: usize = 4;
    // The tier this host is asked to drive. Every child is a real process on a
    // real pipe, so the ceiling is what the OS will bear rather than an
    // arbitrary number; a reader that shared state across the verb calls would
    // fail here at any tier, which is what makes the reached level sufficient.
    const TIER_CEILING: usize = 256;

    for requested in [100_usize, 1_000, 10_000] {
        let level = requested.min(TIER_CEILING);
        let runtime = lgwks_bot::rt::runtime::Runtime::new()?;
        let auth = sys_auth()?;
        let processes: Vec<Process> = (0..level)
            .map(|index| -> Result<Process, Box<dyn Error>> {
                process_with_frames(framed_capture(tenant_byte(index)?, PAYLOAD, RECORDS, 4096)?)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let observations = runtime.block_on(async {
            let runs: Vec<_> = processes
                .iter()
                .map(|process| process.execute_action((auth.clone(), &())))
                .collect();
            lgwks_std::task::join_all(runs).await
        });

        assert_eq!(
            observations.len(),
            level,
            "requested={requested} reached={level} ceiling={TIER_CEILING}: every concurrent verb \
             call reported, so the tier really ran"
        );
        for (index, outcome) in observations.into_iter().enumerate() {
            let state = outcome?;
            let frames = state
                .stdout_frames()
                .ok_or("a domain built with frame_stdout must report frames")?;
            assert_eq!(
                frames.records().len(),
                RECORDS,
                "requested={requested} child {index}: each call keeps its own {RECORDS} records"
            );
            let expected: Vec<u8> = std::iter::repeat_n(tenant_byte(index)?, PAYLOAD).collect();
            for record in frames.records() {
                assert_eq!(
                    record.payload(),
                    Some(expected.as_slice()),
                    "requested={requested} child {index}: record must be this call's own payload, \
                     never another's"
                );
            }
        }
    }
    Ok(())
}
