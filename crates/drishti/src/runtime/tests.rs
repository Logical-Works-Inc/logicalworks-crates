//! Runtime residency tests: idle frames, budgets, device loss, uploads, reuse and replay.

#[cfg(test)]
use super::{Budgets, DeviceState, HostId, Runtime, RuntimeError, RuntimeReport};
use crate::ink_lab::{Canvas, Effect, HostSpec, Input};

/// A runtime with one loader host.
fn runtime_with_loader(seed: u64) -> (Runtime, HostId) {
    let mut runtime = Runtime::new(seed, Budgets::conservative());
    let id = admitted(runtime.add_host(HostSpec {
        effect: Effect::Loader,
        canvas: Canvas::new(120.0, 120.0),
        visible: true,
    }));
    (runtime, id)
}

/// The admitted host id; a refusal fails the test rather than inventing one.
fn admitted(result: Result<HostId, RuntimeError>) -> HostId {
    let succeeded = result.is_ok();
    match result {
        Ok(host) => host,
        Err(error) => {
            assert!(succeeded, "host refused: {error:?}");
            HostId(u32::MAX)
        }
    }
}

/// Unwrap a report without `unwrap`/`expect`/`panic`.
fn presented(result: Result<RuntimeReport, RuntimeError>) -> RuntimeReport {
    let succeeded = result.is_ok();
    match result {
        Ok(report) => report,
        Err(error) => {
            assert!(succeeded, "present refused: {error:?}");
            RuntimeReport {
                receipts: Vec::new(),
                uploads: Vec::new(),
                idle: true,
                epoch: 0,
                uploaded_bytes: 0,
                reused: 0,
            }
        }
    }
}

#[test]
fn idle_runtime_submits_nothing() {
    let mut runtime = Runtime::new(1, Budgets::conservative());
    let report = presented(runtime.present(16.0));
    assert!(report.idle());
    assert!(report.receipts().is_empty());
    assert_eq!(runtime.totals().idle_frames, 1);
}

#[test]
fn idle_frames_do_not_upload() {
    let (mut runtime, _) = runtime_with_loader(4);
    presented(runtime.present(0.0));
    let before = runtime.totals().uploaded_bytes;
    runtime.set_paused(true);
    let report = presented(runtime.present(16.0));
    assert!(report.idle(), "paused engine should be idle");
    assert_eq!(
        runtime.totals().uploaded_bytes,
        before,
        "idle advanced uploads"
    );
}

#[test]
fn budget_pressure_is_refused_not_silently_dropped() {
    let budgets = Budgets {
        max_in_flight: 1,
        ..Budgets::conservative()
    };
    let mut runtime = Runtime::new(2, budgets);
    for _ in 0..3 {
        assert!(
            runtime
                .add_host(HostSpec {
                    effect: Effect::Loader,
                    canvas: Canvas::new(80.0, 80.0),
                    visible: true,
                })
                .is_ok(),
            "host budget should admit three hosts"
        );
    }
    assert!(
        matches!(runtime.present(16.0), Err(RuntimeError::Backpressure)),
        "over-budget frame was not refused"
    );
    assert_eq!(
        runtime.totals().uploaded_bytes,
        0,
        "refused frame committed bytes"
    );
}

#[test]
fn device_loss_refuses_frames_until_epoch_bump() {
    let (mut runtime, _) = runtime_with_loader(6);
    presented(runtime.present(16.0));
    runtime.lose_device();
    assert_eq!(runtime.state(), DeviceState::Lost);
    assert!(
        matches!(runtime.present(16.0), Err(RuntimeError::DeviceUnavailable)),
        "lost device accepted a frame"
    );
    let before = runtime.epoch();
    let after = runtime.recover();
    assert_eq!(after, before.wrapping_add(1), "recovery must bump epoch");
    assert!(runtime.present(16.0).is_ok());
}

#[test]
fn uploads_carry_exactly_the_changed_frames() {
    let (mut runtime, id) = runtime_with_loader(8);
    runtime.set_reduce_motion(true);
    let first = presented(runtime.present(16.0));
    assert_eq!(first.uploads().len(), 1, "first frame must upload");
    let upload = first.uploads().first();
    assert_eq!(
        upload.map(|render| render.host),
        Some(id),
        "upload belongs to the host"
    );
    // The resolved loader is an empty frame: zero bytes, yet still an
    // upload, because it must replace whatever the backend had resident.
    assert_eq!(
        upload.map(|render| render.frame.shapes().is_empty()),
        Some(true),
        "resolved loader is empty"
    );
    // The resolved loader is finished: no demand, so nothing uploads.
    let second = presented(runtime.present(16.0));
    assert!(second.idle(), "a resolved host has no demand");
    assert!(second.uploads().is_empty(), "idle frame uploaded");
}

#[test]
fn a_reused_frame_is_receipted_but_not_uploaded() {
    let mut runtime = Runtime::new(5, Budgets::conservative());
    let id = admitted(runtime.add_host(HostSpec {
        effect: Effect::Mark,
        canvas: Canvas::new(100.0, 100.0),
        visible: true,
    }));
    // The mark is crisp from 1850 ms until its hold ends at 3700 ms, so
    // consecutive frames in that window are identical.
    for _ in 0..125 {
        presented(runtime.present(16.0));
    }
    let held = presented(runtime.present(16.0));
    assert_eq!(held.receipts().len(), 1, "the mark has demand");
    let receipt = held.receipts().first();
    assert_eq!(
        receipt.map(|entry| entry.host),
        Some(id),
        "receipt belongs to the mark"
    );
    assert_eq!(
        receipt.map(|entry| entry.upload_bytes),
        Some(0),
        "held mark re-uploaded"
    );
    assert!(
        held.uploads().is_empty(),
        "held mark delivered as an upload"
    );
}

#[test]
fn host_limit_is_enforced() {
    let budgets = Budgets {
        max_hosts: 1,
        ..Budgets::conservative()
    };
    let mut runtime = Runtime::new(3, budgets);
    let spec = HostSpec {
        effect: Effect::Loader,
        canvas: Canvas::new(50.0, 50.0),
        visible: true,
    };
    assert!(runtime.add_host(spec).is_ok());
    assert_eq!(runtime.add_host(spec), Err(RuntimeError::HostLimit));
}

#[test]
fn seeded_runtimes_replay_identical_frames() {
    let run = |seed: u64| {
        let mut runtime = Runtime::new(seed, Budgets::conservative());
        let id = admitted(runtime.add_host(HostSpec {
            effect: Effect::Ripple,
            canvas: Canvas::new(120.0, 120.0),
            visible: true,
        }));
        let mut trace = 0_u64;
        for _ in 0..20 {
            let accepted = runtime.set_input(
                id,
                Input {
                    pointer: None,
                    click: Some((30.0, 30.0)),
                },
            );
            assert!(accepted.is_ok(), "finite click refused");
            let report = presented(runtime.present(16.0));
            for receipt in report.receipts() {
                trace = trace.wrapping_mul(31).wrapping_add(receipt.digest);
            }
        }
        trace
    };
    let original = run(11);
    let replayed = run(11);
    let reseeded = run(12);
    assert_eq!(original, replayed, "replay diverged");
    assert_ne!(original, reseeded, "seed did not change the trace");
}
