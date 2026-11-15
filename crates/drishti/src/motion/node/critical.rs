//! Analytic, bounded critically-damped motion for interruptible presentation.
//!
//! Absorbed from the kernel motion core. This does not change the legacy RK4
//! integrator and claims no stability result for other damping regimes.
//! Position and velocity remain continuous when the target changes.

use lgwks_deps::libm;

use super::super::error::{MotionError, MotionErrorCode};

/// Critically-damped spring with a closed-form step.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use drishti::motion::node::critical::CriticalSpring;
///
/// let mut spring = CriticalSpring::new(0.0, 0.5)?;
/// spring.retarget(1.0)?;
/// spring.advance(0.1)?;
/// // Moving toward the target, not yet there.
/// assert!(spring.position() > 0.0 && spring.position() < 1.0);
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct CriticalSpring {
    /// Current position.
    position: f64,
    /// Current velocity.
    velocity: f64,
    /// Current target.
    target: f64,
    /// Angular frequency, derived from the response time.
    omega: f64,
}

impl CriticalSpring {
    /// Build a spring at `position` with the given response time.
    ///
    /// `response_seconds` is a tuning parameter, not a guaranteed settling
    /// deadline.
    ///
    /// # Errors
    ///
    /// Returns [`MotionErrorCode::SpringUnstable`]
    /// for a non-finite position, a position beyond 1e9 in magnitude, or a
    /// response outside [0.016, 10] seconds.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::MotionErrorCode;
    /// use drishti::motion::node::critical::CriticalSpring;
    ///
    /// let spring = CriticalSpring::new(2.0, 0.5)?;
    /// assert_eq!(spring.target(), 2.0);
    ///
    /// let refused = CriticalSpring::new(f64::NAN, 0.5);
    /// assert_eq!(
    ///     refused.err().map(|error| error.code()),
    ///     Some(MotionErrorCode::SpringUnstable)
    /// );
    /// assert!(CriticalSpring::new(0.0, 0.001).is_err());
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(position: f64, response_seconds: f64) -> Result<Self, MotionError> {
        let domain_ok = position.is_finite()
            && position.abs() <= 1.0e9
            && response_seconds.is_finite()
            && (0.016..=10.0).contains(&response_seconds);
        MotionError::require(
            domain_ok,
            MotionErrorCode::SpringUnstable,
            "finite position and response in [0.016, 10] seconds required",
        )?;
        Ok(Self {
            position,
            velocity: 0.0,
            target: position,
            omega: 6.0 / response_seconds,
        })
    }

    /// Where the spring sits now. A new spring starts at rest on its target.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::node::critical::CriticalSpring;
    ///
    /// assert_eq!(CriticalSpring::new(2.0, 0.5)?.position(), 2.0);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn position(&self) -> f64 {
        self.position
    }

    /// Speed of the spring in position units per second; 0.0 at rest.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::node::critical::CriticalSpring;
    ///
    /// let mut spring = CriticalSpring::new(0.0, 0.5)?;
    /// assert_eq!(spring.velocity(), 0.0);
    /// spring.retarget(1.0)?;
    /// spring.advance(0.1)?;
    /// assert!(spring.velocity() > 0.0);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn velocity(&self) -> f64 {
        self.velocity
    }

    /// Position the spring is pulled toward; retargeting does not move the
    /// spring itself.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::node::critical::CriticalSpring;
    ///
    /// let mut spring = CriticalSpring::new(0.0, 0.5)?;
    /// spring.retarget(3.0)?;
    /// assert_eq!(spring.target(), 3.0);
    /// assert_eq!(spring.position(), 0.0);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn target(&self) -> f64 {
        self.target
    }

    /// Move the target, keeping position and velocity continuous. Invalid
    /// targets are rejected before mutation; momentum is retained.
    ///
    /// # Errors
    ///
    /// Returns [`MotionErrorCode::SpringUnstable`]
    /// for a non-finite target or one beyond 1e9 in magnitude.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::node::critical::CriticalSpring;
    ///
    /// let mut spring = CriticalSpring::new(0.0, 0.5)?;
    /// spring.retarget(1.0)?;
    /// assert!(spring.retarget(f64::INFINITY).is_err());
    /// // The refused call left the earlier target in place.
    /// assert_eq!(spring.target(), 1.0);
    /// # Ok(())
    /// # }
    /// ```
    pub fn retarget(&mut self, target: f64) -> Result<(), MotionError> {
        MotionError::require(
            target.is_finite() && target.abs() <= 1.0e9,
            MotionErrorCode::SpringUnstable,
            "target must be finite and within the presentation domain",
        )?;
        self.target = target;
        Ok(())
    }

    /// Exact solution for a constant target over this interval; no iterative
    /// substeps. Intervals beyond 60 seconds require an explicit host resume
    /// policy instead.
    ///
    /// # Errors
    ///
    /// Returns [`MotionErrorCode::SpringUnstable`]
    /// for non-finite elapsed time, elapsed time outside [0, 60] seconds, or
    /// a non-finite result.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::node::critical::CriticalSpring;
    ///
    /// let mut spring = CriticalSpring::new(0.0, 0.5)?;
    /// spring.retarget(1.0)?;
    /// spring.advance(0.1)?;
    /// assert!(spring.position() > 0.0);
    /// assert!(spring.advance(-1.0).is_err());
    /// assert!(spring.advance(61.0).is_err());
    /// # Ok(())
    /// # }
    /// ```
    pub fn advance(&mut self, seconds: f64) -> Result<(), MotionError> {
        MotionError::require(
            seconds.is_finite() && (0.0..=60.0).contains(&seconds),
            MotionErrorCode::SpringUnstable,
            "elapsed time must be finite and in [0, 60] seconds",
        )?;
        if seconds <= 0.0 {
            return Ok(());
        }
        let displacement = self.position - self.target;
        let coefficient = self.velocity + self.omega * displacement;
        let decay = libm::exp(-self.omega * seconds);
        let position = self.target + (displacement + coefficient * seconds) * decay;
        let velocity = (self.velocity - self.omega * coefficient * seconds) * decay;
        MotionError::require(
            position.is_finite() && velocity.is_finite(),
            MotionErrorCode::SpringUnstable,
            "motion evaluation overflowed",
        )?;
        self.position = position;
        self.velocity = velocity;
        Ok(())
    }

    /// True when the spring has visually settled on its target.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::node::critical::CriticalSpring;
    ///
    /// let mut spring = CriticalSpring::new(0.0, 0.5)?;
    /// assert!(spring.is_settled());
    /// spring.retarget(1.0)?;
    /// assert!(!spring.is_settled());
    /// spring.advance(10.0)?;
    /// assert!(spring.is_settled());
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn is_settled(&self) -> bool {
        (self.position - self.target).abs() < 1.0e-5 && self.velocity.abs() < 1.0e-4
    }

    /// Explicit discontinuity for reduced motion or resume; never an implicit
    /// fallback.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::node::critical::CriticalSpring;
    ///
    /// let mut spring = CriticalSpring::new(0.0, 0.5)?;
    /// spring.retarget(5.0)?;
    /// spring.snap();
    /// assert_eq!(spring.position(), 5.0);
    /// assert_eq!(spring.velocity(), 0.0);
    /// # Ok(())
    /// # }
    /// ```
    pub fn snap(&mut self) {
        self.position = self.target;
        self.velocity = 0.0;
    }
}
