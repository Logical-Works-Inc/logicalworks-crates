//! 2D affine matrix interpolation via component decompose/recompose.

use super::scalar::lerp_scalar;

/// Decomposed 2D transform.
///
/// Fields are private: construction runs through [`Transform2D::new`] and
/// reads go through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::matrix::Transform2D;
///
/// let transform = Transform2D::new(10.0, 20.0, 2.0, 3.0, 0.5);
/// assert_eq!(transform.translate_x(), 10.0);
/// assert_eq!(transform.rotation(), 0.5);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Transform2D {
    /// Translation x.
    translate_x: f32,
    /// Translation y.
    translate_y: f32,
    /// Scale x.
    scale_x: f32,
    /// Scale y.
    scale_y: f32,
    /// Rotation in radians.
    rotation: f32,
}

impl Transform2D {
    /// Assemble a decomposed 2D transform. The channels are independent and
    /// unvalidated; a zero or negative scale is stored as given.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::matrix::Transform2D;
    ///
    /// let identity = Transform2D::new(0.0, 0.0, 1.0, 1.0, 0.0);
    /// assert_eq!(identity.scale_x(), 1.0);
    /// ```
    #[must_use]
    pub const fn new(
        translate_x: f32,
        translate_y: f32,
        scale_x: f32,
        scale_y: f32,
        rotation: f32,
    ) -> Self {
        Self {
            translate_x,
            translate_y,
            scale_x,
            scale_y,
            rotation,
        }
    }

    /// Horizontal offset applied after scale and rotation.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::matrix::Transform2D;
    ///
    /// assert_eq!(Transform2D::new(10.0, 20.0, 2.0, 3.0, 0.5).translate_x(), 10.0);
    /// ```
    #[must_use]
    pub const fn translate_x(&self) -> f32 {
        self.translate_x
    }

    /// Vertical offset applied after scale and rotation.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::matrix::Transform2D;
    ///
    /// assert_eq!(Transform2D::new(10.0, 20.0, 2.0, 3.0, 0.5).translate_y(), 20.0);
    /// ```
    #[must_use]
    pub const fn translate_y(&self) -> f32 {
        self.translate_y
    }

    /// Horizontal stretch factor; 1.0 leaves the width unchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::matrix::Transform2D;
    ///
    /// assert_eq!(Transform2D::new(10.0, 20.0, 2.0, 3.0, 0.5).scale_x(), 2.0);
    /// ```
    #[must_use]
    pub const fn scale_x(&self) -> f32 {
        self.scale_x
    }

    /// Vertical stretch factor; 1.0 leaves the height unchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::matrix::Transform2D;
    ///
    /// assert_eq!(Transform2D::new(10.0, 20.0, 2.0, 3.0, 0.5).scale_y(), 3.0);
    /// ```
    #[must_use]
    pub const fn scale_y(&self) -> f32 {
        self.scale_y
    }

    /// Rotation in radians, unnormalised: 2π and 0 are different values here.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::matrix::Transform2D;
    ///
    /// assert_eq!(Transform2D::new(10.0, 20.0, 2.0, 3.0, 0.5).rotation(), 0.5);
    /// ```
    #[must_use]
    pub const fn rotation(&self) -> f32 {
        self.rotation
    }
}

/// Blend `from` into `to` at `mix`, lerping each decomposed channel
/// INDEPENDENTLY. This is deliberately not matrix interpolation: decomposing
/// first keeps scale from shearing, which a naive element-wise lerp of two
/// 2x3 matrices does not.
///
/// Rotation is a caveat, not a feature: it lerps as a plain scalar in
/// radians with no angle normalization or shortest-arc selection, so a pair
/// straddling the wrap point takes the long way around. Callers that need
/// shortest-arc must pre-normalize `to.rotation` into the same turn first.
///
/// `mix` is not clamped: values outside `0.0..=1.0` extrapolate.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::matrix::{Transform2D, interp_transform};
///
/// let from = Transform2D::new(0.0, 0.0, 1.0, 1.0, 0.0);
/// let to = Transform2D::new(10.0, 20.0, 2.0, 3.0, 2.0);
/// let mixed = interp_transform(from, to, 0.5);
/// assert_eq!(mixed, Transform2D::new(5.0, 10.0, 1.5, 2.0, 1.0));
/// ```
#[must_use]
pub const fn interp_transform(from: Transform2D, to: Transform2D, mix: f32) -> Transform2D {
    Transform2D::new(
        lerp_scalar(from.translate_x, to.translate_x, mix),
        lerp_scalar(from.translate_y, to.translate_y, mix),
        lerp_scalar(from.scale_x, to.scale_x, mix),
        lerp_scalar(from.scale_y, to.scale_y, mix),
        lerp_scalar(from.rotation, to.rotation, mix),
    )
}
