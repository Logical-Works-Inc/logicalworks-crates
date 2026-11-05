//! Fail-closed start over the durable run-store sink (issue #820).
//!
//! The kernel's `canvas-backend/tests/startup.rs` pins the fail-closed serving
//! contract of a degraded router: liveness stays up, readiness reports
//! not-ready with a reason, no protected route is served, and the reason leaks
//! no secret. The ported contract here is the refusal-before-start half: the
//! sink readiness gate consults the composite before anything is served, and a
//! dead sink refuses the start with a one-line typed reason naming the sink.
//! The serving half lives in `start_gate_edge.rs`, which drives the real start
//! paths (`Estate::durable` and `serve`).
//!
//! | Test | What it pins |
//! |---|---|
//! | `composite_readiness_is_the_and_of_its_children_naming_the_failed_child` | the AND of the children, and the composite names the child that failed |
//! | `probe_performs_a_real_barrier_not_a_constant` | the shipped probe performs a real fsync barrier, so a constant-true implementation fails here |
//! | `dead_store_dir_after_mount_refuses_at_the_durable_sink_gate` | a dead dir after mount refuses at the gate, sink named, one line |
//! | `recovery_after_the_cause_is_removed_starts_clean` | a restored directory starts clean, with no sticky latch |
//! | `refusal_reason_never_leaks_secrets` | the refusal carries the failure kind, never a path, a credential, or a second line |

use std::error::Error;
use std::sync::Arc;

use lgwks_bot::task::{CompositeSink, DurableSink, SinkRefusal, StoreSink};

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

/// A test-only sink whose barrier answer is decided by hand, so the composite
/// can be driven with one unready child without touching the filesystem.
struct StubSink {
    /// The name the composite must repeat when this child fails.
    name: String,
    /// The barrier answer this child reports.
    ready: bool,
}

impl DurableSink for StubSink {
    fn sink_name(&self) -> &str {
        self.name.as_str()
    }

    fn probe_barrier(&self) -> Result<(), SinkRefusal> {
        if self.ready {
            Ok(())
        } else {
            Err(SinkRefusal::new(
                self.name.clone(),
                String::from("the stubbed barrier failed"),
            ))
        }
    }
}

/// The composite is the AND of its children, and names the child that failed.
///
/// Ports the kernel `composite_readiness_is_fail_closed_and_requires_every_child`:
/// all ready reads ready, one unready child makes the composite unready, and an
/// empty fan-out is unready too — with the failing child named every time.
#[test]
fn composite_readiness_is_the_and_of_its_children_naming_the_failed_child() -> TestResult {
    let ready = || -> Arc<dyn DurableSink> {
        Arc::new(StubSink {
            name: String::from("file"),
            ready: true,
        })
    };
    let unready = || -> Arc<dyn DurableSink> {
        Arc::new(StubSink {
            name: String::from("journal"),
            ready: false,
        })
    };
    CompositeSink::new("durable-sink", [ready(), ready()])
        .readiness()
        .map_err(|refusal| format!("two ready children must read ready: {refusal}"))?;
    let Err(refusal) = CompositeSink::new("durable-sink", [ready(), unready()]).readiness() else {
        return Err(String::from("one unready child must make the composite unready").into());
    };
    assert_eq!(
        refusal.sink(),
        "durable-sink",
        "the composite names itself as the refused sink"
    );
    assert!(
        refusal.reason().contains("journal"),
        "the refusal names the failed child, got: {}",
        refusal.reason()
    );
    let Err(empty) = CompositeSink::new("durable-sink", []).readiness() else {
        return Err(String::from("an empty fan-out holds no durable sink").into());
    };
    assert_eq!(
        empty.sink(),
        "durable-sink",
        "an empty composite names itself, got: {}",
        empty.sink()
    );
    Ok(())
}

