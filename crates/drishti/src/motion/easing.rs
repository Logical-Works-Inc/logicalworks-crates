//! Easing registry and cubic-bezier evaluator.
//!
//! Absorbed from the kernel motion core. `libm` backs the transcendental
//! functions so output is identical across architectures. Named curves follow
//! Robert Penner's easing equations; `time` is normalized time on [0, 1] and
//! `parameter` is the cubic-bezier curve parameter solved by Newton-Raphson.

use lgwks_deps::libm;

use super::error::{MotionError, MotionErrorCode};

/// Sized easing discriminant — no trait objects for the primitive.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use drishti::motion::{Easing, NamedCurve};
///
/// let named = Easing::named(NamedCurve::Linear);
/// assert_eq!(named, Easing::Named(NamedCurve::Linear));
/// let custom = Easing::cubic_bezier(0.25, 0.1, 0.25, 1.0)?;
/// assert!(matches!(custom, Easing::CubicBezier { .. }));
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Easing {
    /// A named Penner curve.
    Named(NamedCurve),
    /// Custom cubic-bezier: p1x/p2x must be in [0, 1]; y may overshoot.
    CubicBezier {
        /// First control point x, in [0, 1].
        p1x: f32,
        /// First control point y, unbounded.
        p1y: f32,
        /// Second control point x, in [0, 1].
        p2x: f32,
        /// Second control point y, unbounded.
        p2y: f32,
    },
}

/// The 27 named easing curves, plus instant.
///
/// # Examples
///
/// ```
/// use drishti::motion::{Easing, NamedCurve};
///
/// // Quadratic ease-in squares its input; the step curve holds 0 until time 1.
/// assert_eq!(Easing::named(NamedCurve::QuadIn).value_at(0.5), 0.25);
/// assert_eq!(Easing::named(NamedCurve::Instant).value_at(0.99), 0.0);
/// assert_eq!(Easing::named(NamedCurve::Instant).value_at(1.0), 1.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NamedCurve {
    /// Step to 1 at exactly `time == 1`.
    Instant,
    /// Linear interpolation.
    Linear,
    /// Quadratic ease-in.
    QuadIn,
    /// Quadratic ease-out.
    QuadOut,
    /// Quadratic ease-in-out.
    QuadInOut,
    /// Cubic ease-in.
    CubicIn,
    /// Cubic ease-out.
    CubicOut,
    /// Cubic ease-in-out.
    CubicInOut,
    /// Quartic ease-in.
    QuartIn,
    /// Quartic ease-out.
    QuartOut,
    /// Quartic ease-in-out.
    QuartInOut,
    /// Quintic ease-in.
    QuintIn,
    /// Quintic ease-out.
    QuintOut,
    /// Quintic ease-in-out.
    QuintInOut,
    /// Sinusoidal ease-in.
    SineIn,
    /// Sinusoidal ease-out.
    SineOut,
    /// Sinusoidal ease-in-out.
    SineInOut,
    /// Exponential ease-in.
    ExpoIn,
    /// Exponential ease-out.
    ExpoOut,
    /// Exponential ease-in-out.
    ExpoInOut,
    /// Circular ease-in.
    CircIn,
    /// Circular ease-out.
    CircOut,
    /// Circular ease-in-out.
    CircInOut,
    /// Back ease-in (slight overshoot).
    BackIn,
    /// Back ease-out (slight overshoot).
    BackOut,
    /// Back ease-in-out (slight overshoot).
    BackInOut,
}

impl Easing {
    /// Construct a named easing curve. Infallible: every [`NamedCurve`] is a
    /// closed-form function, so there is no domain to reject.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{Easing, NamedCurve};
    ///
    /// let easing = Easing::named(NamedCurve::CubicIn);
    /// assert_eq!(easing.value_at(0.5), 0.125);
    /// ```
    #[must_use]
    pub const fn named(curve: NamedCurve) -> Self {
        Self::Named(curve)
    }

