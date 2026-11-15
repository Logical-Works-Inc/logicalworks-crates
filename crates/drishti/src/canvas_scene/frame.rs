//! Prepared presentation frames and their frame-local authority.
//!
//! Split from the model for the size law: the model owns the input
//! vocabulary, resource envelope, lease and artifact; this module owns the
//! frame the reducer prepares and the token that admits input against it.

use std::sync::Arc;

use super::model::{Artifact, CommitStatus, View};

/// Frame-local authority. Cannot be constructed outside [`crate::canvas_scene::Canvas`];
/// not a capability token or an authenticated lease. Equality is instance
/// identity plus generation, serial, and revision.
///
/// # Examples
///
/// ```
/// # use drishti::canvas_protocol::SessionInvariant;
/// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
/// # let id = "11111111-1111-1111-1111-111111111111";
/// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
/// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
/// let first = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?.token();
/// let second = canvas.prepare(0.0, 900, 600, true)?.ok_or("forced frame")?.token();
/// assert_eq!(first.clone(), first);
/// assert_ne!(first, second);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
pub struct FrameToken {
    /// Presenting instance identity.
    pub(crate) instance: Arc<()>,
    /// Instance generation.
    pub(crate) generation: u64,
    /// Frame serial within the instance.
    pub(crate) serial: u64,
    /// Scene revision the frame was prepared at.
    pub(crate) revision: u64,
}

impl PartialEq for FrameToken {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.instance, &other.instance)
            && self.generation == other.generation
            && self.serial == other.serial
            && self.revision == other.revision
    }
}

impl Eq for FrameToken {}

/// One prepared presentation frame.
///
/// Fields are private: the reducer builds frames through [`Frame::new`] and
/// every reader goes through the accessors.
///
/// # Examples
///
/// ```
/// # use drishti::canvas_protocol::SessionInvariant;
/// # use drishti::canvas_scene::{Artifact, Canvas, Limits, View};
/// # let id = "11111111-1111-1111-1111-111111111111";
/// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
/// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
/// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
/// assert_eq!(frame.artifact().key(), id);
/// assert_eq!((frame.width(), frame.height(), frame.target()), (900, 600, View::Source));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Frame {
    /// Frame-local authority.
    pub(crate) token: FrameToken,
    /// Presented artifact.
    artifact: Arc<Artifact>,
    /// Visible status (optimistic overlay when set).
    commit_status: CommitStatus,
    /// Confirmed domain status, never the overlay.
    confirmed_commit_status: CommitStatus,
    /// Whether an optimistic overlay is active.
    optimistic: bool,
    /// Spring position on the 0/1/2 view axis.
    position: f64,
    /// Spring velocity.
    velocity: f64,
    /// Navigation target.
    target: View,
    /// Scroll offsets in [`View::index`] order.
    scroll: [usize; 3],
    /// Viewport width.
    width: u32,
    /// Viewport height.
    height: u32,
    /// Reduced-motion mode.
    reduced_motion: bool,
    /// Whether motion is still settling.
    animating: bool,
}