/// The probe performs a real barrier: a stubbed-to-fail barrier makes readiness
/// false, so a constant-true implementation cannot pass this test.
///
/// The shipped [`StoreSink`] is constructed over a live directory (ready) and
/// over a dead one (a file where the directory should be, so the barrier
/// create fails for every user, privileged or not).
#[test]
fn probe_performs_a_real_barrier_not_a_constant() -> TestResult {
    let live = Scratch::new("start-gate-live")?;
    StoreSink::for_dir(live.path())
        .probe_barrier()
        .map_err(|refusal| format!("a live store dir must probe ready: {refusal}"))?;
    let parent = Scratch::new("start-gate-dead")?;
    let blocker = parent.path().join("blocker");
    std::fs::write(&blocker, b"a file, not a directory")?;
    let Err(refusal) = StoreSink::for_dir(&blocker).probe_barrier() else {
        return Err(String::from("a dead store dir must probe unready").into());
    };
    assert_eq!(
        refusal.sink(),
        "runstore",
        "the file sink names itself, got: {}",
        refusal.sink()
    );
    Ok(())
}

/// A dead directory after mount refuses at the gate, naming sink and failure.
///
/// Ports the kernel `degraded_router_reports_not_ready_with_reason`: the reason
/// is operator-facing, one line, and carries the sink that failed rather than
/// a bare errno. The squatter subdirectory at the probe path stands in for a
/// directory that died after mount: the probe cannot persist there, for every
/// user, without touching permissions or mounts.
#[test]
fn dead_store_dir_after_mount_refuses_at_the_durable_sink_gate() -> TestResult {
    let dir = Scratch::new("start-gate-squat")?;
    std::fs::create_dir_all(dir.path().join(lgwks_bot::task::START_PROBE_FILE))?;
    let child: Arc<dyn DurableSink> = Arc::new(StoreSink::for_dir(dir.path()));
    let gate = CompositeSink::new("durable-sink", [child]);
    let Err(refusal) = gate.readiness() else {
        return Err(
            String::from("a store dir that cannot persist must refuse the start gate").into(),
        );
    };
    let line = refusal.to_string();
    assert!(
        line.contains("durable-sink") && line.contains("runstore"),
        "the one-line reason names sink and failure, got: {line}"
    );
    assert!(
        !line.contains('\n'),
        "the refusal is one line for the operator log, got: {line}"
    );
    Ok(())
}

/// Recovery after the cause is removed: a restored directory starts clean.
///
/// Ports the kernel tape-recovery restart test: the gate holds no sticky latch,
/// so a second probe after the repair answers ready.
#[test]
fn recovery_after_the_cause_is_removed_starts_clean() -> TestResult {
    let dir = Scratch::new("start-gate-recover")?;
    let squatter = dir.path().join(lgwks_bot::task::START_PROBE_FILE);
    std::fs::create_dir_all(&squatter)?;
    let child: Arc<dyn DurableSink> = Arc::new(StoreSink::for_dir(dir.path()));
    let gate = CompositeSink::new("durable-sink", [child]);
    if gate.readiness().is_ok() {
        return Err(String::from("the squatted dir must refuse before the repair").into());
    }
    std::fs::remove_dir(&squatter)?;
    gate.readiness()
        .map_err(|refusal| format!("the repaired dir must probe ready: {refusal}"))?;
    gate.readiness()
        .map_err(|refusal| format!("readiness must not latch: {refusal}"))?;
    Ok(())
}

/// The refusal reason never leaks secrets, and stays one line.
///
/// Ports the kernel `degraded_reason_does_not_leak_secrets`: the start gate is
/// unauthenticated operator surface, so its reason carries the failure kind,
/// never a path, a credential, or a second line.
#[test]
fn refusal_reason_never_leaks_secrets() -> TestResult {
    let parent = Scratch::new("start-gate-secrets")?;
    let secreted = parent.path().join("dir-password=hunter2-token-abc123");
    std::fs::write(&secreted, b"a file, not a directory")?;
    let Err(refusal) = StoreSink::for_dir(&secreted).probe_barrier() else {
        return Err(String::from("a dead store dir must probe unready").into());
    };
    let line = refusal.to_string();
    assert!(
        !line.contains("hunter2") && !line.contains("abc123"),
        "the refusal must not echo the path it probed, got: {line}"
    );
    assert!(
        !line.contains('\n'),
        "the refusal is one line for the operator log, got: {line}"
    );
    assert_eq!(
        refusal.sink(),
        "runstore",
        "the refusal still names its sink, got: {}",
        refusal.sink()
    );
    Ok(())
}
