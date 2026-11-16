//! `domain::sys::Process` through the verb traits: the state it reports, the
//! certainty of its refusals, its shared ceiling and its framed stdout door.

use crate::scratch::Scratch;

use std::num::NonZeroUsize;
use std::time::Duration;

use lgwks_bot::app_error::BotError;
use lgwks_bot::cap::{Auth, Cap};
use lgwks_bot::domain::sys::{DEFAULT_CAPTURE_LIMIT, DEFAULT_DEADLINE, Process};
use lgwks_bot::ecs::EcsError;
use lgwks_bot::error::{DispatchCertainty, KernelError};
use lgwks_bot::gate::GrantSet;
use lgwks_bot::rt::process::{FrameRead, ProcessSpec, StdioPolicy};
use lgwks_bot::verb::{Execute, Observe, Query};

use super::{TestResult, shell};

/// A `domain::sys::Process` for `script`, at the documented defaults.
fn process_for(script: &str) -> Process {
    let mut spec = shell(script);
    spec.capture_stdout(DEFAULT_CAPTURE_LIMIT);
    spec.capture_stderr(DEFAULT_CAPTURE_LIMIT);
    spec.deadline(DEFAULT_DEADLINE);
    Process::from_spec(spec)
}

/// An `Auth` covering `bot.sys`.
fn sys_auth() -> Result<Auth, KernelError> {
    GrantSet::empty().grant(Cap::sys()).issue(&[Cap::sys()])
}

/// An `Auth` covering nothing, for the missing-capability case.
fn empty_auth() -> Result<Auth, KernelError> {
    GrantSet::empty().issue(&[])
}

// ── domain::sys::Process ────────────────────────────────────────────────────

#[test]
fn execute_reports_the_exit_code_and_the_captured_streams() -> TestResult {
    let process = process_for("printf out; printf err >&2; exit 3");
    let state = lgwks_bot::rt::runtime::block_on(async {
        process.execute_action((sys_auth()?, &())).await
    })?;
    assert_eq!(
        state.exit_code,
        Some(3),
        "the state carries the code; judging it is the caller's job (T35)"
    );
    assert!(!state.running, "a returned one-shot run is not running");
    assert_eq!(state.stdout(), "out", "stdout is reported");
    assert_eq!(state.stderr(), "err", "stderr is reported");
    assert!(!state.stdout_truncated && !state.stderr_truncated);
    assert_eq!(state.stdout_total_bytes, 3);
    assert_eq!(state.stderr_total_bytes, 3);
    assert!(!state.deadline_fired);
    Ok(())
}

#[test]
fn exit_zero_is_reported_as_zero_not_success() -> TestResult {
    let process = process_for("exit 0");
    let state = lgwks_bot::rt::runtime::block_on(async {
        process.execute_action((sys_auth()?, &())).await
    })?;
    assert_eq!(
        state.exit_code,
        Some(0),
        "exit zero is an exit code, not a success verdict (T35)"
    );
    Ok(())
}

#[test]
fn a_signal_death_is_reported_in_the_state() -> TestResult {
    let process = process_for("kill -TERM $$");
    let state = lgwks_bot::rt::runtime::block_on(async {
        process.execute_action((sys_auth()?, &())).await
    })?;
    assert_eq!(state.exit_code, None, "a signal has no exit code");
    assert_eq!(state.signal, Some(15), "the signal is reported");
    Ok(())
}

#[test]
fn a_deadline_is_reported_in_the_state() -> TestResult {
    let mut spec = shell("sleep 60");
    spec.capture_stdout(DEFAULT_CAPTURE_LIMIT);
    spec.capture_stderr(DEFAULT_CAPTURE_LIMIT);
    spec.deadline(Duration::from_millis(200));
    let process = Process::from_spec(spec);
    let state = lgwks_bot::rt::runtime::block_on(async {
        process.execute_action((sys_auth()?, &())).await
    })?;
    assert!(state.deadline_fired, "the deadline stop is reported");
    assert_eq!(
        state.exit_code, None,
        "the deadline stopped the process; it did not exit"
    );
    Ok(())
}

