//! Single-owner presentation reducer with a bounded inbox.
//!
//! Absorbed from `primitives/canvas-native/crates/canvas/src/scene.rs`
//! (kernel#850 row 15). Acknowledgement is present-after-success: the adapter
//! calls [`Canvas::acknowledge_present`] only after actual presentation.
//! Headless acknowledgement is not native-display evidence. The read-only
//! accessors live in `query`; the ordering envelope and counters in `delta`.

use std::{collections::VecDeque, sync::Arc};

pub use super::delta::{Delta, Statistics};
use super::frame::{Frame, FrameToken};
use super::model::{Action, Artifact, CommitStatus, Error, Limits, PresentationLease, View};
use crate::canvas_protocol::SessionInvariant;
use crate::motion::node::critical::CriticalSpring;

mod query;

/// Single-owner presentation instance.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::SessionInvariant;
/// use drishti::canvas_scene::{Artifact, Canvas, Limits};
///
/// let id = "11111111-1111-1111-1111-111111111111";
/// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
/// let limits = Limits::default();
/// let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, limits)?, limits)?;
/// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("a new canvas owes a frame")?;
/// canvas.acknowledge_present(frame.token())?;
/// assert!(canvas.presented_token().is_some());
/// assert_eq!(canvas.stats().presented(), 1);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Canvas {
    /// Bound session.
    session: SessionInvariant,
    /// Bound lease, if any.
    lease: Option<PresentationLease>,
    /// Instance identity for token scoping.
    instance: Arc<()>,
    /// Instance generation.
    generation: u64,
    /// Scene revision: increments per drained action and per denial.
    revision: u64,
    /// Resource envelope.
    limits: Limits,
    /// Presented artifact.
    artifact: Arc<Artifact>,
    /// Confirmed domain status.
    commit_status: CommitStatus,
    /// Optimistic overlay, if any.
    optimistic_status: Option<CommitStatus>,
    /// Bounded inbox.
    inbox: VecDeque<Action>,
    /// View-axis spring.
    motion: CriticalSpring,
    /// Navigation target.
    target: View,
    /// Scroll offsets in view-index order.
    scroll: [usize; 3],
    /// Reduced-motion mode.
    reduced: bool,
    /// Whether a frame is owed.
    dirty: bool,
    /// Whether the instance was revoked.
    revoked: bool,
    /// Last prepared token, if any.
    prepared: Option<FrameToken>,
    /// Last presented token, if any.
    presented: Option<FrameToken>,
    /// Frame serial.
    serial: u64,
    /// Reducer counters.
    stats: Statistics,
}

