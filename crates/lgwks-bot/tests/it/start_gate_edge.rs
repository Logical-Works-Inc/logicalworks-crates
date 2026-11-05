//! Fail-closed start over the durable run-store sink, at the start paths.
//!
//! The contract half lives in `start_gate.rs` (composite AND, real barrier,
//! recovery, no secret leak). This file drives the two production start paths
//! that must consult it: [`Estate::durable`](lgwks_bot::rt::estate::Estate)
//! and [`serve`](lgwks_bot::rt::serve::serve). A dead store directory makes
//! the next start refuse with a non-zero exit and the sink named, serves no
//! request, and recovers cleanly once the directory is restored.
//!
//! | Test | What it pins |
//! |---|---|
//! | `dead_store_dir_after_mount_refuses_the_estate_gate` | a dead dir refuses `Estate::durable` at the durable-sink gate, sink named, one line |
//! | `restored_directory_starts_clean_with_no_sticky_latch` | removing the cause lets the next start through, twice |
//! | `refused_start_serves_no_request_and_names_the_sink` | `serve` refuses before accepting, the port stays closed, the sink is named |
//! | `refused_start_exits_non_zero_from_the_shipped_binary` | `estate serve` over a dead store exits non-zero and serves nothing |

#![cfg(all(feature = "edge", feature = "ephemeral"))]

use std::net::TcpListener;

use lgwks_bot::rt::edge::EdgeStop;
use lgwks_bot::rt::estate::{Estate, EstateError};
use lgwks_bot::rt::serve::{ServeError, ServeOptions, serve};

use crate::scratch::Scratch;
use crate::serve_edge::{estate_bin, free_port};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A scratch store directory whose probe path squats under a subdirectory, so
/// the barrier cannot persist there for any user, without permissions or mounts.
fn squatted_dir(tag: &str) -> Result<Scratch, Box<dyn std::error::Error>> {
    let dir = Scratch::new(tag)?;
    std::fs::create_dir_all(dir.path().join(lgwks_bot::task::START_PROBE_FILE))?;
    Ok(dir)
}

/// A dead directory after mount refuses the estate gate, naming the sink.
///
/// Ports the kernel `run_startup_sequence` fail-closed row for the journal: a
/// journal that cannot persist degrades the boot at the durable-sink gate
/// rather than after the first accepted request.
#[test]
fn dead_store_dir_after_mount_refuses_the_estate_gate() -> TestResult {
    let dir = squatted_dir("start-gate-estate-dead")?;
    let Err(refusal) = Estate::durable("acme", 8, dir.path()) else {
        return Err(String::from("a dead store dir must refuse the durable estate gate").into());
    };
    let line = refusal.to_string();
    let EstateError::StartGate { sink, reason } = refusal else {
        return Err(
            String::from("the refusal must be the typed start-gate arm, naming the sink").into(),
        );
    };
    assert_eq!(
        sink.as_str(),
        "durable-sink",
        "the gate names its composite sink, got: {sink}"
    );
    assert!(
        reason.contains("runstore"),
        "the reason names the failed child, got: {reason}"
    );
    assert!(
        line.contains("durable-sink") && line.contains("runstore"),
        "the one-line refusal names sink and failure, got: {line}"
    );
    assert!(
        !line.contains('\n'),
        "the refusal is one line for the operator log, got: {line}"
    );
    Ok(())
}

/// Recovery: removing the cause lets the next start through, with no latch.
///
/// The squatter is removed and the same directory opens twice, so a refusal
/// that stuck would fail the second open rather than the first probe.
#[test]
fn restored_directory_starts_clean_with_no_sticky_latch() -> TestResult {
    let dir = Scratch::new("start-gate-estate-recover")?;
    let squatter = dir.path().join(lgwks_bot::task::START_PROBE_FILE);
    std::fs::create_dir_all(&squatter)?;
    if Estate::durable("acme", 8, dir.path()).is_ok() {
        return Err(String::from("the squatted dir must refuse before the repair").into());
    }
    std::fs::remove_dir(&squatter)?;
    let first = Estate::durable("acme", 8, dir.path())
        .map_err(|refusal| format!("the repaired dir must open: {refusal}"))?;
    drop(first);
    Estate::durable("acme", 8, dir.path())
        .map_err(|refusal| format!("readiness must not latch: {refusal}"))?;
    Ok(())
}

/// A refused start serves no request: `serve` returns before accepting, the
/// port the caller bound stays closed, and the sink is named.
///
/// Ports the kernel `degraded_router_does_not_expose_protected_routes`: in the
/// refused start there is no degraded plane at all, so a protected route is
/// not merely guarded — nothing answers it.
#[test]
fn refused_start_serves_no_request_and_names_the_sink() -> TestResult {
    let dir = squatted_dir("start-gate-serve-dead")?;
    let options = ServeOptions::new(0, 1_000, dir.path().to_path_buf())
        .map_err(|refusal| format!("the serve options must build: {refusal}"))?;
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|refusal| format!("bind refused: {refusal}"))?;
    let addr = listener
        .local_addr()
        .map_err(|refusal| format!("the loopback address is refused: {refusal}"))?;
    let stop = EdgeStop::new();
    let Err(refusal) = serve(&options, std::vec![], listener, &stop) else {
        return Err(String::from("a dead store dir must refuse the serve gate").into());
    };
    let line = refusal.to_string();
    let ServeError::StartGate { sink, reason } = refusal else {
        return Err(
            String::from("the refusal must be the typed start-gate arm, naming the sink").into(),
        );
    };
    assert_eq!(
        sink.as_str(),
        "durable-sink",
        "the gate names its composite sink, got: {sink}"
    );
    assert!(
        reason.contains("runstore"),
        "the reason names the failed child, got: {reason}"
    );
    assert!(
        !line.contains('\n'),
        "the refusal is one line for the operator log, got: {line}"
    );
    match std::net::TcpStream::connect(addr) {
        Ok(_) => Err(String::from("nothing may answer on a refused start").into()),
        Err(_) => Ok(()),
    }
}

/// The shipped binary exits non-zero over a dead store and serves nothing.
///
/// Ports the kernel `degraded_router_keeps_liveness_up` by inversion: there is
/// no degraded plane to keep alive, so the process reports its refusal on its
/// exit status instead of on a readiness probe, and the port stays closed.
#[test]
fn refused_start_exits_non_zero_from_the_shipped_binary() -> TestResult {
    let dir = squatted_dir("start-gate-serve-bin-dead")?;
    let port = free_port().map_err(|refusal| format!("no loopback port is free: {refusal}"))?;
    let store = dir
        .path()
        .to_str()
        .ok_or_else(|| String::from("the store path is not text"))?;
    let output = std::process::Command::new(
        estate_bin().map_err(|refusal| format!("the estate binary is missing: {refusal}"))?,
    )
    .arg("serve")
    .arg("--store")
    .arg(store)
    .arg("--port")
    .arg(std::format!("{port}"))
    .output()
    .map_err(|refusal| format!("estate serve did not run: {refusal}"))?;
    assert!(
        !output.status.success(),
        "a dead store dir must exit non-zero, got: {}",
        output.status
    );
    let addr = std::format!("127.0.0.1:{port}");
    match std::net::TcpStream::connect(addr.as_str()) {
        Ok(_) => Err(String::from("nothing may answer on a refused start").into()),
        Err(_) => Ok(()),
    }
}