#[test]
fn the_observed_and_query_forms_run_the_command() -> TestResult {
    let process = process_for("exit 0");
    let (observed, queried) = lgwks_bot::rt::runtime::block_on(async {
        let observed = process.poll((sys_auth()?, ())).await?;
        let queried = process.query((sys_auth()?, &())).await?;
        Ok::<_, EcsError>((observed, queried))
    })?;
    assert_eq!(
        observed.exit_code,
        Some(0),
        "poll is not a silent no-op: it runs and reports"
    );
    assert_eq!(
        queried.exit_code,
        Some(0),
        "query is not a silent no-op: it runs and reports"
    );
    Ok(())
}

#[test]
fn a_missing_program_is_a_refusal_with_nothing_run() -> TestResult {
    let process = Process::from_spec(ProcessSpec::new("/nonexistent/lgwks-bot-probe"));
    let error = lgwks_bot::rt::runtime::block_on(async {
        process.execute_action((sys_auth()?, &())).await
    })
    .err()
    .ok_or_else(|| std::io::Error::other("a missing program must be refused"))?;
    let shown = error.to_string();
    assert_eq!(
        BotError::from(error).dispatch_certainty(),
        DispatchCertainty::Refused,
        "a program that never started establishes that nothing ran: {shown}"
    );
    Ok(())
}

#[test]
fn a_missing_capability_is_refused_before_any_spawn() -> TestResult {
    let dir = Scratch::new("cap")?;
    let marker = dir.path().join("marker");
    let process = process_for(&format!("touch {}", marker.display()));
    let error = lgwks_bot::rt::runtime::block_on(async {
        process.execute_action((empty_auth()?, &())).await
    })
    .err()
    .ok_or_else(|| std::io::Error::other("the capability check must deny"))?;
    assert!(
        matches!(
            error,
            EcsError::Kernel(KernelError::CapabilityDenied { .. })
        ),
        "a call without bot.sys must be refused as a capability deficit: {error}"
    );
    // The verb runs its child to completion before it returns, so a spawn that
    // preceded the check would already have left its marker: no wait is needed.
    assert!(
        !marker.exists(),
        "the cap check must precede any spawn, so the command never ran"
    );
    Ok(())
}

/// The default constructor bounds its streams and its runtime even for a child
/// that would not bound itself.
#[test]
fn the_default_constructor_runs_a_real_child() -> TestResult {
    let process = Process::new("true");
    let state = lgwks_bot::rt::runtime::block_on(async {
        process.execute_action((sys_auth()?, &())).await
    })?;
    assert_eq!(
        state.exit_code,
        Some(0),
        "the default constructor runs a real child"
    );
    assert!(
        !state.deadline_fired,
        "an ordinary child finishes before the default deadline"
    );
    Ok(())
}

/// Concurrent verb calls on one `Process` share its ceiling: a burst of calls
/// waits for slots instead of forking a burst of children. Each child counts
/// the live children (its own marker included) when it starts, so the highest
/// count any child printed is a lower bound on the real concurrency and can
/// only exceed the ceiling if the ceiling was not enforced.
#[test]
fn concurrent_calls_on_one_process_share_its_ceiling() -> TestResult {
    const CALLS: usize = 64;
    const CEILING: usize = 4;
    let live = Scratch::new("ceiling-live")?;
    let done = Scratch::new("ceiling-done")?;
    let live_dir = live.path().join("");
    let done_dir = done.path().join("");
    let script = format!(
        "touch {live}/$$; ls {live} | wc -l; sleep 0.2; mv {live}/$$ {done}/",
        live = live_dir.display(),
        done = done_dir.display(),
    );
    let process = process_for(&script)
        .max_concurrent(NonZeroUsize::new(CEILING).ok_or("the ceiling admits at least one call")?);
    let auth = sys_auth()?;
    let states = lgwks_bot::rt::runtime::block_on(lgwks_std::task::join_all(
        (0..CALLS).map(|_| process.execute_action((auth.clone(), &()))),
    ));
    let mut highest = 0_usize;
    for state in states {
        let state = state?;
        assert_eq!(
            state.exit_code,
            Some(0),
            "every call runs its child to a zero exit"
        );
        highest = highest.max(state.stdout().trim().parse::<usize>()?);
    }
    assert!(
        highest <= CEILING,
        "a child saw {highest} live children against a ceiling of {CEILING}"
    );
    assert!(
        highest >= 2,
        "the calls must overlap up to the ceiling, not run one at a time"
    );
    Ok(())
}

