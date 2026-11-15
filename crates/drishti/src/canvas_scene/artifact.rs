//! The presented artifact: one stable object with three caller-supplied lenses.

use std::sync::Arc;

use super::model::{CommitStatus, Error, Limits, View};

/// One stable presentation object with three caller-supplied lenses. No
/// fabricated evidence: missing proposal/evidence carry explicit absence text
/// and `supplied == false`.
///
/// # Examples
///
/// ```
/// use drishti::canvas_scene::{Artifact, Limits, View};
///
/// let artifact = Artifact::new("work-1", 1, "fn main() {}", None, Some("tests pass"), Limits::default())?;
/// assert!(artifact.supplied(View::Source) && !artifact.supplied(View::Proposal));
/// assert_eq!(artifact.text(View::Evidence), "tests pass");
/// # Ok::<(), drishti::canvas_scene::Error>(())
/// ```
#[derive(Debug)]
pub struct Artifact {
    /// Stable object key, bound to the session work object id.
    key: Arc<str>,
    /// Content revision, always positive.
    revision: u64,
    /// Domain commit status.
    status: CommitStatus,
    /// Lens texts in [`View::index`] order.
    texts: [Arc<str>; 3],
    /// Which lenses were caller-supplied.
    supplied: [bool; 3],
}

impl Artifact {
    /// Build an artifact with `Proposed` status.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for a malformed key or revision, or
    /// [`Error::BudgetExceeded`] for an over-envelope payload.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::{Artifact, CommitStatus, Error, Limits};
    ///
    /// let artifact = Artifact::new("work-1", 1, "src", None, None, Limits::default())?;
    /// assert_eq!(artifact.commit_status(), CommitStatus::Proposed);
    /// let tiny = Limits::new(8, 1, 100)?;
    /// let over = Artifact::new("work-1", 1, "a long source text", None, None, tiny);
    /// assert_eq!(over.err(), Some(Error::BudgetExceeded));
    /// # Ok::<(), Error>(())
    /// ```
    pub fn new(
        key: &str,
        revision: u64,
        source: &str,
        proposal: Option<&str>,
        evidence: Option<&str>,
        limits: Limits,
    ) -> Result<Self, Error> {
        Self::with_status(
            key,
            revision,
            source,
            proposal,
            evidence,
            CommitStatus::Proposed,
            limits,
        )
    }

    /// Build an artifact with an explicit status.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for a malformed key, a zero revision,
    /// or embedded null bytes, or [`Error::BudgetExceeded`] for an
    /// over-envelope payload.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::{Artifact, CommitStatus, Error, Limits};
    ///
    /// let limits = Limits::default();
    /// let failed = Artifact::with_status("work-1", 2, "src", None, None, CommitStatus::Failed, limits)?;
    /// assert_eq!(failed.commit_status(), CommitStatus::Failed);
    /// let zero = Artifact::with_status("work-1", 0, "src", None, None, CommitStatus::Failed, limits);
    /// assert_eq!(zero.err(), Some(Error::InvalidInput));
    /// # Ok::<(), Error>(())
    /// ```
    pub fn with_status(
        key: &str,
        revision: u64,
        source: &str,
        proposal: Option<&str>,
        evidence: Option<&str>,
        status: CommitStatus,
        limits: Limits,
    ) -> Result<Self, Error> {
        limits.validate()?;
        Error::require(
            !key.is_empty()
                && key.len() <= 512
                && !key.chars().any(char::is_control)
                && revision != 0,
            Error::InvalidInput,
        )?;
        let texts = [
            source,
            match proposal {
                Some(text) => text,
                // Absent lenses carry explicit absence text, never a guess.
                None => "No proposal supplied. Nothing has been applied.",
            },
            match evidence {
                Some(text) => text,
                None => "No evidence supplied. No tests or verification are claimed.",
            },
        ];
        let bytes = texts
            .iter()
            .try_fold(key.len(), |sum, text| sum.checked_add(text.len()))
            .ok_or(Error::BudgetExceeded)?;
        Error::require(bytes <= limits.text_bytes(), Error::BudgetExceeded)?;
        Error::require(
            !texts.iter().any(|text| text.contains('\0')),
            Error::InvalidInput,
        )?;
        Ok(Self {
            key: Arc::from(key),
            revision,
            status,
            texts: texts.map(Arc::from),
            supplied: [true, proposal.is_some(), evidence.is_some()],
        })
    }

    /// Stable object key; a canvas accepts the artifact only when it equals
    /// the session's work object id.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::{Artifact, Limits};
    ///
    /// let artifact = Artifact::new("work-1", 1, "src", None, None, Limits::default())?;
    /// assert_eq!(artifact.key(), "work-1");
    /// # Ok::<(), drishti::canvas_scene::Error>(())
    /// ```
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Version of the artifact's content as the caller numbered it; zero is
    /// refused at construction.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::{Artifact, Limits};
    ///
    /// let artifact = Artifact::new("work-1", 7, "src", None, None, Limits::default())?;
    /// assert_eq!(artifact.revision(), 7);
    /// # Ok::<(), drishti::canvas_scene::Error>(())
    /// ```
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Domain commit status the artifact was built with.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::{Artifact, CommitStatus, Limits};
    ///
    /// let artifact = Artifact::with_status("work-1", 1, "s", None, None, CommitStatus::Running, Limits::default())?;
    /// assert_eq!(artifact.commit_status(), CommitStatus::Running);
    /// # Ok::<(), drishti::canvas_scene::Error>(())
    /// ```
    #[must_use]
    pub const fn commit_status(&self) -> CommitStatus {
        self.status
    }

    /// Text shown in a lens: the caller's text, or the explicit absence
    /// notice for a lens nobody supplied.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::{Artifact, Limits, View};
    ///
    /// let artifact = Artifact::new("work-1", 1, "src", Some("patch"), None, Limits::default())?;
    /// assert_eq!(artifact.text(View::Proposal), "patch");
    /// assert!(artifact.text(View::Evidence).starts_with("No evidence supplied"));
    /// # Ok::<(), drishti::canvas_scene::Error>(())
    /// ```
    #[must_use]
    pub fn text(&self, view: View) -> &str {
        &self.texts[view.index()]
    }

    /// Whether the lens was caller-supplied (false means absence text).
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::{Artifact, Limits, View};
    ///
    /// let artifact = Artifact::new("work-1", 1, "src", Some("patch"), None, Limits::default())?;
    /// assert_eq!(View::ALL.map(|view| artifact.supplied(view)), [true, true, false]);
    /// # Ok::<(), drishti::canvas_scene::Error>(())
    /// ```
    #[must_use]
    pub fn supplied(&self, view: View) -> bool {
        self.supplied[view.index()]
    }

    /// Total payload bytes: key plus all three lens texts.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::{Artifact, Limits};
    ///
    /// let artifact = Artifact::new("k", 1, "ab", Some("cd"), Some("ef"), Limits::default())?;
    /// assert_eq!(artifact.payload_bytes(), 7);
    /// # Ok::<(), drishti::canvas_scene::Error>(())
    /// ```
    #[must_use]
    pub fn payload_bytes(&self) -> usize {
        self.texts
            .iter()
            .fold(self.key.len(), |sum, text| sum.saturating_add(text.len()))
    }
}
