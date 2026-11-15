//! Clock abstraction for the motion kernel.
//!
//! All node evaluations in one [`crate::motion::frame()`] call see the same
//! clock value. Time is injected, never read from the wall, so replays are
//! bit-identical.

/// Explicit wall-clock milliseconds, used in tests and replay.
///
/// # Examples
///
/// ```
/// use drishti::motion::Clock;
///
/// let clock = Clock::deterministic(12.5);
/// assert!(matches!(clock, Clock::Deterministic(wall_ms) if wall_ms == 12.5));
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Clock {
    /// Deterministic timestamp in milliseconds.
    Deterministic(f64),
}

impl Clock {
    /// Construct a deterministic clock at an explicit timestamp. The value is
    /// stored as given: nothing is validated, so a NaN or negative stamp is
    /// carried through to the frame evaluator unchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::Clock;
    ///
    /// let clock = Clock::deterministic(3.0);
    /// assert_eq!(clock.wall_clock_ms(), 3.0);
    /// ```
    #[must_use]
    pub const fn deterministic(wall_ms: f64) -> Self {
        Self::Deterministic(wall_ms)
    }

    /// Current wall-clock time in milliseconds, exactly the stamp the clock
    /// was built or advanced to; reading never moves it.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::Clock;
    ///
    /// let clock = Clock::deterministic(42.0);
    /// assert_eq!(clock.wall_clock_ms(), 42.0);
    /// assert_eq!(clock.wall_clock_ms(), 42.0);
    /// ```
    #[must_use]
    pub const fn wall_clock_ms(&self) -> f64 {
        match *self {
            Self::Deterministic(wall_ms) => wall_ms,
        }
    }

    /// A NEW clock `delta_ms` milliseconds later. This does not mutate `self`,
    /// so a caller that drops the return value has not advanced anything.
    ///
    /// `delta_ms` is not validated: a negative value silently rewinds the
    /// clock. [`crate::motion::MotionErrorCode::ClockBackwards`] names exactly
    /// that condition but is never constructed by this crate, so the invariant
    /// is DECLARED and UNENFORCED — a caller driving this from an external
    /// time source must reject backwards deltas itself.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::Clock;
    ///
    /// let start = Clock::deterministic(10.0);
    /// let later = start.advance_ms(6.0);
    /// assert_eq!(start.wall_clock_ms(), 10.0);
    /// assert_eq!(later.wall_clock_ms(), 16.0);
    /// ```
    #[must_use]
    pub const fn advance_ms(&self, delta_ms: f64) -> Self {
        match *self {
            Self::Deterministic(wall_ms) => Self::Deterministic(wall_ms + delta_ms),
        }
    }
}