/// The capture builders set the data policy the runner reads, and the ceiling
/// is non-zero by its type.
#[test]
fn capture_builders_set_the_declared_policy() -> TestResult {
    let limit = NonZeroUsize::new(1024).ok_or("a capture limit of 1024 is not zero")?;
    let mut spec = ProcessSpec::new("true");
    spec.capture_stdout(limit);
    spec.capture_stderr(limit);
    assert_eq!(
        spec.stdout_policy(),
        StdioPolicy::Capture(limit),
        "capture_stdout must set the Capture policy"
    );
    assert_eq!(
        spec.stderr_policy(),
        StdioPolicy::Capture(limit),
        "capture_stderr must set the Capture policy"
    );
    Ok(())
}

// ── domain::sys::Process, the production door to the frame grammar ──────────

/// A `Process` for `script` whose verbs report a framed stdout reading.
///
/// Built on `Process::from_spec` plus `frame_stdout`, the two steps a caller
/// takes, so the framed reading is reached through the verb rather than only by
/// a test that re-plumbs a captured slice.
fn framing_process(
    script: &str,
    capture: NonZeroUsize,
) -> Result<Process, Box<dyn std::error::Error>> {
    let ceiling = NonZeroUsize::new(4096).ok_or("a frame ceiling of at least one")?;
    let mut spec = shell(script);
    spec.capture_stdout(capture);
    spec.capture_stderr(capture);
    spec.deadline(DEFAULT_DEADLINE);
    Ok(Process::from_spec(spec).frame_stdout(ceiling))
}

/// T05, wired: the frames of a child's output are read through the verb a
/// caller performs work with, not only through a reader the test plumbs.
///
/// The child writes two whole records and then a third whose payload stops
/// half way. The `ProcessState` the `Execute` verb returns must carry exactly
/// the two whole records and end in the typed truncation, which is what makes
/// `CapturedStream::frames` a production path rather than a capability with no
/// caller.
#[test]
fn the_execute_verb_reports_stdout_frames_two_records_and_a_cut_third() -> TestResult {
    let script = "printf '\\000\\000\\000\\004done\\000\\000\\000\\002ok\\000\\000\\000\\010part'";
    let process = framing_process(script, DEFAULT_CAPTURE_LIMIT)?;
    let state = lgwks_bot::rt::runtime::block_on(async {
        process.execute_action((sys_auth()?, &())).await
    })?;
    assert_eq!(
        state.exit_code,
        Some(0),
        "the child completed its own work; the framing is what is under test"
    );
    assert!(
        !state.stdout_truncated,
        "the write fits the capture ceiling, so the ending is the child's own"
    );
    let frames = state.stdout_frames().ok_or_else(|| {
        std::io::Error::other("a domain built with frame_stdout must report frames")
    })?;
    assert_eq!(
        frames.records().len(),
        2,
        "only the records that arrived whole are frames: {:?}",
        frames.records()
    );
    let payloads: Vec<&[u8]> = frames
        .records()
        .iter()
        .filter_map(FrameRead::payload)
        .collect();
    assert_eq!(
        payloads,
        vec![&b"done"[..], &b"ok"[..]],
        "both whole records decode to exactly what the child wrote"
    );
    assert_eq!(
        frames.ended(),
        &FrameRead::TruncatedPayload {
            declared: 8,
            partial: b"part".to_vec(),
        },
        "a prefix naming eight bytes that delivered four is a truncation"
    );
    assert!(
        !frames.is_complete(),
        "a stream whose last record was cut off has no complete reading"
    );
    Ok(())
}

