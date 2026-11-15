//! SVG path morphing: interpolate between two point sequences of equal length.

use super::scalar::lerp_scalar;

/// A single 2D point.
///
/// Fields are private: construction runs through [`Point2::new`] and reads go
/// through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::path_morph::Point2;
///
/// let point = Point2::new(1.0, 2.0);
/// assert_eq!((point.x(), point.y()), (1.0, 2.0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Point2 {
    /// X coordinate.
    x: f32,
    /// Y coordinate.
    y: f32,
}

impl Point2 {
    /// Assemble a 2D point from its horizontal and vertical coordinates.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::path_morph::Point2;
    ///
    /// assert_eq!(Point2::new(3.0, 4.0), Point2::new(3.0, 4.0));
    /// ```
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Horizontal coordinate, increasing to the right.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::path_morph::Point2;
    ///
    /// assert_eq!(Point2::new(3.0, 4.0).x(), 3.0);
    /// ```
    #[must_use]
    pub const fn x(&self) -> f32 {
        self.x
    }

    /// Vertical coordinate, as stored; the point carries no axis convention.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::path_morph::Point2;
    ///
    /// assert_eq!(Point2::new(3.0, 4.0).y(), 4.0);
    /// ```
    #[must_use]
    pub const fn y(&self) -> f32 {
        self.y
    }
}

/// Interpolate two equal-length point sequences into `out`.
///
/// The three slices must share a length; a mismatch leaves `out` holding the
/// prefix that shares indices and reports it through the returned count.
/// Callers that need the kernel's debug assertion instead check lengths first.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::path_morph::{Point2, interp_path};
///
/// let from = [Point2::new(0.0, 0.0)];
/// let to = [Point2::new(2.0, 4.0)];
/// let mut out = [Point2::new(0.0, 0.0)];
/// assert_eq!(interp_path(&from, &to, 0.5, &mut out), 1);
/// assert_eq!(out[0], Point2::new(1.0, 2.0));
///
/// // A shorter `to` stops the walk at the shared prefix.
/// assert_eq!(interp_path(&from, &[], 0.5, &mut out), 0);
/// ```
pub fn interp_path(from: &[Point2], to: &[Point2], mix: f32, out: &mut [Point2]) -> usize {
    let mut written = 0_usize;
    for (index, from_point) in from.iter().enumerate() {
        let Some(to_point) = to.get(index) else {
            break;
        };
        let Some(slot) = out.get_mut(index) else {
            break;
        };
        *slot = Point2::new(
            lerp_scalar(from_point.x, to_point.x, mix),
            lerp_scalar(from_point.y, to_point.y, mix),
        );
        written = written.saturating_add(1);
    }
    written
}
