//! Trace checker oracle: synthetic fixtures prove the checker accepts the
//! valid shape and rejects every corruption.
//!
//! Counterpart of `tools/test_oracle.py` (kernel#850 row 15): the eleven
//! unittest cases over synthetic (never Rust-run) fixtures, plus the
//! closed-form segment fixture computed independently from per-phase totals
//! rather than the checker's own recurrence.

use drishti::preview::check_records;
use lgwks_deps::libm;
use lgwks_std::json::{Value, json};

#[path = "support/fixtures.rs"]
mod fixtures;

use fixtures::ensure;

/// Synthetic reduced 4-frame fixture with its summary.
fn fixture() -> Vec<Value> {
    let mut rows = Vec::new();
    for (step, target) in [
        ("initial", 0.0),
        ("toward-evidence", 2.0),
        ("interrupt", 0.0),
        ("settle", 1.0),
    ] {
        rows.push(json!({
            "kind": "frame", "mode": "headless", "step": step,
            "object_key": "fixture", "content_revision": 1,
            "position": target, "velocity": 0.0, "target": target,
            "reduced": true, "native_presented": false, "source_bytes": 7,
        }));
    }
    rows.push(json!({
        "kind": "summary", "idle_no_frame": true, "revocation_refused": true,
        "native_presented": false, "prepared": 4, "acknowledged": 4,
    }));
    rows
}

/// Replace one field of one row (the estate value has no index assignment):
/// an out-of-range row or a non-object row stays as it was, which the
/// rejection tests would then fail loudly on.
fn set(rows: &mut [Value], index: usize, key: &str, value: Value) {
    if let Some(row) = rows.get_mut(index)
        && let Value::Object(mut map) = row.clone()
    {
        map.insert(String::from(key), value);
        *row = Value::Object(map);
    }
}

/// The valid reduced fixture checks four frames.
#[test]
fn valid_reduced_fixture_checks() -> Result<(), String> {
    ensure(check_records(&fixture(), 7, true) == Ok(4), || {
        String::from("valid fixture must check four")
    })
}

/// One corruption vector: mutate a fresh fixture, then require rejection.
fn require_rejection(mutate: impl FnOnce(&mut Vec<Value>), what: &str) -> Result<(), String> {
    let mut rows = fixture();
    mutate(&mut rows);
    ensure(check_records(&rows, 7, true).is_err(), || {
        String::from(what)
    })
}

/// Each corruption is rejected.
#[test]
fn corruptions_reject() -> Result<(), String> {
    // Missing summary.
    require_rejection(void_pop, "missing summary must fail")?;
    // False native claim.
    require_rejection(
        |rows| {
            set(rows, 1, "native_presented", Value::Bool(true));
        },
        "false native must fail",
    )?;
    // Wrong source size.
    require_rejection(
        |rows| {
            set(rows, 0, "source_bytes", json!(8));
        },
        "wrong bytes must fail",
    )?;
    // Identity drift.
    require_rejection(
        |rows| {
            set(rows, 1, "object_key", json!("other"));
        },
        "identity drift must fail",
    )?;
    // Trajectory error.
    require_rejection(
        |rows| {
            set(rows, 2, "position", json!(0.5));
        },
        "trajectory error must fail",
    )?;
    // Non-finite trajectory: JSON has no NaN, so corrupt the shape instead
    // (a string where a number belongs).
    require_rejection(
        |rows| {
            set(rows, 1, "position", json!("NaN"));
        },
        "non-numeric trajectory must fail",
    )?;
    // False idle.
    require_rejection(
        |rows| {
            let last = rows.len().saturating_sub(1);
            set(rows, last, "idle_no_frame", Value::Bool(false));
        },
        "false idle must fail",
    )?;
    // False accounting.
    require_rejection(
        |rows| {
            let last = rows.len().saturating_sub(1);
            set(rows, last, "prepared", json!(400));
        },
        "false accounting must fail",
    )?;
    Ok(())
}

/// Drop the trailing summary row.
fn void_pop(rows: &mut Vec<Value>) {
    rows.pop();
}

/// Wrong mode and truncated phases are rejected.
#[test]
fn mode_and_phase_mismatch_reject() -> Result<(), String> {
    ensure(check_records(&fixture(), 7, false).is_err(), || {
        String::from("wrong mode must fail")
    })?;
    let mut rows = fixture();
    set(&mut rows, 2, "step", json!("settle"));
    ensure(check_records(&rows, 7, true).is_err(), || {
        String::from("truncated phases must fail")
    })
}

/// Closed-form normal fixture computed from per-phase totals, not the
/// checker's recurrence.
#[test]
fn closed_form_normal_fixture_checks() -> Result<(), String> {
    let mut rows = vec![fixture()[0].clone()];
    set(&mut rows, 0, "reduced", Value::Bool(false));
    let mut position = 0.0_f64;
    let mut velocity = 0.0_f64;
    let omega = 6.0_f64 / 0.45_f64;
    let phases = [
        ("toward-evidence", 2.0, 5),
        ("interrupt", 0.0, 5),
        ("settle", 1.0, 240),
    ];
    for (label, target, count) in phases {
        let start = position;
        let start_velocity = velocity;
        let mut emitted = 0_usize;
        for index in 1..=count {
            let elapsed = f64::from(index) / 60.0;
            let displacement = start - target;
            let coefficient = start_velocity + omega * displacement;
            let decay = libm::exp(-omega * elapsed);
            position = target + (displacement + coefficient * elapsed) * decay;
            velocity = (start_velocity - omega * coefficient * elapsed) * decay;
            let settled = (position - target).abs() < 1e-5 && velocity.abs() < 1e-4;
            if settled {
                position = target;
                velocity = 0.0;
            }
            let mut frame = vec![rows[0].clone()];
            set(&mut frame, 0, "step", json!(label));
            set(&mut frame, 0, "target", json!(target));
            set(&mut frame, 0, "position", json!(position));
            set(&mut frame, 0, "velocity", json!(velocity));
            rows.extend(frame);
            emitted = emitted.saturating_add(1);
            if settled {
                break;
            }
        }
        ensure(emitted != 0, || String::from("phase must emit"))?;
    }
    let frames = rows.len();
    rows.push(json!({
        "kind": "summary", "idle_no_frame": true, "revocation_refused": true,
        "native_presented": false, "prepared": frames,
        "acknowledged": frames,
    }));
    ensure(check_records(&rows, 7, false) == Ok(frames), || {
        String::from("closed-form fixture must check")
    })
}
