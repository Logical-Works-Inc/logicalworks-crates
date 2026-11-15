//! Spring stability: convergence, bounded oscillation, critical damping.
//!
//! Counterpart of `motion-core/tests/spring_stability.rs` (kernel#850 row
//! 15). The RK4 integrator converges production springs, conserves the
//! undamped oscillator below 5x, never overshoots at critical damping, and
//! shrinks the state norm for every production configuration.

use drishti::motion::node::spring::{SpringConfig, integrate_spring};
use lgwks_deps::libm;

/// 60 Hz frame step.
const DT: f32 = 0.0166;

/// Spring target.
const TARGET: f32 = 1.0;

/// Convergence tolerance after ten seconds.
const CONVERGENCE_TOLERANCE: f32 = 0.01;

/// Build a unit-mass spring or fail the test.
fn spring(tension: f32, friction: f32) -> Result<SpringConfig, String> {
    SpringConfig::new(tension, friction, 1.0)
        .map_err(|error| format!("valid spring rejected: {error}"))
}

/// Positive tension and friction converge within ten seconds.
#[test]
fn converges_with_positive_tension_and_friction() -> Result<(), String> {
    let config = spring(170.0, 26.0)?;
    let mut position = 0.0_f32;
    let mut velocity = 0.0_f32;
    for _ in 0..600 {
        let (next_position, next_velocity) =
            integrate_spring(position, velocity, TARGET, &config, DT);
        position = next_position;
        velocity = next_velocity;
    }
    if (position - TARGET).abs() >= CONVERGENCE_TOLERANCE {
        return Err(format!("spring did not converge: pos={position}"));
    }
    Ok(())
}

/// Zero friction conserves energy: oscillation stays bounded below 5x.
#[test]
fn zero_friction_stays_bounded() -> Result<(), String> {
    let config = spring(100.0, 0.0)?;
    let mut position = 0.0_f32;
    let mut velocity = 0.0_f32;
    let mut max_position: f32 = 0.0;
    for _ in 0..600 {
        let (next_position, next_velocity) =
            integrate_spring(position, velocity, TARGET, &config, DT);
        position = next_position;
        velocity = next_velocity;
        if position.abs() > max_position {
            max_position = position.abs();
        }
    }
    if max_position >= 5.0 * TARGET {
        return Err(format!("zero-friction spring diverged: {max_position}"));
    }
    Ok(())
}

/// Critical damping (`friction = 2 sqrt(tension mass)`) never overshoots.
#[test]
fn critical_damping_never_overshoots() -> Result<(), String> {
    let tension = 100.0_f32;
    let mass = 1.0_f32;
    let friction = 2.0 * libm::sqrtf(tension * mass);
    let config = SpringConfig::new(tension, friction, mass)
        .map_err(|error| format!("valid spring rejected: {error}"))?;
    let mut position = 0.0_f32;
    let mut velocity = 0.0_f32;
    for _ in 0..600 {
        let (next_position, next_velocity) =
            integrate_spring(position, velocity, TARGET, &config, DT);
        if next_position > TARGET + 1e-3 {
            return Err(String::from("critically damped spring overshot"));
        }
        position = next_position;
        velocity = next_velocity;
    }
    Ok(())
}

/// Every production configuration shrinks the state norm: spectral radius
/// below one over two hundred steps.
#[test]
fn production_configs_shrink_state_norm() -> Result<(), String> {
    let configs = [
        (170.0_f32, 26.0_f32, 1.0_f32),
        (100.0, 20.0, 1.0),
        (300.0, 40.0, 1.0),
        (50.0, 10.0, 0.5),
    ];
    for (tension, friction, mass) in configs {
        let config = SpringConfig::new(tension, friction, mass)
            .map_err(|error| format!("valid spring rejected: {error}"))?;
        let mut position = 0.0_f32;
        let mut velocity = 0.0_f32;
        let mut early_distance = f32::MAX;
        for step in 0..200_u32 {
            let (next_position, next_velocity) =
                integrate_spring(position, velocity, TARGET, &config, DT);
            position = next_position;
            velocity = next_velocity;
            if step == 10 {
                early_distance = libm::sqrtf((position - TARGET).powi(2) + velocity * velocity);
            }
        }
        let final_distance = libm::sqrtf((position - TARGET).powi(2) + velocity * velocity);
        if final_distance >= early_distance {
            return Err(format!(
                "integrator unstable for tension={tension} friction={friction} mass={mass}"
            ));
        }
    }
    Ok(())
}

/// Negative tension and zero mass are rejected.
#[test]
fn rejects_negative_tension_and_zero_mass() -> Result<(), String> {
    if SpringConfig::new(-1.0, 10.0, 1.0).is_ok() {
        return Err(String::from("negative tension must be rejected"));
    }
    if SpringConfig::new(100.0, 10.0, 0.0).is_ok() {
        return Err(String::from("zero mass must be rejected"));
    }
    Ok(())
}
