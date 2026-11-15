//! Read-only accessors of the presentation reducer.

use super::Canvas;
use crate::canvas_protocol::SessionInvariant;
use crate::canvas_scene::{
    CommitStatus, Error, FrameToken, Limits, PresentationLease, Statistics, View,
};

impl Canvas {
    /// Session every accepted delta must equal; the scope key of this canvas.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let canvas = Canvas::new(session.clone(), 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// assert_eq!(canvas.session(), &session);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn session(&self) -> &SessionInvariant {
        &self.session
    }

    /// Lease the canvas was built with; `Canvas::new` grants a diagnostic one.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// assert_eq!(canvas.lease().map(|lease| lease.surface_id()), Some("primary-surface"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn lease(&self) -> Option<&PresentationLease> {
        self.lease.as_ref()
    }

    /// Visible status (optimistic overlay when set).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Artifact, Canvas, CommitStatus, Delta, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session.clone(), 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// assert_eq!(canvas.commit_status(), CommitStatus::Proposed);
    /// canvas.enqueue(Delta::new(session, 1, 0, Action::OptimisticTransition(CommitStatus::Committed)))?;
    /// canvas.prepare(0.0, 900, 600, false)?;
    /// assert_eq!(canvas.commit_status(), CommitStatus::Committed);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn commit_status(&self) -> CommitStatus {
        match self.optimistic_status {
            Some(overlay) => overlay,
            None => self.commit_status,
        }
    }

    /// Confirmed domain status, never the overlay.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Artifact, Canvas, CommitStatus, Delta, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session.clone(), 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// canvas.enqueue(Delta::new(session, 1, 0, Action::OptimisticTransition(CommitStatus::Committed)))?;
    /// canvas.prepare(0.0, 900, 600, false)?;
    /// assert_eq!(canvas.confirmed_commit_status(), CommitStatus::Proposed);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn confirmed_commit_status(&self) -> CommitStatus {
        self.commit_status
    }

    /// Whether an optimistic overlay is active.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Artifact, Canvas, CommitStatus, Delta, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session.clone(), 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// assert!(!canvas.is_optimistic());
    /// canvas.enqueue(Delta::new(session, 1, 0, Action::OptimisticTransition(CommitStatus::Running)))?;
    /// canvas.prepare(0.0, 900, 600, false)?;
    /// assert!(canvas.is_optimistic());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn is_optimistic(&self) -> bool {
        self.optimistic_status.is_some()
    }

    /// Instance generation the lease binds; deltas for another generation
    /// are stale.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let canvas = Canvas::new(session, 4, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// assert_eq!(canvas.generation(), 4);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Scene revision: counts drained actions and optimistic denials.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Artifact, Canvas, Delta, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session.clone(), 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// assert_eq!(canvas.revision(), 0);
    /// canvas.enqueue(Delta::new(session, 1, 0, Action::Step(1)))?;
    /// canvas.prepare(0.0, 900, 600, false)?;
    /// assert_eq!(canvas.revision(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Revision the next accepted delta must name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] on counter overflow.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Artifact, Canvas, Delta, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session.clone(), 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// assert_eq!(canvas.next_base_revision()?, 0);
    /// canvas.enqueue(Delta::new(session, 1, 0, Action::Step(1)))?;
    /// assert_eq!(canvas.next_base_revision()?, 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn next_base_revision(&self) -> Result<u64, Error> {
        let inbox_count = u64::try_from(self.inbox.len())?;
        self.revision
            .checked_add(inbox_count)
            .ok_or(Error::InvalidInput)
    }

    /// Reducer counters.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Artifact, Canvas, Delta, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session.clone(), 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// assert_eq!(canvas.stats().accepted(), 0);
    /// canvas.enqueue(Delta::new(session, 1, 0, Action::Step(1)))?;
    /// assert_eq!(canvas.stats().accepted(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn stats(&self) -> Statistics {
        self.stats
    }

    /// Resource envelope the canvas enforces on its inbox, payload, and viewport.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// assert_eq!(canvas.limits(), Limits::default());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    /// Whether a frame is owed: revoked instances never owe one.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// assert!(canvas.needs_frame());
    /// canvas.revoke();
    /// assert!(!canvas.needs_frame());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn needs_frame(&self) -> bool {
        !self.revoked && (self.dirty || !self.inbox.is_empty() || !self.motion.is_settled())
    }

    /// Token of the frame the adapter acknowledged; input is admitted only
    /// against it, and any drained action clears it.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// assert!(canvas.presented_token().is_none());
    /// let token = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?.token();
    /// canvas.acknowledge_present(token.clone())?;
    /// assert_eq!(canvas.presented_token(), Some(token));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn presented_token(&self) -> Option<FrameToken> {
        self.presented.clone()
    }

    /// View the next prepared frame navigates to.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Artifact, Canvas, Delta, Limits, View};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session.clone(), 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// assert_eq!(canvas.target(), View::Source);
    /// canvas.enqueue(Delta::new(session, 1, 0, Action::Navigate(View::Evidence)))?;
    /// canvas.prepare(0.0, 900, 600, false)?;
    /// assert_eq!(canvas.target(), View::Evidence);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn target(&self) -> View {
        self.target
    }

    /// Whether motion snaps instead of animating.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Artifact, Canvas, Delta, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session.clone(), 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// assert!(!canvas.reduced_motion());
    /// canvas.enqueue(Delta::new(session, 1, 0, Action::ReducedMotion(true)))?;
    /// canvas.prepare(0.0, 900, 600, false)?;
    /// assert!(canvas.reduced_motion());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn reduced_motion(&self) -> bool {
        self.reduced
    }
}
