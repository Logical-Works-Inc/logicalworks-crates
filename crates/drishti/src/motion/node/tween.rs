//! Tween component: pure value-at-time computation.
//!
//! `value_at` is a function of (from, to, time, easing) only — no mutable
//! state. Tweens are immutable after construction and replaced atomically via
//! [`crate::motion::apply_patch`].

use super::super::easing::Easing;
use super::super::error::{MotionError, MotionErrorCode};
use super::super::interpolate::scalar::lerp_scalar;

/// Animatable scalar value.
///
/// The field is private: construction runs through [`AnimValue::new`] and
/// reads through [`AnimValue::value`].
///
/// # Examples
///
/// ```
/// use drishti::motion::AnimValue;
///
/// assert_eq!(AnimValue::new(0.25).value(), 0.25);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct AnimValue(f32);

impl AnimValue {
    /// Wrap a scalar value. No range check: NaN and infinities are wrapped
    /// as given.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::AnimValue;
    ///
    /// assert_eq!(AnimValue::new(-3.5), AnimValue::new(-3.5));
    /// ```
    #[must_use]
    pub const fn new(value: f32) -> Self {
        Self(value)
    }

    /// Unwrap back to the plain `f32` passed to [`AnimValue::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::AnimValue;
    ///
    /// assert_eq!(AnimValue::new(7.0).value(), 7.0);
    /// ```
    #[must_use]
    pub const fn value(self) -> f32 {
        self.0
    }
}

/// Immutable-after-construction tween.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use drishti::motion::{Easing, NamedCurve, TweenComponent};
///
/// let tween = TweenComponent::new(0.0, 10.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
/// assert_eq!(tween.value_at_wall_ms(50.0), 5.0);
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct TweenComponent {
    /// Start value.
    from: AnimValue,
    /// End value.
    to: AnimValue,
    /// Duration in milliseconds, always positive.
    duration_ms: f32,
    /// Start time in milliseconds.
    start_ms: f32,
    /// Easing applied to normalized time.
    easing: Easing,
}

impl TweenComponent {
    /// Construct a tween, validating that every field is finite and
    /// `duration_ms` is positive.
    ///
    /// # Errors
    ///
    /// Returns [`MotionErrorCode::PatchInvalid`]
    /// for a non-finite field or a non-positive duration.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{Easing, MotionErrorCode, NamedCurve, TweenComponent};
    ///
    /// let linear = Easing::named(NamedCurve::Linear);
    /// assert!(TweenComponent::new(0.0, 1.0, 0.0, 100.0, linear.clone()).is_ok());
    ///
    /// let zero_duration = TweenComponent::new(0.0, 1.0, 0.0, 0.0, linear.clone());
    /// assert_eq!(
    ///     zero_duration.err().map(|error| error.code()),
    ///     Some(MotionErrorCode::PatchInvalid)
    /// );
    /// assert!(TweenComponent::new(f32::NAN, 1.0, 0.0, 100.0, linear).is_err());
    /// ```
    pub fn new(
        from: f32,
        to: f32,
        start_ms: f32,
        duration_ms: f32,
        easing: Easing,
    ) -> Result<Self, MotionError> {
        MotionError::require(
            duration_ms.is_finite() && duration_ms > 0.0,
            MotionErrorCode::PatchInvalid,
            "duration_ms must be a finite positive number",
        )?;
        MotionError::require(
            from.is_finite(),
            MotionErrorCode::PatchInvalid,
            "from must be a finite number",
        )?;
        MotionError::require(
            to.is_finite(),
            MotionErrorCode::PatchInvalid,
            "to must be a finite number",
        )?;
        MotionError::require(
            start_ms.is_finite(),
            MotionErrorCode::PatchInvalid,
            "start_ms must be a finite number",
        )?;
        Ok(Self {
            from: AnimValue::new(from),
            to: AnimValue::new(to),
            duration_ms,
            start_ms,
            easing,
        })
    }

    /// Value the tween holds before its start time and leaves from.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{Easing, NamedCurve, TweenComponent};
    ///
    /// let tween = TweenComponent::new(2.0, 8.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
    /// assert_eq!(tween.from_value(), 2.0);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn from_value(&self) -> f32 {
        self.from.0
    }

    /// Value the tween reaches and holds after its end time.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{Easing, NamedCurve, TweenComponent};
    ///
    /// let tween = TweenComponent::new(2.0, 8.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
    /// assert_eq!(tween.to_value(), 8.0);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn to_value(&self) -> f32 {
        self.to.0
    }

    /// Timeline instant, in milliseconds, at which the tween begins.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{Easing, NamedCurve, TweenComponent};
    ///
    /// let tween = TweenComponent::new(0.0, 1.0, 20.0, 100.0, Easing::named(NamedCurve::Linear))?;
    /// assert_eq!(tween.start_ms(), 20.0);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn start_ms(&self) -> f32 {
        self.start_ms
    }

    /// Length of the tween in milliseconds; construction guarantees it is
    /// finite and positive.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{Easing, NamedCurve, TweenComponent};
    ///
    /// let tween = TweenComponent::new(0.0, 1.0, 0.0, 250.0, Easing::named(NamedCurve::Linear))?;
    /// assert_eq!(tween.duration_ms(), 250.0);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn duration_ms(&self) -> f32 {
        self.duration_ms
    }

    /// Curve that shapes normalized time between start and end.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{Easing, NamedCurve, TweenComponent};
    ///
    /// let easing = Easing::named(NamedCurve::QuadIn);
    /// let tween = TweenComponent::new(0.0, 1.0, 0.0, 100.0, easing.clone())?;
    /// assert_eq!(tween.easing(), &easing);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn easing(&self) -> &Easing {
        &self.easing
    }

    /// Evaluate the tween at global timeline time `wall_ms`. Times before the
    /// start hold `from`; times past the end hold `to`.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{Easing, NamedCurve, TweenComponent};
    ///
    /// let tween = TweenComponent::new(0.0, 10.0, 100.0, 100.0, Easing::named(NamedCurve::Linear))?;
    /// assert_eq!(tween.value_at_wall_ms(0.0), 0.0);
    /// assert_eq!(tween.value_at_wall_ms(150.0), 5.0);
    /// assert_eq!(tween.value_at_wall_ms(500.0), 10.0);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn value_at_wall_ms(&self, wall_ms: f32) -> f32 {
        let local = (wall_ms - self.start_ms) / self.duration_ms;
        self.value_at(local.clamp(0.0, 1.0))
    }

    /// Evaluate at normalized time in [0, 1].
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{Easing, NamedCurve, TweenComponent};
    ///
    /// let tween = TweenComponent::new(0.0, 10.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
    /// assert_eq!(tween.value_at(0.25), 2.5);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn value_at(&self, mix: f32) -> f32 {
        let eased = self.easing.value_at(mix);
        lerp_scalar(self.from.0, self.to.0, eased)
    }
}
