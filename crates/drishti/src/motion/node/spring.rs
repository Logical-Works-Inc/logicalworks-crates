//! Damped harmonic oscillator via RK4 integration.
//!
//! RK4 is chosen because its spectral radius is below 1 for all
//! production-realistic (tension, friction, mass, dt) combinations. Euler
//! integration diverges for large tension/small dt ratios. All arithmetic uses
//! `f32`; no transcendental functions are called here, so `libm` is unneeded.

use super::super::error::{MotionError, MotionErrorCode};

/// Spring parameters. Positive tension and friction are enforced at
/// construction; mass must be positive.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use drishti::motion::SpringConfig;
///
/// let config = SpringConfig::new(100.0, 10.0, 1.0)?;
/// assert_eq!((config.tension(), config.friction(), config.mass()), (100.0, 10.0, 1.0));
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringConfig {
    /// Spring stiffness.
    tension: f32,
    /// Damping.
    friction: f32,
    /// Mass.
    mass: f32,
}

impl SpringConfig {
    /// Build spring parameters, rejecting non-finite tension/friction, any
    /// negative tension/friction, and non-positive mass.
    ///
    /// # Errors
    ///
    /// Returns [`MotionErrorCode::SpringUnstable`]
    /// for any out-of-domain parameter.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{MotionErrorCode, SpringConfig};
    ///
    /// assert!(SpringConfig::new(100.0, 10.0, 1.0).is_ok());
    /// let refused = SpringConfig::new(-1.0, 10.0, 1.0);
    /// assert_eq!(
    ///     refused.err().map(|error| error.code()),
    ///     Some(MotionErrorCode::SpringUnstable)
    /// );
    /// assert!(SpringConfig::new(100.0, 10.0, 0.0).is_err());
    /// ```
    pub fn new(tension: f32, friction: f32, mass: f32) -> Result<Self, MotionError> {
        MotionError::require(
            tension.is_finite() && tension >= 0.0,
            MotionErrorCode::SpringUnstable,
            "tension must be finite and >= 0",
        )?;
        MotionError::require(
            friction.is_finite() && friction >= 0.0,
            MotionErrorCode::SpringUnstable,
            "friction must be finite and >= 0",
        )?;
        MotionError::require(
            mass.is_finite() && mass > 0.0,
            MotionErrorCode::SpringUnstable,
            "mass must be finite and > 0",
        )?;
        Ok(Self {
            tension,
            friction,
            mass,
        })
    }

    /// Spring stiffness: the restoring force per unit of displacement from
    /// the target.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::SpringConfig;
    ///
    /// assert_eq!(SpringConfig::new(120.0, 14.0, 1.0)?.tension(), 120.0);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn tension(&self) -> f32 {
        self.tension
    }

    /// Damping: the drag force per unit of velocity.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::SpringConfig;
    ///
    /// assert_eq!(SpringConfig::new(120.0, 14.0, 1.0)?.friction(), 14.0);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn friction(&self) -> f32 {
        self.friction
    }

    /// Moving mass; always positive, so it is safe to divide by.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::SpringConfig;
    ///
    /// assert_eq!(SpringConfig::new(120.0, 14.0, 2.5)?.mass(), 2.5);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn mass(&self) -> f32 {
        self.mass
    }
}

/// Advance spring state by one integration step using RK4.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use drishti::motion::{SpringConfig, integrate_spring};
///
/// let config = SpringConfig::new(100.0, 10.0, 1.0)?;
/// // From rest at 0 with the target at 1, one step pulls position and
/// // velocity toward the target.
/// let (position, velocity) = integrate_spring(0.0, 0.0, 1.0, &config, 0.016);
/// assert!(position > 0.0 && velocity > 0.0);
///
/// // With no tension and no friction the spring coasts at constant velocity.
/// let free = SpringConfig::new(0.0, 0.0, 1.0)?;
/// let (coasted, speed) = integrate_spring(1.0, 2.0, 0.0, &free, 0.5);
/// assert!((coasted - 2.0).abs() < 1e-5);
/// assert!((speed - 2.0).abs() < 1e-5);
/// # Ok(())
/// # }
/// ```
#[must_use]
pub fn integrate_spring(
    position: f32,
    velocity: f32,
    target: f32,
    config: &SpringConfig,
    dt_seconds: f32,
) -> (f32, f32) {
    let acceleration = |pos: f32, vel: f32| -> f32 {
        (-config.tension * (pos - target) - config.friction * vel) / config.mass
    };

    // RK4 coefficients for position and velocity.
    let vel_k1 = acceleration(position, velocity);
    let pos_k1 = velocity;

    let vel_k2 = acceleration(
        position + 0.5 * dt_seconds * pos_k1,
        velocity + 0.5 * dt_seconds * vel_k1,
    );
    let pos_k2 = velocity + 0.5 * dt_seconds * vel_k1;

    let vel_k3 = acceleration(
        position + 0.5 * dt_seconds * pos_k2,
        velocity + 0.5 * dt_seconds * vel_k2,
    );
    let pos_k3 = velocity + 0.5 * dt_seconds * vel_k2;

    let vel_k4 = acceleration(
        position + dt_seconds * pos_k3,
        velocity + dt_seconds * vel_k3,
    );
    let pos_k4 = velocity + dt_seconds * vel_k3;

    let new_position =
        position + (dt_seconds / 6.0) * (pos_k1 + 2.0 * pos_k2 + 2.0 * pos_k3 + pos_k4);
    let new_velocity =
        velocity + (dt_seconds / 6.0) * (vel_k1 + 2.0 * vel_k2 + 2.0 * vel_k3 + vel_k4);

    (new_position, new_velocity)
}