    /// Construct a cubic-bezier easing; rejects x-coords outside [0, 1].
    ///
    /// # Errors
    ///
    /// Returns [`MotionErrorCode::EasingDomainError`]
    /// when either x control point leaves [0, 1].
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{Easing, MotionErrorCode};
    ///
    /// // The identity curve keeps the midpoint at the midpoint.
    /// let identity = Easing::cubic_bezier(0.0, 0.0, 1.0, 1.0)?;
    /// assert!((identity.value_at(0.5) - 0.5).abs() < 1e-6);
    ///
    /// let refused = Easing::cubic_bezier(1.5, 0.0, 0.5, 1.0);
    /// assert_eq!(
    ///     refused.err().map(|error| error.code()),
    ///     Some(MotionErrorCode::EasingDomainError)
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub fn cubic_bezier(p1x: f32, p1y: f32, p2x: f32, p2y: f32) -> Result<Self, MotionError> {
        let x_in_domain = (0.0..=1.0).contains(&p1x) && (0.0..=1.0).contains(&p2x);
        MotionError::require(
            x_in_domain,
            MotionErrorCode::EasingDomainError,
            "cubic-bezier x control points must lie in [0, 1]",
        )?;
        Ok(Self::CubicBezier { p1x, p1y, p2x, p2y })
    }

    /// Sample the easing at normalized time; clamps the input to [0, 1].
    /// Named `value_at` for the [`crate::motion::TweenComponent::value_at`]
    /// vocabulary (the kernel called this `eval`; the behavior is identical).
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{Easing, NamedCurve};
    ///
    /// let linear = Easing::named(NamedCurve::Linear);
    /// assert_eq!(linear.value_at(-1.0), 0.0);
    /// assert_eq!(linear.value_at(0.25), 0.25);
    /// assert_eq!(linear.value_at(2.0), 1.0);
    /// ```
    #[must_use]
    pub fn value_at(&self, time_normalized: f32) -> f32 {
        let clamped = time_normalized.clamp(0.0, 1.0);
        match *self {
            Self::Named(curve) => eval_named(curve, clamped),
            Self::CubicBezier { p1x, p1y, p2x, p2y } => {
                eval_cubic_bezier(p1x, p1y, p2x, p2y, clamped)
            }
        }
    }
}

/// Evaluate one named curve at clamped normalized time.
fn eval_named(curve: NamedCurve, time: f32) -> f32 {
    const BACK_OVERSHOOT: f32 = 1.70158;
    match curve {
        NamedCurve::Instant => {
            if time < 1.0 {
                0.0
            } else {
                1.0
            }
        }
        NamedCurve::Linear => time,
        NamedCurve::QuadIn => time * time,
        NamedCurve::QuadOut => 1.0 - (1.0 - time) * (1.0 - time),
        NamedCurve::QuadInOut => smooth_power_in_out(time, 2.0),
        NamedCurve::CubicIn => time * time * time,
        NamedCurve::CubicOut => 1.0 - (1.0 - time) * (1.0 - time) * (1.0 - time),
        NamedCurve::CubicInOut => smooth_power_in_out(time, 3.0),
        NamedCurve::QuartIn => time * time * time * time,
        NamedCurve::QuartOut => 1.0 - pow4(1.0 - time),
        NamedCurve::QuartInOut => smooth_power_in_out(time, 4.0),
        NamedCurve::QuintIn => time * time * time * time * time,
        NamedCurve::QuintOut => 1.0 - pow5(1.0 - time),
        NamedCurve::QuintInOut => smooth_power_in_out(time, 5.0),
        NamedCurve::SineIn => 1.0 - libm::cosf(time * core::f32::consts::FRAC_PI_2),
        NamedCurve::SineOut => libm::sinf(time * core::f32::consts::FRAC_PI_2),
        NamedCurve::SineInOut => -(libm::cosf(core::f32::consts::PI * time) - 1.0) / 2.0,
        NamedCurve::ExpoIn => {
            if time <= 0.0 {
                0.0
            } else {
                libm::powf(2.0, 10.0 * time - 10.0)
            }
        }
        NamedCurve::ExpoOut => {
            if time >= 1.0 {
                1.0
            } else {
                1.0 - libm::powf(2.0, -10.0 * time)
            }
        }
        NamedCurve::ExpoInOut => smooth_expo_in_out(time),
        NamedCurve::CircIn => 1.0 - libm::sqrtf(1.0 - time * time),
        NamedCurve::CircOut => libm::sqrtf(1.0 - (time - 1.0) * (time - 1.0)),
        NamedCurve::CircInOut => smooth_circ_in_out(time),
        NamedCurve::BackIn => time * time * ((BACK_OVERSHOOT + 1.0) * time - BACK_OVERSHOOT),
        NamedCurve::BackOut => {
            let shifted = time - 1.0;
            shifted * shifted * ((BACK_OVERSHOOT + 1.0) * shifted + BACK_OVERSHOOT) + 1.0
        }
        NamedCurve::BackInOut => smooth_back_in_out(time, BACK_OVERSHOOT),
    }
}

