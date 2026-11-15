//! Typed errors for the motion core.
//!
//! Every variant maps to one `MotionErrorCode` from the G01 schema. Every
//! recoverable failure returns `Result<_, MotionError>`.

use lgwks_macros::Error;

/// Every variant maps to one `MotionErrorCode` from the G01 schema.
///
/// # Examples
///
/// ```
/// use drishti::motion::MotionErrorCode;
///
/// // Display prints the schema's wire name, not the Rust variant name.
/// assert_eq!(MotionErrorCode::NodeNotFound.to_string(), "NODE_NOT_FOUND");
/// assert_eq!(MotionErrorCode::SpringUnstable.to_string(), "SPRING_UNSTABLE");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MotionErrorCode {
    /// Cubic-bezier x control points outside [0, 1].
    EasingDomainError,
    /// An interpolator produced NaN.
    InterpolatorNaN,
    /// An interpolator failed structurally.
    InterpolatorPanic,
    /// A patch or lookup referenced an unknown node.
    NodeNotFound,
    /// A patch addressed an immutable path.
    PatchPathImmutable,
    /// A patch addressed an unknown path.
    PatchPathUnknown,
    /// A patch value or combination is invalid.
    PatchInvalid,
    /// Spring parameters or integration are unstable.
    SpringUnstable,
    /// The clock moved backwards (declared, unenforced).
    ClockBackwards,
    /// A graph snapshot conflicted on version.
    GraphVersionConflict,
}

impl core::fmt::Display for MotionErrorCode {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let code = match *self {
            Self::EasingDomainError => "EASING_DOMAIN_ERROR",
            Self::InterpolatorNaN => "INTERPOLATOR_NAN",
            Self::InterpolatorPanic => "INTERPOLATOR_PANIC",
            Self::NodeNotFound => "NODE_NOT_FOUND",
            Self::PatchPathImmutable => "MOTION_PATCH_PATH_IMMUTABLE",
            Self::PatchPathUnknown => "MOTION_PATCH_PATH_UNKNOWN",
            Self::PatchInvalid => "MOTION_PATCH_INVALID",
            Self::SpringUnstable => "SPRING_UNSTABLE",
            Self::ClockBackwards => "CLOCK_BACKWARDS",
            Self::GraphVersionConflict => "GRAPH_VERSION_CONFLICT",
        };
        formatter.write_str(code)
    }
}

/// Typed error for all motion-core operations.
///
/// Fields are private: constructors build errors and readers go through the
/// accessors.
///
/// # Examples
///
/// ```
/// use drishti::motion::{MotionError, MotionErrorCode};
///
/// let error = MotionError::new(MotionErrorCode::PatchInvalid).with_detail("bad value");
/// assert_eq!(error.code(), MotionErrorCode::PatchInvalid);
/// assert_eq!(error.detail(), "bad value");
/// ```
#[derive(Clone, Debug, Error, PartialEq)]
#[non_exhaustive]
#[error("{code}: {detail}")]
pub struct MotionError {
    /// Machine-readable failure code.
    code: MotionErrorCode,
    /// Human context for the failure.
    detail: String,
}

impl MotionError {
    /// Build an error with an empty detail string.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{MotionError, MotionErrorCode};
    ///
    /// let error = MotionError::new(MotionErrorCode::NodeNotFound);
    /// assert_eq!(error.code(), MotionErrorCode::NodeNotFound);
    /// assert_eq!(error.detail(), "");
    /// ```
    #[must_use]
    pub fn new(code: MotionErrorCode) -> Self {
        Self {
            code,
            detail: String::new(),
        }
    }

    /// Attach human context to the error, replacing any detail already set.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{MotionError, MotionErrorCode};
    ///
    /// let error = MotionError::new(MotionErrorCode::PatchInvalid)
    ///     .with_detail("first")
    ///     .with_detail("second");
    /// assert_eq!(error.detail(), "second");
    /// ```
    #[must_use]
    pub fn with_detail(mut self, detail: &str) -> Self {
        self.detail = String::from(detail);
        self
    }

    /// Machine-readable failure code, the stable part to branch on; the detail
    /// text may change between releases.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{MotionError, MotionErrorCode};
    ///
    /// let error = MotionError::new(MotionErrorCode::GraphVersionConflict);
    /// assert_eq!(error.code(), MotionErrorCode::GraphVersionConflict);
    /// ```
    #[must_use]
    pub const fn code(&self) -> MotionErrorCode {
        self.code
    }

    /// Human context for the failure; empty until [`MotionError::with_detail`]
    /// attaches some.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{MotionError, MotionErrorCode};
    ///
    /// let error = MotionError::new(MotionErrorCode::SpringUnstable).with_detail("mass");
    /// assert_eq!(error.detail(), "mass");
    /// ```
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// Validate a pure motion constraint while retaining its code and context.
    pub(crate) fn require(valid: bool, code: MotionErrorCode, detail: &str) -> Result<(), Self> {
        if valid {
            Ok(())
        } else {
            Err(Self::new(code).with_detail(detail))
        }
    }
}
