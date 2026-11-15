//! Bounded presentation data and frame-local identity.
//!
//! Absorbed from `primitives/canvas-native/crates/canvas/src/model.rs`
//! (kernel#850 row 15). All bounds cover payload bytes, queued commands, and
//! one pixel buffer — not RSS. Session identity comes from the absorbed
//! [`crate::canvas_protocol::SessionInvariant`]; motion errors come from the
//! absorbed [`crate::motion::MotionError`]. The lease and the artifact live in
//! their own modules and are re-exported here.

use std::fmt;

use crate::motion::MotionError;

pub use super::artifact::Artifact;
pub use super::lease::PresentationLease;

/// Presentation failure modes.
///
/// # Examples
///
/// ```
/// use drishti::canvas_scene::Error;
///
/// assert_eq!(Error::QueueFull.to_string(), "Canvas presentation: QueueFull");
/// assert_ne!(Error::Stale, Error::Revoked);
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// Malformed input or out-of-domain parameter.
    InvalidInput,
    /// A bounded resource would overflow.
    BudgetExceeded,
    /// Data belongs to a different session or work object.
    WrongScope,
    /// Stale generation, revision, or token.
    Stale,
    /// The bounded inbox is full.
    QueueFull,
    /// The instance was revoked.
    Revoked,
    /// Motion evaluation failed.
    Motion(MotionError),
    /// An integer conversion overflowed its target.
    IntegerConversion(core::num::TryFromIntError),
}

impl Error {
    /// Enforce a reducer precondition without adding an effect to the core.
    pub(crate) fn require(valid: bool, error: Self) -> Result<(), Self> {
        if valid { Ok(()) } else { Err(error) }
    }
}

impl From<MotionError> for Error {
    /// Wrap a motion failure.
    fn from(error: MotionError) -> Self {
        Self::Motion(error)
    }
}

impl From<core::num::TryFromIntError> for Error {
    /// Keep the conversion source instead of discarding it.
    fn from(error: core::num::TryFromIntError) -> Self {
        Self::IntegerConversion(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Canvas presentation: {self:?}")
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Motion(ref error) => Some(error),
            Self::IntegerConversion(ref error) => Some(error),
            _ => None,
        }
    }
}

/// Resource envelope: payload bytes, queued commands, one pixel buffer.
///
/// Fields are private: construction runs through the [`Limits`] literal or
/// [`Default`], validation through [`Limits::validate`], and reads go
/// through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::canvas_scene::Limits;
///
/// let limits = Limits::default();
/// assert_eq!((limits.text_bytes(), limits.queued_commands(), limits.pixels()), (262_144, 32, 8_847_360));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum text payload bytes (256 KiB default).
    text_bytes: usize,
    /// Maximum queued commands (32 default).
    queued_commands: usize,
    /// Maximum pixel buffer (4096x2160 default).
    pixels: usize,
}

impl Default for Limits {
    /// Default envelope: 256 KiB text, 32 queued commands, 4096x2160 pixels.
    fn default() -> Self {
        Self {
            text_bytes: 262_144,
            queued_commands: 32,
            pixels: 8_847_360,
        }
    }
}

impl Limits {
    /// Assemble a resource envelope, validating every ceiling immediately.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for a zero or over-ceiling field:
    /// text up to 1 MiB, queues up to 256, buffers up to 4096x2160.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::{Error, Limits};
    ///
    /// assert!(Limits::new(1024, 4, 100).is_ok());
    /// assert_eq!(Limits::new(0, 4, 100), Err(Error::InvalidInput));
    /// assert_eq!(Limits::new(1024, 257, 100), Err(Error::InvalidInput));
    /// ```
    pub fn new(text_bytes: usize, queued_commands: usize, pixels: usize) -> Result<Self, Error> {
        let envelope = Self {
            text_bytes,
            queued_commands,
            pixels,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    /// Accept text up to 1 MiB, queues up to 256, buffers up to 4096x2160.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for a zero or over-ceiling field.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::Limits;
    ///
    /// let limits = Limits::default();
    /// assert_eq!(limits.validate(), Ok(limits));
    /// ```
    pub fn validate(self) -> Result<Self, Error> {
        Error::require(
            self.text_bytes != 0
                && self.text_bytes <= 1_048_576
                && self.queued_commands != 0
                && self.queued_commands <= 256
                && self.pixels != 0
                && self.pixels <= 8_847_360,
            Error::InvalidInput,
        )?;
        Ok(self)
    }

    /// Ceiling on the artifact payload: key plus the three lens texts, in bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::Limits;
    ///
    /// assert_eq!(Limits::new(2048, 8, 1000)?.text_bytes(), 2048);
    /// # Ok::<(), drishti::canvas_scene::Error>(())
    /// ```
    #[must_use]
    pub const fn text_bytes(&self) -> usize {
        self.text_bytes
    }

    /// Ceiling on the reducer inbox; an enqueue past it fails with `QueueFull`.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::Limits;
    ///
    /// assert_eq!(Limits::new(2048, 8, 1000)?.queued_commands(), 8);
    /// # Ok::<(), drishti::canvas_scene::Error>(())
    /// ```
    #[must_use]
    pub const fn queued_commands(&self) -> usize {
        self.queued_commands
    }

    /// Ceiling on the viewport area a frame may be prepared for, in pixels.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::Limits;
    ///
    /// assert_eq!(Limits::new(2048, 8, 1000)?.pixels(), 1000);
    /// # Ok::<(), drishti::canvas_scene::Error>(())
    /// ```
    #[must_use]
    pub const fn pixels(&self) -> usize {
        self.pixels
    }

    /// Checked pixel count for a viewport, bounded by [`Limits::pixels`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for a zero dimension and
    /// [`Error::BudgetExceeded`] for overflow or an over-budget buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::{Error, Limits};
    ///
    /// let limits = Limits::default();
    /// assert_eq!(limits.pixel_count(900, 600), Ok(540_000));
    /// assert_eq!(limits.pixel_count(0, 600), Err(Error::InvalidInput));
    /// assert_eq!(limits.pixel_count(u32::MAX, u32::MAX), Err(Error::BudgetExceeded));
    /// ```
    pub fn pixel_count(self, width: u32, height: u32) -> Result<usize, Error> {
        Error::require(width != 0 && height != 0, Error::InvalidInput)?;
        // `u32` fits in `usize` on every target: the refusal below is
        // unreachable, and keeps its source for the caller.
        let width_count = usize::try_from(width)?;
        let height_count = usize::try_from(height)?;
        let count = width_count
            .checked_mul(height_count)
            .ok_or(Error::BudgetExceeded)?;
        Error::require(count <= self.pixels, Error::BudgetExceeded)?;
        Ok(count)
    }
}

/// The three presentation lenses.
///
/// # Examples
///
/// ```
/// use drishti::canvas_scene::View;
///
/// assert_eq!(View::ALL.len(), 3);
/// assert_ne!(View::Source, View::Evidence);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum View {
    /// Authored source.
    Source,
    /// Proposed change.
    Proposal,
    /// Supporting evidence.
    Evidence,
}

impl View {
    /// All three views in binding order.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::View;
    ///
    /// assert_eq!(View::ALL, [View::Source, View::Proposal, View::Evidence]);
    /// ```
    pub const ALL: [Self; 3] = [Self::Source, Self::Proposal, Self::Evidence];

