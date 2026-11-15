//! Two- and three-component vector interpolation.
//!
//! The kernel used `glam` for these two functions only; the arithmetic below
//! is the same component-wise single-lerp formula, so results are identical
//! without the dependency.

/// A two-component vector.
///
/// Fields are private: construction runs through [`Vec2::new`] and reads go
/// through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::vec::Vec2;
///
/// let vector = Vec2::new(1.0, 2.0);
/// assert_eq!((vector.x(), vector.y()), (1.0, 2.0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Vec2 {
    /// X component.
    x: f32,
    /// Y component.
    y: f32,
}

impl Vec2 {
    /// Assemble a two-component vector from its x and y components.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::vec::Vec2;
    ///
    /// assert_eq!(Vec2::new(1.0, 2.0), Vec2::new(1.0, 2.0));
    /// ```
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// First component, as given to [`Vec2::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::vec::Vec2;
    ///
    /// assert_eq!(Vec2::new(1.0, 2.0).x(), 1.0);
    /// ```
    #[must_use]
    pub const fn x(&self) -> f32 {
        self.x
    }

    /// Second component, as given to [`Vec2::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::vec::Vec2;
    ///
    /// assert_eq!(Vec2::new(1.0, 2.0).y(), 2.0);
    /// ```
    #[must_use]
    pub const fn y(&self) -> f32 {
        self.y
    }
}

/// A three-component vector.
///
/// Fields are private: construction runs through [`Vec3::new`] and reads go
/// through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::vec::Vec3;
///
/// let vector = Vec3::new(1.0, 2.0, 3.0);
/// assert_eq!((vector.x(), vector.y(), vector.z()), (1.0, 2.0, 3.0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Vec3 {
    /// X component.
    x: f32,
    /// Y component.
    y: f32,
    /// Z component.
    z: f32,
}

impl Vec3 {
    /// Assemble a three-component vector from its x, y and z components.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::vec::Vec3;
    ///
    /// assert_eq!(Vec3::new(1.0, 2.0, 3.0), Vec3::new(1.0, 2.0, 3.0));
    /// ```
    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// First component, as given to [`Vec3::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::vec::Vec3;
    ///
    /// assert_eq!(Vec3::new(1.0, 2.0, 3.0).x(), 1.0);
    /// ```
    #[must_use]
    pub const fn x(&self) -> f32 {
        self.x
    }

    /// Second component, as given to [`Vec3::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::vec::Vec3;
    ///
    /// assert_eq!(Vec3::new(1.0, 2.0, 3.0).y(), 2.0);
    /// ```
    #[must_use]
    pub const fn y(&self) -> f32 {
        self.y
    }

    /// Third component, as given to [`Vec3::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::vec::Vec3;
    ///
    /// assert_eq!(Vec3::new(1.0, 2.0, 3.0).z(), 3.0);
    /// ```
    #[must_use]
    pub const fn z(&self) -> f32 {
        self.z
    }
}

/// Linear interpolation of two `Vec2` values, component by component. `mix`
/// is not clamped.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::vec::{Vec2, lerp_vec2};
///
/// let mixed = lerp_vec2(Vec2::new(0.0, 0.0), Vec2::new(2.0, 4.0), 0.5);
/// assert_eq!(mixed, Vec2::new(1.0, 2.0));
/// ```
#[must_use]
pub const fn lerp_vec2(from: Vec2, to: Vec2, mix: f32) -> Vec2 {
    Vec2::new(
        from.x + (to.x - from.x) * mix,
        from.y + (to.y - from.y) * mix,
    )
}

/// Linear interpolation of two `Vec3` values, component by component. `mix`
/// is not clamped.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::vec::{Vec3, lerp_vec3};
///
/// let mixed = lerp_vec3(Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 4.0, 6.0), 0.5);
/// assert_eq!(mixed, Vec3::new(1.0, 2.0, 3.0));
/// ```
#[must_use]
pub const fn lerp_vec3(from: Vec3, to: Vec3, mix: f32) -> Vec3 {
    Vec3::new(
        from.x + (to.x - from.x) * mix,
        from.y + (to.y - from.y) * mix,
        from.z + (to.z - from.z) * mix,
    )
}
