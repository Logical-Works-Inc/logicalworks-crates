//! The fools that attack from outside: liar-upstream answers with a kind or a
//! cause that lies about what it means, pulled-plug kills a run between two
//! effects and again during recovery, and clock-fool steps the clock back and
//! forward. The failure must reach the caller typed, with its class from its
//! variant and its readings intact.

use std::io;
use std::time::Duration;

use lgwks_bot::app_error::BotError;
use lgwks_bot::ecs::EcsError;
use lgwks_bot::error::{DispatchCertainty, KernelError, RetryClass};
use lgwks_bot::script::FlowError;
use lgwks_bot::session::{JournalError, SessionError};
use lgwks_bot::task::StoreError;

use super::{DRAWS, Fields, Hash, Kind, Route, Spec, TestResult, hostile, path, reading, refuse};
use super::{replays, walk};
use crate::sim::{Rng, Trace};

/// The io kinds the flow layer reads as a moment (`true`) or as a fact.
const IO_KINDS: [(io::ErrorKind, bool); 16] = [
    (io::ErrorKind::TimedOut, true),
    (io::ErrorKind::Interrupted, true),
    (io::ErrorKind::WouldBlock, true),
    (io::ErrorKind::ConnectionReset, true),
    (io::ErrorKind::ConnectionAborted, true),
    (io::ErrorKind::ConnectionRefused, true),
    (io::ErrorKind::NotConnected, true),
    (io::ErrorKind::BrokenPipe, true),
    (io::ErrorKind::UnexpectedEof, true),
    (io::ErrorKind::NotFound, false),
    (io::ErrorKind::PermissionDenied, false),
    (io::ErrorKind::InvalidData, false),
    (io::ErrorKind::AlreadyExists, false),
    (io::ErrorKind::InvalidInput, false),
    (io::ErrorKind::Other, false),
    (io::ErrorKind::WriteZero, false),
];

/// An upstream that fails with a drawn kind: the flow reads it by the table, and
/// a store that fails the same way stays a typed store refusal with its kind.
pub(super) fn sim_liar_upstream_io_and_store(seed: u64) -> Hash {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    for draw in 0..DRAWS {
        let row = IO_KINDS
            .get(usize::try_from(rng.below(16))?)
            .ok_or("an io kind")?;
        let (kind, moment) = (row.0, row.1);
        let at = format!("seed {seed:#x} draw {draw} {kind:?}");
        let read = BotError::from(FlowError::from(io::Error::new(kind, hostile(&mut rng))));
        let (name, certainty, class) = if moment {
            (
                "Script(Transient",
                DispatchCertainty::NotDelivered,
                RetryClass::Safe,
            )
        } else {
            (
                "Script(Failed",
                DispatchCertainty::Refused,
                RetryClass::Never,
            )
        };
        assert!(
            format!("{read:?}").starts_with(name),
            "{at}: read as {read:?}"
        );
        assert_eq!(read.dispatch_certainty(), certainty, "{at}");
        assert_eq!(read.retry_class(), class, "{at}");
        let device = StoreError::Storage {
            cause: io::Error::new(kind, hostile(&mut rng)),
        };
        let stored = BotError::from(FlowError::from(device));
        let debug = format!("{stored:?}");
        assert!(
            debug.starts_with("Script(Store"),
            "{at}: a disk fault is a store fault"
        );
        assert_eq!(
            stored.retry_class(),
            RetryClass::Never,
            "{at}: no plain retry of a disk"
        );
        assert_eq!(walk(&stored), ["store", "io"], "{at}");
        let kept = std::error::Error::source(&stored)
            .and_then(std::error::Error::source)
            .and_then(|link| link.downcast_ref::<io::Error>())
            .map(io::Error::kind);
        assert_eq!(kept, Some(kind), "{at}: the kind was lost");
        trace.record(&format!("{kind:?}"));
    }
    Ok(trace.hash())
}

#[test]
/// A failing upstream or disk reaches the caller typed, with its kind and its class.
fn sim_error_layers_liar_upstream_io_and_store_failures_stay_typed() -> TestResult {
    replays(sim_liar_upstream_io_and_store)
}

