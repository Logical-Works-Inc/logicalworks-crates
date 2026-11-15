//! CSS `clip-path: inset(...)` interpolation.

use super::scalar::lerp_scalar;

/// Four-sided inset in CSS units (top, right, bottom, left).
///
/// Fields are private: construction runs through [`Inset::new`] and reads go
/// through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::clip_path::Inset;
///
/// let inset = Inset::new(1.0, 2.0, 3.0, 4.0);
/// assert_eq!((inset.top(), inset.right(), inset.bottom(), inset.left()), (1.0, 2.0, 3.0, 4.0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Inset {
    /// Top inset.
    top: f32,
    /// Right inset.
    right: f32,
    /// Bottom inset.
    bottom: f32,
    /// Left inset.
    left: f32,
}

impl Inset {
    /// Assemble a four-sided inset in CSS argument order (top, right, bottom,
    /// left). Values are stored as given; negative or non-finite sides are not
    /// rejected.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::clip_path::Inset;
    ///
    /// let inset = Inset::new(0.0, 8.0, 0.0, 8.0);
    /// assert_eq!(inset.right(), 8.0);
    /// ```
    #[must_use]
    pub const fn new(top: f32, right: f32, bottom: f32, left: f32) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// Distance the clip edge sits below the top of the box.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::clip_path::Inset;
    ///
    /// assert_eq!(Inset::new(1.0, 2.0, 3.0, 4.0).top(), 1.0);
    /// ```
    #[must_use]
    pub const fn top(&self) -> f32 {
        self.top
    }

    /// Distance the clip edge sits inside the right of the box.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::clip_path::Inset;
    ///
    /// assert_eq!(Inset::new(1.0, 2.0, 3.0, 4.0).right(), 2.0);
    /// ```
    #[must_use]
    pub const fn right(&self) -> f32 {
        self.right
    }

    /// Distance the clip edge sits above the bottom of the box.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::clip_path::Inset;
    ///
    /// assert_eq!(Inset::new(1.0, 2.0, 3.0, 4.0).bottom(), 3.0);
    /// ```
    #[must_use]
    pub const fn bottom(&self) -> f32 {
        self.bottom
    }

    /// Distance the clip edge sits right of the left of the box.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::clip_path::Inset;
    ///
    /// assert_eq!(Inset::new(1.0, 2.0, 3.0, 4.0).left(), 4.0);
    /// ```
    #[must_use]
    pub const fn left(&self) -> f32 {
        self.left
    }
}

/// Interpolate two inset clip-path values, side by side. `mix` is not
/// clamped, so values outside `0.0..=1.0` extrapolate.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::clip_path::{Inset, interp_inset};
///
/// let mixed = interp_inset(Inset::new(0.0, 0.0, 0.0, 0.0), Inset::new(8.0, 8.0, 8.0, 8.0), 0.5);
/// assert_eq!(mixed, Inset::new(4.0, 4.0, 4.0, 4.0));
/// ```
#[must_use]
pub const fn interp_inset(from: Inset, to: Inset, mix: f32) -> Inset {
    Inset::new(
        lerp_scalar(from.top, to.top, mix),
        lerp_scalar(from.right, to.right, mix),
        lerp_scalar(from.bottom, to.bottom, mix),
        lerp_scalar(from.left, to.left, mix),
    )
}