/// Symmetric power ease-in-out.
fn smooth_power_in_out(time: f32, power: f32) -> f32 {
    if time < 0.5 {
        libm::powf(2.0 * time, power) / 2.0
    } else {
        1.0 - libm::powf(-2.0 * time + 2.0, power) / 2.0
    }
}

/// Exponential ease-in-out with exact endpoints.
fn smooth_expo_in_out(time: f32) -> f32 {
    if time <= 0.0 {
        return 0.0;
    }
    if time >= 1.0 {
        return 1.0;
    }
    if time < 0.5 {
        libm::powf(2.0, 20.0 * time - 10.0) / 2.0
    } else {
        (2.0 - libm::powf(2.0, -20.0 * time + 10.0)) / 2.0
    }
}

/// Circular ease-in-out.
fn smooth_circ_in_out(time: f32) -> f32 {
    if time < 0.5 {
        (1.0 - libm::sqrtf(1.0 - 4.0 * time * time)) / 2.0
    } else {
        (libm::sqrtf(1.0 - (-2.0 * time + 2.0) * (-2.0 * time + 2.0)) + 1.0) / 2.0
    }
}

/// Back ease-in-out with the standard 1.525-scaled overshoot.
fn smooth_back_in_out(time: f32, overshoot: f32) -> f32 {
    let overshoot_scaled = overshoot * 1.525;
    if time < 0.5 {
        let scaled = 2.0 * time;
        scaled * scaled * ((overshoot_scaled + 1.0) * scaled - overshoot_scaled) / 2.0
    } else {
        let scaled = 2.0 * time - 2.0;
        (scaled * scaled * ((overshoot_scaled + 1.0) * scaled + overshoot_scaled) + 2.0) / 2.0
    }
}

/// Fourth power without a transcendental call.
fn pow4(value: f32) -> f32 {
    value * value * value * value
}

/// Fifth power without a transcendental call.
fn pow5(value: f32) -> f32 {
    value * value * value * value * value
}

/// Newton-Raphson: solve for the parameter where x(parameter) = time, return
/// y(parameter). Eight iterations reach 1e-7 accuracy on realistic controls.
fn eval_cubic_bezier(p1x: f32, p1y: f32, p2x: f32, p2y: f32, time: f32) -> f32 {
    let mut parameter = time;
    for _ in 0..8 {
        let position = bezier_value(p1x, p2x, parameter);
        let slope = bezier_derivative(p1x, p2x, parameter);
        if slope.abs() < 1e-6 {
            break;
        }
        parameter -= (position - time) / slope;
    }
    bezier_value(p1y, p2y, parameter.clamp(0.0, 1.0))
}

/// Cubic-bezier position for one axis at the curve parameter.
fn bezier_value(p1: f32, p2: f32, time: f32) -> f32 {
    let mt = 1.0 - time;
    3.0 * mt * mt * time * p1 + 3.0 * mt * time * time * p2 + time * time * time
}

/// Cubic-bezier derivative for one axis at the curve parameter.
fn bezier_derivative(p1: f32, p2: f32, time: f32) -> f32 {
    let mt = 1.0 - time;
    3.0 * mt * mt * p1 + 6.0 * mt * time * (p2 - p1) + 3.0 * time * time * (1.0 - p2)
}