impl Canvas {
    /// Read-only host context. Creates a default diagnostic presentation
    /// lease from the session, generation, and envelope.
    ///
    /// # Errors
    ///
    /// Returns the lease or scope failure when the session cannot present
    /// the artifact.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::SessionInvariant;
    /// use drishti::canvas_scene::{Artifact, Canvas, Error, Limits};
    ///
    /// let id = "11111111-1111-1111-1111-111111111111";
    /// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// let limits = Limits::default();
    /// let canvas = Canvas::new(session.clone(), 1, Artifact::new(id, 1, "src", None, None, limits)?, limits)?;
    /// assert!(canvas.lease().is_some() && canvas.needs_frame());
    /// let foreign = Artifact::new("other-work", 1, "src", None, None, limits)?;
    /// assert_eq!(Canvas::new(session, 1, foreign, limits).err(), Some(Error::WrongScope));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(
        session: SessionInvariant,
        generation: u64,
        artifact: Artifact,
        limits: Limits,
    ) -> Result<Self, Error> {
        let lease = PresentationLease::new(
            "diagnostic-lease",
            "primary-surface",
            session,
            generation,
            limits,
        )?;
        Self::with_lease(lease, artifact)
    }

    /// Construct a presentation instance bound to an explicit versioned lease.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for an invalid lease,
    /// [`Error::WrongScope`] when the artifact key differs from the session
    /// work object id, or [`Error::BudgetExceeded`] for an over-envelope
    /// payload.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::SessionInvariant;
    /// use drishti::canvas_scene::{Artifact, Canvas, Limits, PresentationLease};
    ///
    /// let id = "11111111-1111-1111-1111-111111111111";
    /// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// let limits = Limits::default();
    /// let lease = PresentationLease::new("lease-1", "surface-1", session, 3, limits)?;
    /// let canvas = Canvas::with_lease(lease, Artifact::new(id, 1, "src", None, None, limits)?)?;
    /// assert_eq!(canvas.generation(), 3);
    /// assert_eq!(canvas.lease().map(|lease| lease.lease_id()), Some("lease-1"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_lease(lease: PresentationLease, artifact: Artifact) -> Result<Self, Error> {
        lease.validate()?;
        let limits = lease.limits();
        let session = lease.session().clone();
        let generation = lease.generation();
        // Presentation data belongs to the session work object.
        Error::require(
            artifact.key() == session.work_object_id(),
            Error::WrongScope,
        )?;
        Error::require(
            artifact.payload_bytes() <= limits.text_bytes(),
            Error::BudgetExceeded,
        )?;
        let commit_status = artifact.commit_status();
        let motion = CriticalSpring::new(0.0, 0.45).map_err(Error::from)?;
        Ok(Self {
            session,
            lease: Some(lease),
            instance: Arc::new(()),
            generation,
            revision: 0,
            limits,
            artifact: Arc::new(artifact),
            commit_status,
            optimistic_status: None,
            inbox: VecDeque::with_capacity(limits.queued_commands()),
            motion,
            target: View::Source,
            scroll: [0; 3],
            reduced: false,
            dirty: true,
            revoked: false,
            prepared: None,
            presented: None,
            serial: 0,
            stats: Statistics::default(),
        })
    }

    /// Cancel active and queued optimistic overlays, advancing the revision
    /// past both so no accepted revision is ever reused.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Revoked`] on a revoked instance.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Artifact, Canvas, CommitStatus, Delta, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session.clone(), 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let base = canvas.next_base_revision()?;
    /// canvas.enqueue(Delta::new(session, 1, base, Action::OptimisticTransition(CommitStatus::Committed)))?;
    /// canvas.prepare(0.0, 900, 600, false)?;
    /// assert!(canvas.is_optimistic());
    /// canvas.deny_optimistic()?;
    /// assert!(!canvas.is_optimistic());
    /// assert_eq!(canvas.commit_status(), CommitStatus::Proposed);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn deny_optimistic(&mut self) -> Result<(), Error> {
        Error::require(!self.revoked, Error::Revoked)?;
        let cancelled = self
            .inbox
            .iter()
            .filter(|action| matches!(action, Action::OptimisticTransition(_)))
            .count();
        let cancelled_count = u64::try_from(cancelled)?;
        let overlay_count = u64::from(self.optimistic_status.is_some());
        let advance = cancelled_count
            .checked_add(overlay_count)
            .ok_or(Error::InvalidInput)?;
        if advance > 0 {
            let inbox_count = u64::try_from(self.inbox.len())?;
            let live_count = inbox_count
                .checked_sub(cancelled_count)
                .ok_or(Error::InvalidInput)?;
            let revision = self
                .revision
                .checked_add(advance)
                .ok_or(Error::InvalidInput)?;
            revision
                .checked_add(live_count)
                .ok_or(Error::InvalidInput)?;
            self.inbox
                .retain(|action| !matches!(action, Action::OptimisticTransition(_)));
            self.optimistic_status = None;
            self.revision = revision;
            self.dirty = true;
            self.presented = None;
            self.prepared = None;
        }
        Ok(())
    }

    /// Enqueue one ordering envelope after scope, freshness, and capacity
    /// checks. Rejections consume nothing: no revision, no inbox slot.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Revoked`], [`Error::WrongScope`], [`Error::Stale`],
    /// [`Error::QueueFull`], or [`Error::InvalidInput`] on overflow.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Artifact, Canvas, Delta, Error, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session.clone(), 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let delta = Delta::new(session, 1, 0, Action::Step(1));
    /// canvas.enqueue(delta.clone())?;
    /// assert_eq!(canvas.next_base_revision()?, 1);
    /// assert_eq!(canvas.enqueue(delta), Err(Error::Stale));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn enqueue(&mut self, delta: Delta) -> Result<(), Error> {
        let Delta {
            session,
            generation,
            base_revision,
            action,
        } = delta;
        Error::require(!self.revoked, Error::Revoked)?;
        Error::require(session == self.session, Error::WrongScope)?;
        Error::require(
            generation == self.generation && base_revision == self.next_base_revision()?,
            Error::Stale,
        )?;
        Error::require(
            self.inbox.len() != self.limits.queued_commands(),
            Error::QueueFull,
        )?;
        base_revision.checked_add(1).ok_or(Error::InvalidInput)?;
        self.inbox.push_back(action);
        self.stats.accepted = self.stats.accepted.saturating_add(1);
        Ok(())
    }

    /// Enqueue a local interaction against a presented token. A frame token
    /// authorizes local interaction, never a domain-status claim: transition
    /// actions are rejected here.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Revoked`], [`Error::Stale`], or [`Error::WrongScope`]
    /// for transition-family actions.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Artifact, Canvas, CommitStatus, Error, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// canvas.acknowledge_present(frame.token())?;
    /// canvas.input(frame.token(), Action::Step(1))?;
    /// let claim = canvas.input(frame.token(), Action::Transition(CommitStatus::Committed));
    /// assert_eq!(claim, Err(Error::WrongScope));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn input(&mut self, token: FrameToken, action: Action) -> Result<(), Error> {
        Error::require(!self.revoked, Error::Revoked)?;
        Error::require(self.presented == Some(token), Error::Stale)?;
        Error::require(
            !matches!(
                action,
                Action::Transition(_) | Action::OptimisticTransition(_) | Action::RevertOptimistic
            ),
            Error::WrongScope,
        )?;
        self.enqueue(Delta::new(
            self.session.clone(),
            self.generation,
            self.next_base_revision()?,
            action,
        ))
    }

    /// Drain the inbox in order. Upper bound is the fixed inbox limit: no
    /// actor, thread, or durable event per widget.
    fn drain(&mut self) -> Result<(), Error> {
        while let Some(action) = self.inbox.pop_front() {
            match action {
                Action::Navigate(view) => {
                    self.motion
                        .retarget(view_position(view))
                        .map_err(Error::from)?;
                    self.target = view;
                }
                Action::Step(amount) => {
                    let target = step_target(self.target, amount);
                    self.motion
                        .retarget(view_position(target))
                        .map_err(Error::from)?;
                    self.target = target;
                }
                Action::ToggleReducedMotion => self.reduced = !self.reduced,
                Action::ReducedMotion(reduced) => self.reduced = reduced,
                Action::Scroll { view, lines } => {
                    let max = self.artifact.text(view).lines().count().saturating_sub(1);
                    let max_line = i64::try_from(max)?;
                    let [source, proposal, evidence] = self.scroll.each_mut();
                    let slot = match view {
                        View::Source => source,
                        View::Proposal => proposal,
                        View::Evidence => evidence,
                    };
                    let current = i64::try_from(*slot)?;
                    let scrolled = current
                        .checked_add(i64::from(lines))
                        .ok_or(Error::InvalidInput)?;
                    let clamped = scrolled.clamp(0, max_line);
                    *slot = usize::try_from(clamped)?;
                }
                Action::Transition(status) => {
                    self.commit_status = status;
                    self.optimistic_status = None;
                }
                Action::OptimisticTransition(status) => {
                    self.optimistic_status = Some(status);
                }
                Action::RevertOptimistic => {
                    self.optimistic_status = None;
                }
            }
            if self.reduced {
                self.motion.snap();
            }
            self.revision = self.revision.checked_add(1).ok_or(Error::InvalidInput)?;
            self.stats.drained = self.stats.drained.saturating_add(1);
            self.dirty = true;
            self.presented = None;
        }
        Ok(())
    }

    /// Validate time and viewport BEFORE draining. A rejected frame cannot
    /// consume queued actions.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Revoked`] on a revoked instance or
    /// [`Error::InvalidInput`] for non-finite or over-60s time, or an
    /// over-budget viewport.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Error, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let frame = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?;
    /// assert_eq!((frame.width(), frame.height()), (900, 600));
    /// assert_eq!(canvas.prepare(f64::NAN, 900, 600, false).err(), Some(Error::InvalidInput));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn prepare(
        &mut self,
        seconds: f64,
        width: u32,
        height: u32,
        force: bool,
    ) -> Result<Option<Frame>, Error> {
        Error::require(!self.revoked, Error::Revoked)?;
        Error::require(
            seconds.is_finite() && (0.0..=60.0).contains(&seconds),
            Error::InvalidInput,
        )?;
        self.limits.pixel_count(width, height)?;
        if !force && !self.needs_frame() {
            return Ok(None);
        }
        let serial = self.serial.checked_add(1).ok_or(Error::InvalidInput)?;
        self.drain()?;
        if !self.motion.is_settled() {
            self.motion.advance(seconds).map_err(Error::from)?;
            self.stats.sampled = self.stats.sampled.saturating_add(1);
        }
        if self.motion.is_settled() {
            self.motion.snap();
        }
        self.serial = serial;
        let token = FrameToken {
            instance: Arc::clone(&self.instance),
            generation: self.generation,
            serial,
            revision: self.revision,
        };
        self.prepared = Some(token.clone());
        self.stats.prepared = self.stats.prepared.saturating_add(1);
        let mut frame = Frame::new(
            token,
            Arc::clone(&self.artifact),
            self.commit_status(),
            self.commit_status,
            self.is_optimistic(),
            self.motion.velocity(),
            self.motion.position(),
        );
        frame.set_target(self.target);
        frame.set_scroll(self.scroll);
        frame.set_viewport(width, height);
        frame.set_reduced_motion(self.reduced);
        frame.set_animating(!self.motion.is_settled());
        Ok(Some(frame))
    }

    /// Record successful presentation of a prepared token. The adapter calls
    /// this only AFTER actual presentation succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Revoked`] on a revoked instance or [`Error::Stale`]
    /// for a superseded or mismatched token.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Error, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let token = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?.token();
    /// canvas.acknowledge_present(token.clone())?;
    /// assert_eq!(canvas.presented_token(), Some(token.clone()));
    /// assert_eq!(canvas.acknowledge_present(token), Err(Error::Stale));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn acknowledge_present(&mut self, token: FrameToken) -> Result<(), Error> {
        Error::require(!self.revoked, Error::Revoked)?;
        Error::require(
            self.prepared.as_ref() == Some(&token) && token.revision == self.revision,
            Error::Stale,
        )?;
        self.presented = Some(token);
        self.prepared = None;
        self.dirty = false;
        self.stats.presented = self.stats.presented.saturating_add(1);
        Ok(())
    }

    /// Drop presented/prepared tokens and owe a fresh frame (surface lost).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let token = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?.token();
    /// canvas.acknowledge_present(token)?;
    /// canvas.invalidate_surface();
    /// assert!(canvas.presented_token().is_none());
    /// assert!(canvas.needs_frame());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn invalidate_surface(&mut self) {
        self.presented = None;
        self.prepared = None;
        self.dirty = true;
    }

    /// Snap motion and invalidate the surface: recover from a long idle sleep
    /// without replaying events.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Artifact, Canvas, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session, 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// let token = canvas.prepare(0.0, 900, 600, false)?.ok_or("frame owed")?.token();
    /// canvas.acknowledge_present(token)?;
    /// canvas.resume_without_replay();
    /// assert!(canvas.presented_token().is_none() && canvas.needs_frame());
    /// assert_eq!(canvas.revision(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn resume_without_replay(&mut self) {
        self.motion.snap();
        self.invalidate_surface();
    }

    /// Revoke the instance: clear the inbox and refuse everything after.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Artifact, Canvas, Delta, Error, Limits};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// # let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// # let mut canvas = Canvas::new(session.clone(), 1, Artifact::new(id, 1, "one\ntwo", None, None, Limits::default())?, Limits::default())?;
    /// canvas.revoke();
    /// let late = Delta::new(session, 1, 0, Action::Step(1));
    /// assert_eq!(canvas.enqueue(late), Err(Error::Revoked));
    /// assert!(!canvas.needs_frame());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn revoke(&mut self) {
        self.revoked = true;
        self.inbox.clear();
        self.presented = None;
        self.prepared = None;
    }
}

