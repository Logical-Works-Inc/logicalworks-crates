//! Easing curve correctness.
//!
//! Counterpart of `motion-core/tests/easing_curve_correctness.rs`
//! (kernel#850 row 15): every named curve starts near 0 and ends near 1,
//! out-of-range time clamps, bezier x-coords gate, overshoot y passes, and
//! repeated evaluation is bit-identical.

use drishti::motion::easing::{Easing, NamedCurve};

#[path = "support/fixtures.rs"]
mod fixtures;

/// Every named curve starts near zero.
#[test]
fn curves_start_near_zero() -> Result<(), String> {
    for curve in fixtures::all_named_curves() {
        let value = Easing::named(curve).value_at(0.0);
        if value.abs() >= fixtures::EASING_TOLERANCE {
            return Err(format!("{curve:?} at t=0 returned {value}, want ~0.0"));
        }
    }
    Ok(())
}

/// Every named curve ends near one.
#[test]
fn curves_end_near_one() -> Result<(), String> {
    for curve in fixtures::all_named_curves() {
        let value = Easing::named(curve).value_at(1.0);
        if (value - 1.0).abs() >= fixtures::EASING_TOLERANCE {
            return Err(format!("{curve:?} at t=1 returned {value}, want ~1.0"));
        }
    }
    Ok(())
}

/// The instant curve steps at exactly one.
#[test]
fn instant_steps_at_one() -> Result<(), String> {
    let easing = Easing::named(NamedCurve::Instant);
    fixtures::require_same_bits(
        easing.value_at(0.5),
        0.0,
        "instant must hold zero before one",
    )?;
    fixtures::require_same_bits(easing.value_at(1.0), 1.0, "instant must step at one")?;
    Ok(())
}

/// Out-of-range time clamps to the endpoints, bit-identically.
#[test]
fn time_clamps_outside_unit_interval() -> Result<(), String> {
    let easing = Easing::named(NamedCurve::CubicInOut);
    fixtures::require_same_bits(
        easing.value_at(-0.5),
        easing.value_at(0.0),
        "negative time must clamp to t=0",
    )?;
    fixtures::require_same_bits(
        easing.value_at(1.5),
        easing.value_at(1.0),
        "time past one must clamp to t=1",
    )?;
    Ok(())
}

/// Bezier x-coords outside [0, 1] are rejected on either side.
#[test]
fn bezier_rejects_out_of_domain_x() -> Result<(), String> {
    if Easing::cubic_bezier(1.5, 0.0, 0.5, 1.0).is_ok() {
        return Err(String::from("p1x=1.5 must be rejected"));
    }
    if Easing::cubic_bezier(0.5, 0.0, -0.1, 1.0).is_ok() {
        return Err(String::from("p2x=-0.1 must be rejected"));
    }
    Ok(())
}

/// Overshoot y values are accepted for bounce/back effects.
#[test]
fn bezier_accepts_overshoot_y() -> Result<(), String> {
    Easing::cubic_bezier(0.4, -0.5, 0.6, 1.5)
        .map_err(|error| format!("overshoot y must be accepted: {error}"))?;
    Ok(())
}

/// Newton-Raphson converges and stays finite on a realistic curve.
#[test]
fn bezier_converges_and_stays_finite() -> Result<(), String> {
    // power3.inOut control points: (0.645, 0.045, 0.355, 1.0).
    let easing = Easing::cubic_bezier(0.645, 0.045, 0.355, 1.0)
        .map_err(|error| format!("valid bezier rejected: {error}"))?;
    for index in 0..=10_u16 {
        let time = f32::from(index) / 10.0;
        let value = easing.value_at(time);
        if !value.is_finite() {
            return Err(format!("bezier eval at t={time} returned {value}"));
        }
    }
    if easing.value_at(0.0).abs() >= fixtures::EASING_TOLERANCE {
        return Err(String::from("bezier must start at zero"));
    }
    if (easing.value_at(1.0) - 1.0).abs() >= fixtures::EASING_TOLERANCE {
        return Err(String::from("bezier must end at one"));
    }
    Ok(())
}

/// Repeated evaluation is bit-identical.
#[test]
fn bezier_repeats_bit_identically() -> Result<(), String> {
    let easing = Easing::cubic_bezier(0.25, 0.1, 0.25, 1.0)
        .map_err(|error| format!("valid bezier rejected: {error}"))?;
    for index in 0..=20_u16 {
        let time = f32::from(index) / 20.0;
        fixtures::require_same_bits(
            easing.value_at(time),
            easing.value_at(time),
            &format!("bezier not deterministic at t={time}"),
        )?;
    }
    Ok(())
}