/// D2 wired: a capture that cut the child's output ends at the capture's own
/// ceiling, and the domain reports it that way rather than as the child's
/// truncation or a clean end.
#[test]
fn the_execute_verb_reports_the_capture_ceiling_when_the_child_overruns_it() -> TestResult {
    const CAPTURE: usize = 8;
    let script = "printf '\\000\\000\\000\\002ok\\000\\000\\000\\002ok\\000\\000\\000\\002ok'";
    let process = framing_process(script, NonZeroUsize::new(CAPTURE).ok_or("a ceiling")?)?;
    let state = lgwks_bot::rt::runtime::block_on(async {
        process.execute_action((sys_auth()?, &())).await
    })?;
    assert!(
        state.stdout_truncated,
        "ten bytes into an {CAPTURE}-byte capture must report that the capture cut the stream"
    );
    let frames = state.stdout_frames().ok_or_else(|| {
        std::io::Error::other("a domain built with frame_stdout must report frames")
    })?;
    assert_eq!(
        frames.ended(),
        &FrameRead::CeilingReached { ceiling: CAPTURE },
        "the retained bytes are a prefix the capture cut, so the ending is the capture's \
         retained capacity, never a child truncation"
    );
    assert!(
        !frames.is_complete(),
        "a reading of a capture-cut prefix is never a complete reading of the child"
    );
    Ok(())
}

/// A domain built without `frame_stdout` reports no frames and an unchanged
/// lossy stdout view, so attaching the reading is opt-in and changes nothing
/// else about the run.
#[test]
fn a_domain_without_frame_stdout_reports_no_frames_and_unchanged_stdout() -> TestResult {
    let process = process_for("printf out");
    let state = lgwks_bot::rt::runtime::block_on(async {
        process.execute_action((sys_auth()?, &())).await
    })?;
    assert!(
        state.stdout_frames().is_none(),
        "a domain with no frame_stdout must not invent a framed reading"
    );
    assert_eq!(
        state.stdout(),
        "out",
        "the lossy stdout view is unchanged by the absence of a framed reading"
    );
    Ok(())
}

/// The framed reading is byte-exact where the lossy view cannot be.
///
/// The child writes one record whose payload is two bytes that are not valid
/// UTF-8. `stdout_frames()` returns them exactly, while `stdout()` replaces
/// them with U+FFFD, so the two readings of the same child output differ by
/// construction — the discriminating case for why the framed door exists.
#[test]
fn a_binary_record_round_trips_through_frames_while_stdout_is_lossy() -> TestResult {
    let script = "printf '\\000\\000\\000\\002\\377\\376'";
    let process = framing_process(script, DEFAULT_CAPTURE_LIMIT)?;
    let state = lgwks_bot::rt::runtime::block_on(async {
        process.execute_action((sys_auth()?, &())).await
    })?;
    let frames = state.stdout_frames().ok_or_else(|| {
        std::io::Error::other("a domain built with frame_stdout must report frames")
    })?;
    assert_eq!(frames.records().len(), 1, "one whole record arrived");
    assert_eq!(
        frames.records()[0].payload(),
        Some(&[0xff_u8, 0xfe_u8][..]),
        "the framed reading round-trips the binary payload byte for byte"
    );
    assert_eq!(
        frames.ended(),
        &FrameRead::EndOfStream,
        "the whole record ended cleanly"
    );
    assert_ne!(
        state.stdout().as_bytes(),
        &[0xff_u8, 0xfe_u8][..],
        "the lossy view cannot round-trip bytes that are not UTF-8"
    );
    assert!(
        state.stdout().contains('\u{FFFD}'),
        "the lossy view replaces the invalid bytes: {:?}",
        state.stdout()
    );
    Ok(())
}