/// Spring position for a navigation target on the 0/1/2 view axis.
fn view_position(view: View) -> f64 {
    match view {
        View::Source => 0.0,
        View::Proposal => 1.0,
        View::Evidence => 2.0,
    }
}

/// Step the target by a signed amount with wraparound, using only
/// wrapping arithmetic (no remainder operator): `wrapping_rem` keeps the
/// dividend's sign, so a negative remainder is lifted back into range.
fn step_target(current: View, amount: i32) -> View {
    let base = match current {
        View::Source => 0_i64,
        View::Proposal => 1_i64,
        View::Evidence => 2_i64,
    };
    let shifted = base.wrapping_add(i64::from(amount));
    let reduced = shifted.wrapping_rem(3);
    let index = if reduced < 0 {
        reduced.wrapping_add(3)
    } else {
        reduced
    };
    match index {
        0 => View::Source,
        1 => View::Proposal,
        _ => View::Evidence,
    }
}

#[cfg(test)]
mod tests {
    use super::{View, step_target};

    /// Assert one step lands on the expected view.
    fn assert_step(current: View, amount: i32, want: View) -> Result<(), String> {
        let got = step_target(current, amount);
        if got != want {
            return Err(format!(
                "step {current:?} by {amount}: got {got:?}, want {want:?}"
            ));
        }
        Ok(())
    }

    /// Step arithmetic wraps exactly, forward and backward, for small and
    /// extreme amounts.
    #[test]
    fn step_target_wraps_like_euclidean_modulo_three() -> Result<(), String> {
        assert_step(View::Source, 1, View::Proposal)?;
        assert_step(View::Proposal, 1, View::Evidence)?;
        assert_step(View::Evidence, 1, View::Source)?;
        assert_step(View::Source, -1, View::Evidence)?;
        assert_step(View::Evidence, -1, View::Proposal)?;
        assert_step(View::Source, 3, View::Source)?;
        assert_step(View::Source, -3, View::Source)?;
        assert_step(View::Proposal, 4, View::Evidence)?;
        assert_step(View::Evidence, -4, View::Proposal)?;
        assert_step(View::Source, 0, View::Source)?;
        assert_step(View::Source, i32::MAX, View::Proposal)?;
        assert_step(View::Source, i32::MIN, View::Proposal)?;
        assert_step(View::Evidence, i32::MAX, View::Source)?;
        Ok(())
    }
}
