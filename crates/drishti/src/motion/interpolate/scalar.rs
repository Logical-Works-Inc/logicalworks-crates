//! Scalar interpolation.
//!
//! Uses the two-lerp formula so endpoints are exact at mix 0 and 1 regardless
//! of the magnitude of `from` and `to`. The single-lerp
//! `from + (to - from) * mix` can drift at extreme values due to
//! floating-point cancellation. `mix` is NOT clamped; callers pass a
//! pre-evaluated easing output.

/// Linear interpolation between two scalars, exact at both endpoints.
///
/// # Examples
///
/// ```
/// use drishti::motion::lerp_scalar;
///
/// assert_eq!(lerp_scalar(0.0, 10.0, 0.25), 2.5);
/// assert_eq!(lerp_scalar(3.0, 9.0, 0.0), 3.0);
/// assert_eq!(lerp_scalar(3.0, 9.0, 1.0), 9.0);
/// ```
#[must_use]
pub const fn lerp_scalar(from: f32, to: f32, mix: f32) -> f32 {
    (1.0 - mix) * from + mix * to
}