/// Causes that lie about their own retry class: the class is the variant's.
pub(super) fn sim_liar_upstream_cause_text(seed: u64) -> Hash {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    for draw in 0..DRAWS {
        let mut spec = Spec::draw(&mut rng);
        spec.fields.cause = match rng.below(6) {
            0 => String::from("timed out, retry me"),
            1 => String::from("PERMANENT: never retry"),
            2 => String::from("RequiresEvidence"),
            3 => String::from("200 OK"),
            4 => String::from("Safe"),
            _ => hostile(&mut rng),
        };
        let error = spec.raise();
        spec.verify(&error, seed, draw);
        let plain = error.retry_class() == RetryClass::Safe;
        assert_eq!(
            plain,
            spec.kind == Kind::NotDelivered,
            "seed {seed:#x} draw {draw}: only a refusal that never left is a plain retry"
        );
        trace.record(&spec.fields.cause);
    }
    Ok(trace.hash())
}

#[test]
/// The words in a cause never move a failure to another retry class.
fn sim_error_layers_liar_upstream_cause_text_never_decides_the_retry_class() -> TestResult {
    replays(sim_liar_upstream_cause_text)
}

/// One step's receipt: the journal is killed at the step the plug names.
fn record(step: usize, plug: usize, node: &str) -> Result<(), Box<SessionError>> {
    if step == plug {
        let refusal = Err(Box::new(SessionError::ReceiptNotRecorded {
            node: node.to_owned(),
            cause: JournalError::new("run-journal", "the plug was pulled"),
        }));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "record: the plug was pulled");
        return refusal;
    }
    Ok(())
}

/// Recovery reads the run store; it dies on the torn frame when `dies`.
fn recover(torn: u64, dies: bool) -> Result<(), Box<FlowError>> {
    if dies {
        let refusal = Err(Box::new(FlowError::from(StoreError::Corrupt { at: torn })));
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "recover: the store died on the torn frame");
        return refusal;
    }
    Ok(())
}

/// Carry a layer failure up to a boxed app-layer failure by `From`.
fn lift<E: Into<BotError>>(error: E) -> Box<BotError> {
    Box::new(error.into())
}

/// Run the steps from `from`, counting each that completed in `ran`.
fn run(steps: &[String], from: usize, plug: usize, ran: &mut [u32]) -> Result<(), Box<BotError>> {
    for (index, node) in steps.iter().enumerate().skip(from) {
        record(index, plug, node).map_err(|refused| lift(*refused))?;
        if let Some(count) = ran.get_mut(index) {
            *count = count.saturating_add(1);
        }
    }
    Ok(())
}

/// Run, and when killed, restart: recovery may itself die.
fn restart(
    steps: &[String],
    plug: usize,
    dies: bool,
    ran: &mut [u32],
) -> Result<(), Box<BotError>> {
    if run(steps, 0, plug, ran).is_err() {
        let done = ran.iter().take_while(|count| **count > 0);
        let torn = done.fold(0_u64, |frames, _| frames.saturating_add(1));
        let cursor = ran.iter().take_while(|count| **count > 0).count();
        recover(torn, dies).map_err(|refused| lift(*refused))?;
        run(steps, cursor, usize::MAX, ran)?;
    }
    Ok(())
}

/// A run killed at a seeded step: the kill reaches the caller typed, no step is
/// lost or done twice, and a death in recovery is a typed store failure.
pub(super) fn sim_pulled_plug(seed: u64) -> Hash {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    for draw in 0..DRAWS {
        let count = usize::try_from(rng.between(1, 48))?;
        let steps: Vec<String> = (0..count).map(|_| hostile(&mut rng)).collect();
        let plug = usize::try_from(rng.between(0, 52))?;
        let dies = rng.chance(500);
        let at = format!("seed {seed:#x} draw {draw} plug {plug} of {count}");
        let mut alone = vec![0_u32; count];
        let killed = run(&steps, 0, plug, &mut alone).map_err(|boxed| *boxed);
        if plug < count {
            let node = steps.get(plug).ok_or("the plugged step")?;
            match killed {
                Err(BotError::Session(SessionError::ReceiptNotRecorded {
                    node: named, ..
                })) => {
                    assert_eq!(&named, node, "{at}: the kill names its step");
                }
                other => return refuse(format!("{at}: the kill came back as {other:?}")),
            }
            assert!(
                alone.iter().take(plug).all(|ran| *ran == 1),
                "{at}: steps before ran once"
            );
            assert!(
                alone.iter().skip(plug).all(|ran| *ran == 0),
                "{at}: nothing ran past it"
            );
        } else {
            assert!(killed.is_ok(), "{at}: no plug, no failure");
        }
        let mut ran = vec![0_u32; count];
        let outcome = restart(&steps, plug, dies, &mut ran).map_err(|boxed| *boxed);
        match (plug < count && dies, outcome) {
            (false, Ok(())) => {
                assert!(
                    ran.iter().all(|once| *once == 1),
                    "{at}: a step was lost or doubled"
                );
            }
            (true, Err(BotError::Script(FlowError::Store { source, .. }))) => {
                match *source {
                    StoreError::Corrupt { at: torn } => {
                        assert_eq!(u64::try_from(plug), Ok(torn), "{at}: the torn frame");
                    }
                    ref other => return refuse(format!("{at}: the store said {other:?}")),
                }
                assert!(
                    ran.iter().take(plug).all(|once| *once == 1),
                    "{at}: steps before ran once"
                );
                assert!(
                    ran.iter().skip(plug).all(|none| *none == 0),
                    "{at}: nothing ran past it"
                );
            }
            (_, other) => return refuse(format!("{at}: restart came back as {other:?}")),
        }
        trace.record_number("plug", plug);
        trace.record_number("dies", u8::from(dies));
    }
    Ok(trace.hash())
}