impl Frame {
    /// Assemble one prepared frame. Only the reducer builds these: the core
    /// identity (authority, artifact, statuses, spring state) arrives here
    /// and the view state attaches through [`Frame::set_target`],
    /// [`Frame::set_scroll`], [`Frame::set_viewport`],
    /// [`Frame::set_reduced_motion`], and [`Frame::set_animating`], so one
    /// expression still admits no half-built frame while this constructor
    /// stays at the seven-argument clipping bound.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, CommitStatus, Frame, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let prepared = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// let frame = Frame::new(prepared.token(), prepared.artifact().clone(), CommitStatus::Running, CommitStatus::Proposed, false, 0.0, 1.0);
    /// assert_eq!((frame.commit_status(), frame.position(), frame.velocity()), (CommitStatus::Running, 1.0, 0.0));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new(
        token: FrameToken,
        artifact: Arc<Artifact>,
        commit_status: CommitStatus,
        confirmed_commit_status: CommitStatus,
        optimistic: bool,
        velocity: f64,
        position: f64,
    ) -> Self {
        Self {
            token,
            artifact,
            commit_status,
            confirmed_commit_status,
            optimistic,
            velocity,
            position,
            target: View::Source,
            scroll: [0, 0, 0],
            width: 0,
            height: 0,
            reduced_motion: false,
            animating: false,
        }
    }

    /// Attach the navigation target.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits, View};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let mut frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// frame.set_target(View::Evidence);
    /// assert_eq!(frame.target(), View::Evidence);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn set_target(&mut self, target: View) {
        self.target = target;
    }

    /// Attach the scroll offsets in [`View::index`] order.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let mut frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// frame.set_scroll([0, 1, 0]);
    /// assert_eq!(frame.scroll(), [0, 1, 0]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn set_scroll(&mut self, scroll: [usize; 3]) {
        self.scroll = scroll;
    }

    /// Attach the viewport size.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let mut frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// frame.set_viewport(1024, 768);
    /// assert_eq!((frame.width(), frame.height()), (1024, 768));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// Mark the frame as prepared under reduced motion, so a renderer snaps
    /// instead of tweening.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let mut frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// frame.set_reduced_motion(true);
    /// assert!(frame.reduced_motion());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
    }

    /// Attach whether motion is still settling.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let mut frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// frame.set_animating(true);
    /// assert!(frame.animating());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn set_animating(&mut self, animating: bool) {
        self.animating = animating;
    }

    /// Frame-local authority.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert_eq!(frame.token(), frame.token());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn token(&self) -> FrameToken {
        self.token.clone()
    }

    /// Shared handle to the artifact this frame presents; its text is what
    /// the renderer draws.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits, View};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert_eq!(frame.artifact().text(View::Source), "one\ntwo");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn artifact(&self) -> &Arc<Artifact> {
        &self.artifact
    }

    /// Visible status (optimistic overlay when set).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, CommitStatus, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert_eq!(frame.commit_status(), CommitStatus::Proposed);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn commit_status(&self) -> CommitStatus {
        self.commit_status
    }

    /// Confirmed domain status, never the overlay.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, CommitStatus, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert_eq!(frame.confirmed_commit_status(), CommitStatus::Proposed);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn confirmed_commit_status(&self) -> CommitStatus {
        self.confirmed_commit_status
    }

    /// Whether an optimistic overlay is active.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert!(!frame.optimistic());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn optimistic(&self) -> bool {
        self.optimistic
    }

    /// Spring position on the 0/1/2 view axis.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert!((0.0..=2.0).contains(&frame.position()));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn position(&self) -> f64 {
        self.position
    }

    /// Spring velocity in view-axis units per second; finite for every
    /// prepared frame.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert!(frame.velocity().is_finite());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn velocity(&self) -> f64 {
        self.velocity
    }

    /// View the spring is travelling to (the navigation target at prepare time).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits, View};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert_eq!(frame.target(), View::Source);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn target(&self) -> View {
        self.target
    }

    /// Scroll offsets in [`View::index`] order.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert_eq!(frame.scroll(), [0, 0, 0]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn scroll(&self) -> [usize; 3] {
        self.scroll
    }

    /// Viewport width in pixels this frame was prepared for.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 800, 600, false)?.ok_or("frame owed")?;
    /// assert_eq!(frame.width(), 800);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Viewport height in pixels this frame was prepared for.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 800, 600, false)?.ok_or("frame owed")?;
    /// assert_eq!(frame.height(), 600);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Whether the frame was prepared under reduced motion, which snaps the
    /// spring instead of animating it.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert!(!frame.reduced_motion());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    /// Whether motion is still settling.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, CommitStatus, Frame, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let prepared = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// let frame = Frame::new(prepared.token(), prepared.artifact().clone(), CommitStatus::Proposed, CommitStatus::Proposed, false, 0.0, 0.0);
    /// assert!(!frame.animating());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn animating(&self) -> bool {
        self.animating
    }

    /// Pending visual state must never impersonate confirmed domain state.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, CommitStatus, Frame, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let prepared = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert_eq!(prepared.status_label(), "Proposed");
    /// let pending = Frame::new(prepared.token(), prepared.artifact().clone(), CommitStatus::Committed, CommitStatus::Proposed, true, 0.0, 0.0);
    /// assert_eq!(pending.status_label(), "Commit pending");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn status_label(&self) -> &'static str {
        if !self.optimistic {
            return self.commit_status.label();
        }
        match self.commit_status {
            CommitStatus::Proposed => "Proposal pending",
            CommitStatus::Running => "Run pending",
            CommitStatus::Failed => "Failure pending",
            CommitStatus::Committed => "Commit pending",
        }
    }

    /// Revision of the scene when this frame was prepared; a frame older
    /// than the canvas revision is stale.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert_eq!(frame.scene_revision(), canvas.revision());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn scene_revision(&self) -> u64 {
        self.token.revision
    }

    /// Navigation hit test against the presented viewport. The bar occupies
    /// the bottom 56 pixels in three equal thirds; anything else, or any
    /// non-finite coordinate, misses.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits, View};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert_eq!(frame.hit_test(10.0, 590.0), Some(View::Source));
    /// assert_eq!(frame.hit_test(450.0, 590.0), Some(View::Proposal));
    /// assert_eq!(frame.hit_test(850.0, 590.0), Some(View::Evidence));
    /// assert_eq!(frame.hit_test(450.0, 100.0), None);
    /// assert_eq!(frame.hit_test(f64::NAN, 590.0), None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn hit_test(&self, x: f64, y: f64) -> Option<View> {
        let width = f64::from(self.width);
        let height = f64::from(self.height);
        if !x.is_finite()
            || !y.is_finite()
            || x < 0.0
            || x >= width
            || y < height - 56.0
            || y >= height
        {
            return None;
        }
        let third = width / 3.0;
        if x < third {
            Some(View::Source)
        } else if x < third + third {
            Some(View::Proposal)
        } else {
            Some(View::Evidence)
        }
    }
}
