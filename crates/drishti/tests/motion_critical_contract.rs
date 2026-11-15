//! Analytic motion contract.
//!
//! Counterpart of `motion-core/tests/critical_contract.rs` (kernel#850 row
//! 15): closed-form agreement, retarget continuity, split-step equivalence,
//! invalid-input rejection without mutation, response gating, long-run
//! finiteness, and explicit snapping. The closed-form oracle uses the
//! platform exponential while the implementation uses `libm`, so agreement
//! is asserted to 1e-12, not bit equality.

use drishti::motion::node::critical::CriticalSpring;

/// Build a spring or fail the test with context.
fn spring(position: f64, response: f64) -> Result<CriticalSpring, String> {
    CriticalSpring::new(position, response)
        .map_err(|error| format!("valid spring rejected: {error}"))
}

/// Closed form from rest matches `1 - 2.2e^-1.2` at 0.1 seconds.
#[test]
fn matches_closed_form_from_rest() -> Result<(), String> {
    let mut spring = spring(0.0, 0.5)?;
    spring
        .retarget(1.0)
        .map_err(|error| format!("valid retarget rejected: {error}"))?;
    spring
        .advance(0.1)
        .map_err(|error| format!("valid advance rejected: {error}"))?;
    let expected = 1.0 - 2.2 * (-1.2_f64).exp();
    if (spring.position() - expected).abs() >= 1e-12 {
        return Err(format!(
            "position {} differs from closed form {expected}",
            spring.position()
        ));
    }
    Ok(())
}

/// Retargeting preserves position and velocity bit-identically.
#[test]
fn retarget_preserves_position_and_velocity() -> Result<(), String> {
    let mut spring = spring(0.0, 0.45)?;
    spring
        .retarget(2.0)
        .map_err(|error| format!("valid retarget rejected: {error}"))?;
    spring
        .advance(0.04)
        .map_err(|error| format!("valid advance rejected: {error}"))?;
    let before = (spring.position().to_bits(), spring.velocity().to_bits());
    spring
        .retarget(0.0)
        .map_err(|error| format!("valid retarget rejected: {error}"))?;
    let after = (spring.position().to_bits(), spring.velocity().to_bits());
    if before != after {
        return Err(String::from("retarget must preserve position and velocity"));
    }
    spring
        .advance(0.04)
        .map_err(|error| format!("valid advance rejected: {error}"))?;
    if !spring.position().is_finite() {
        return Err(String::from("position must stay finite"));
    }
    Ok(())
}

/// Nine 0.01 steps agree with one 0.09 step.
#[test]
fn split_step_and_whole_step_agree() -> Result<(), String> {
    let mut whole = spring(-2.0, 0.3)?;
    whole
        .retarget(5.0)
        .map_err(|error| format!("valid retarget rejected: {error}"))?;
    let mut split = whole;
    whole
        .advance(0.09)
        .map_err(|error| format!("valid advance rejected: {error}"))?;
    for _ in 0..9 {
        split
            .advance(0.01)
            .map_err(|error| format!("valid advance rejected: {error}"))?;
    }
    if (split.position() - whole.position()).abs() >= 1e-11 {
        return Err(String::from("split positions disagree"));
    }
    if (split.velocity() - whole.velocity()).abs() >= 1e-10 {
        return Err(String::from("split velocities disagree"));
    }
    Ok(())
}

/// Invalid time and target reject without mutating state.
#[test]
fn invalid_time_and_target_leave_state_unchanged() -> Result<(), String> {
    let mut spring = spring(0.0, 0.5)?;
    for time in [f64::NAN, f64::INFINITY, -0.01, 60.01] {
        if spring.advance(time).is_ok() {
            return Err(format!("time {time} must be rejected"));
        }
        if spring.position().to_bits() != 0.0_f64.to_bits() {
            return Err(String::from("rejected advance must not mutate position"));
        }
    }
    if spring.retarget(f64::NAN).is_ok() {
        return Err(String::from("NaN target must be rejected"));
    }
    if spring.target().to_bits() != 0.0_f64.to_bits() {
        return Err(String::from("rejected retarget must not mutate target"));
    }
    Ok(())
}

/// Invalid responses are rejected.
#[test]
fn invalid_response_is_rejected() -> Result<(), String> {
    for response in [f64::NAN, f64::INFINITY, 0.0, -1.0, 10.1] {
        if CriticalSpring::new(0.0, response).is_ok() {
            return Err(format!("response {response} must be rejected"));
        }
    }
    Ok(())
}

/// A thousand interruptions plus a 60-second sleep stay finite and settle.
#[test]
fn long_sleep_and_repeated_interruptions_remain_finite() -> Result<(), String> {
    let mut spring = spring(0.0, 0.016)?;
    let mut target = 0_i32;
    for _ in 0..1000 {
        spring
            .retarget(f64::from(target))
            .map_err(|error| format!("valid retarget rejected: {error}"))?;
        spring
            .advance(0.001)
            .map_err(|error| format!("valid advance rejected: {error}"))?;
        if !spring.position().is_finite() || !spring.velocity().is_finite() {
            return Err(String::from("state must stay finite"));
        }
        target = if target >= 2 {
            0
        } else {
            target.wrapping_add(1)
        };
    }
    spring
        .advance(60.0)
        .map_err(|error| format!("valid advance rejected: {error}"))?;
    if !spring.is_settled() {
        return Err(String::from("spring must settle after 60 seconds"));
    }
    Ok(())
}

/// Snapping is explicit and clears velocity.
#[test]
fn snap_is_explicit_and_clears_velocity() -> Result<(), String> {
    let mut spring = spring(0.0, 0.5)?;
    spring
        .retarget(2.0)
        .map_err(|error| format!("valid retarget rejected: {error}"))?;
    spring.snap();
    if (spring.position().to_bits(), spring.velocity().to_bits())
        != (2.0_f64.to_bits(), 0.0_f64.to_bits())
    {
        return Err(String::from("snap must set (target, 0)"));
    }
    Ok(())
}