#[test]
/// A run killed between two effects reaches the caller typed, and a restart loses nothing.
fn sim_error_layers_pulled_plug_a_killed_run_reaches_the_caller_typed() -> TestResult {
    replays(sim_pulled_plug)
}

/// The clock readings a kernel failure carries.
fn kernel_readings(error: &KernelError) -> Vec<Duration> {
    match *error {
        KernelError::CredentialExpired {
            expired_at, now, ..
        } => vec![expired_at, now],
        _ => Vec::new(),
    }
}

/// The clock readings a world failure carries.
fn world_readings(error: &EcsError) -> Vec<Duration> {
    match *error {
        EcsError::Kernel(ref kernel) => kernel_readings(kernel),
        EcsError::PollStalled { deadline, .. } | EcsError::PollDeadlineUnbounded { deadline } => {
            vec![deadline]
        }
        EcsError::PollDeadlineExceeded { deadline, ceiling } => vec![deadline, ceiling],
        _ => Vec::new(),
    }
}

/// The clock readings any arm of `error` carries, in field order.
fn readings(error: &BotError) -> Vec<Duration> {
    match *error {
        BotError::Kernel(ref kernel) | BotError::Session(SessionError::Kernel(ref kernel)) => {
            kernel_readings(kernel)
        }
        BotError::World(ref world) => world_readings(world),
        BotError::Script(FlowError::Bot { ref source, .. }) => world_readings(source),
        BotError::Script(FlowError::TimedOut { after, .. }) => vec![after],
        _ => Vec::new(),
    }
}

/// The clock steps back, jumps a year, stalls at zero: every reading survives.
pub(super) fn sim_clock_fool(seed: u64) -> Hash {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    for draw in 0..DRAWS {
        let (near, far) = (reading(&mut rng), reading(&mut rng));
        let fields = Fields {
            near,
            far,
            ..Fields::draw(&mut rng)
        };
        let route = Route::draw(&mut rng);
        let expired = Spec {
            kind: Kind::Expired,
            route,
            fields,
        };
        let stalled = EcsError::PollStalled {
            chain: 3,
            deadline: near,
        };
        let exceeded = EcsError::PollDeadlineExceeded {
            deadline: near,
            ceiling: far,
        };
        let timed_out = FlowError::TimedOut {
            at: path("within/pay"),
            after: far,
        };
        let cases = [
            (expired.raise(), vec![near, far]),
            (BotError::from(stalled), vec![near]),
            (
                BotError::from(FlowError::from(EcsError::PollStalled {
                    chain: 0,
                    deadline: far,
                })),
                vec![far],
            ),
            (BotError::from(exceeded), vec![near, far]),
            (BotError::from(timed_out), vec![far]),
        ];
        for case in &cases {
            let at = format!("seed {seed:#x} draw {draw}");
            assert_eq!(readings(&case.0), case.1, "{at}: a reading moved");
            let line = case.0.to_string();
            for held in &case.1 {
                let spelled = format!("{held:?}");
                assert!(
                    line.contains(&spelled),
                    "{at}: {spelled} missing from {line}"
                );
            }
        }
        trace.record(&format!("{near:?}/{far:?}"));
    }
    Ok(trace.hash())
}

#[test]
/// A deadline or credential error keeps the readings it was judged at, through every hop.
fn sim_error_layers_clock_fool_every_clock_reading_survives_every_hop() -> TestResult {
    replays(sim_clock_fool)
}
