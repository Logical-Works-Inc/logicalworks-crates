//! Float tolerance: bit-identical repeats and finite edges.
//!
//! Counterpart of `motion-core/tests/float_tolerance.rs` (kernel#850 row 15):
//! easing and OKLab operations produce bit-identical results on repeated
//! calls with identical inputs.

use drishti::motion::easing::Easing;
use drishti::motion::interpolate::color_oklab::{SRgb, interp_oklab};

#[path = "support/fixtures.rs"]
mod fixtures;

/// Every easing repeats bit-identically across its domain.
#[test]
fn easings_repeat_bit_identically() -> Result<(), String> {
    for curve in fixtures::all_named_curves() {
        let easing = Easing::named(curve);
        for index in 0..=100_u16 {
            let time = f32::from(index) / 100.0;
            fixtures::require_same_bits(
                easing.value_at(time),
                easing.value_at(time),
                &format!("{curve:?} at t={time} is not bit-identical"),
            )?;
        }
    }
    Ok(())
}

/// The bezier path repeats bit-identically.
#[test]
fn bezier_repeats_bit_identically() -> Result<(), String> {
    let easing = Easing::cubic_bezier(0.645, 0.045, 0.355, 1.0)
        .map_err(|error| format!("valid bezier rejected: {error}"))?;
    for index in 0..=50_u16 {
        let time = f32::from(index) / 50.0;
        fixtures::require_same_bits(
            easing.value_at(time),
            easing.value_at(time),
            &format!("bezier at t={time} is not bit-identical"),
        )?;
    }
    Ok(())
}

/// OKLab interpolation repeats bit-identically per channel.
#[test]
fn oklab_repeats_bit_identically() -> Result<(), String> {
    let red = SRgb::new(1.0, 0.0, 0.0);
    let blue = SRgb::new(0.0, 0.0, 1.0);
    for index in 0..=20_u16 {
        let time = f32::from(index) / 20.0;
        let first = interp_oklab(red, blue, time);
        let second = interp_oklab(red, blue, time);
        fixtures::require_same_bits(
            first.red(),
            second.red(),
            &format!("OKLab red differs at t={time}"),
        )?;
        fixtures::require_same_bits(
            first.green(),
            second.green(),
            &format!("OKLab green differs at t={time}"),
        )?;
        fixtures::require_same_bits(
            first.blue(),
            second.blue(),
            &format!("OKLab blue differs at t={time}"),
        )?;
    }
    Ok(())
}

/// Every easing stays finite at endpoints and just inside them.
#[test]
fn easings_stay_finite_at_edges() -> Result<(), String> {
    for curve in fixtures::all_named_curves() {
        let easing = Easing::named(curve);
        for time in [0.0_f32, f32::EPSILON, 0.5, 1.0 - f32::EPSILON, 1.0] {
            if !easing.value_at(time).is_finite() {
                return Err(format!("{curve:?} at t={time} is not finite"));
            }
        }
    }
    Ok(())
}