    /// Binding index: source 0, proposal 1, evidence 2.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::View;
    ///
    /// for (position, view) in View::ALL.into_iter().enumerate() {
    ///     assert_eq!(view.index(), position);
    /// }
    /// ```
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Source => 0,
            Self::Proposal => 1,
            Self::Evidence => 2,
        }
    }

    /// Human-readable lens name for the navigation bar and screen readers.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::View;
    ///
    /// assert_eq!(View::Proposal.label(), "Proposal");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Source => "Source",
            Self::Proposal => "Proposal",
            Self::Evidence => "Evidence",
        }
    }
}

/// Distinct status for work objects across their lifecycle.
///
/// # Examples
///
/// ```
/// use drishti::canvas_scene::CommitStatus;
///
/// assert_eq!(CommitStatus::default(), CommitStatus::Proposed);
/// assert_ne!(CommitStatus::Failed, CommitStatus::Committed);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum CommitStatus {
    /// Drafted, not yet running.
    #[default]
    Proposed,
    /// Executing.
    Running,
    /// Failed.
    Failed,
    /// Confirmed.
    Committed,
}

impl CommitStatus {
    /// All four statuses in lifecycle order.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::CommitStatus;
    ///
    /// assert_eq!(CommitStatus::ALL[0], CommitStatus::default());
    /// assert_eq!(CommitStatus::ALL[3], CommitStatus::Committed);
    /// ```
    pub const ALL: [Self; 4] = [Self::Proposed, Self::Running, Self::Failed, Self::Committed];

    /// Human-readable status name for badges and screen readers.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::CommitStatus;
    ///
    /// assert_eq!(CommitStatus::Failed.label(), "Failed");
    /// ```
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Proposed => "Proposed",
            Self::Running => "Running",
            Self::Failed => "Failed",
            Self::Committed => "Committed",
        }
    }
}

/// Local interactions admitted against a presented frame.
///
/// # Examples
///
/// ```
/// use drishti::canvas_scene::{Action, View};
///
/// assert_eq!(Action::Navigate(View::Proposal), Action::Navigate(View::Proposal));
/// assert_ne!(Action::Step(1), Action::Step(-1));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Action {
    /// Navigate to a view.
    Navigate(View),
    /// Step relatively across views (wraps).
    Step(i32),
    /// Toggle reduced motion.
    ToggleReducedMotion,
    /// Set reduced motion explicitly.
    ReducedMotion(bool),
    /// Scroll a view by lines.
    Scroll {
        /// View to scroll.
        view: View,
        /// Signed line delta.
        lines: i32,
    },
    /// Claim a domain commit status (never via frame input).
    Transition(CommitStatus),
    /// Overlay an optimistic status (never via frame input).
    OptimisticTransition(CommitStatus),
    /// Drop the optimistic overlay (never via frame input).
    RevertOptimistic,
}

impl Action {
    /// Build a scroll action (struct-variant construction from outside the
    /// crate goes through here because the enum is `non_exhaustive`).
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::{Action, View};
    ///
    /// assert_eq!(Action::scroll(View::Evidence, -2), Action::scroll(View::Evidence, -2));
    /// assert_ne!(Action::scroll(View::Evidence, -2), Action::scroll(View::Source, -2));
    /// ```
    #[must_use]
    pub const fn scroll(view: View, lines: i32) -> Self {
        Self::Scroll { view, lines }
    }
}
