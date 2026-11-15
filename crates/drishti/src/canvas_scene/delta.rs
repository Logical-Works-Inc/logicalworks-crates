//! The data-plane ordering envelope and the reducer counters.

use super::model::Action;
use crate::canvas_protocol::SessionInvariant;

/// Data-plane ordering envelope. Caller authentication belongs upstream:
/// matching strings is not authentication.
///
/// Fields are private: construction runs through [`Delta::new`]; the reducer
/// reads them in this module and admission compares them through the
/// accessors.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::SessionInvariant;
/// use drishti::canvas_scene::{Action, Delta, View};
///
/// let id = "11111111-1111-1111-1111-111111111111";
/// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
/// let delta = Delta::new(session, 1, 0, Action::Navigate(View::Proposal));
/// assert_eq!((delta.generation(), delta.base_revision()), (1, 0));
/// assert_eq!(delta.action(), Action::Navigate(View::Proposal));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Delta {
    /// Session the delta claims.
    pub(super) session: SessionInvariant,
    /// Instance generation the delta targets.
    pub(super) generation: u64,
    /// Expected base revision.
    pub(super) base_revision: u64,
    /// Action to enqueue.
    pub(super) action: Action,
}

impl Delta {
    /// Assemble an ordering envelope.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::SessionInvariant;
    /// use drishti::canvas_scene::{Action, Delta};
    ///
    /// let id = "11111111-1111-1111-1111-111111111111";
    /// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// let delta = Delta::new(session.clone(), 2, 5, Action::ToggleReducedMotion);
    /// assert_eq!(delta.session(), &session);
    /// assert_eq!(delta.generation(), 2);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn new(
        session: SessionInvariant,
        generation: u64,
        base_revision: u64,
        action: Action,
    ) -> Self {
        Self {
            session,
            generation,
            base_revision,
            action,
        }
    }

    /// Session the delta claims; the canvas refuses it unless it equals its own.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Delta};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "tenant-q".into(), id.into())?;
    /// let delta = Delta::new(session, 1, 0, Action::Step(1));
    /// assert_eq!(delta.session().tenant_id(), "tenant-q");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn session(&self) -> &SessionInvariant {
        &self.session
    }

    /// Instance generation the delta targets; any other generation is stale.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Delta};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// let delta = Delta::new(session, 9, 0, Action::Step(1));
    /// assert_eq!(delta.generation(), 9);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Revision the sender expects the canvas to be at once the delta's turn
    /// comes; a mismatch is stale.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Delta};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// let delta = Delta::new(session, 1, 6, Action::Step(1));
    /// assert_eq!(delta.base_revision(), 6);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn base_revision(&self) -> u64 {
        self.base_revision
    }

    /// Interaction the reducer applies when it drains this delta.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::SessionInvariant;
    /// # use drishti::canvas_scene::{Action, Delta};
    /// # let id = "11111111-1111-1111-1111-111111111111";
    /// let session = SessionInvariant::new(id.into(), id.into(), id.into(), "t".into(), id.into())?;
    /// let delta = Delta::new(session, 1, 0, Action::ReducedMotion(true));
    /// assert_eq!(delta.action(), Action::ReducedMotion(true));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn action(&self) -> Action {
        self.action
    }
}

/// Reducer counters.
///
/// Fields are private: the reducer mutates them through
/// [`Canvas`](super::Canvas) and readers go through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::canvas_scene::Statistics;
///
/// let stats = Statistics::default();
/// assert_eq!((stats.accepted(), stats.drained(), stats.sampled()), (0, 0, 0));
/// ```
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct Statistics {
    /// Deltas accepted into the inbox.
    pub(super) accepted: u64,
    /// Actions drained from the inbox.
    pub(super) drained: u64,
    /// Motion samples integrated.
    pub(super) sampled: u64,
    /// Frames prepared.
    pub(super) prepared: u64,
    /// Frames acknowledged as presented.
    pub(super) presented: u64,
}

impl Statistics {
    /// Deltas accepted into the inbox (rejections are not counted).
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::Statistics;
    ///
    /// assert_eq!(Statistics::default().accepted(), 0);
    /// ```
    #[must_use]
    pub const fn accepted(&self) -> u64 {
        self.accepted
    }

    /// Actions drained from the inbox; never exceeds `accepted`.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::Statistics;
    ///
    /// let stats = Statistics::default();
    /// assert!(stats.drained() <= stats.accepted());
    /// ```
    #[must_use]
    pub const fn drained(&self) -> u64 {
        self.drained
    }

    /// Motion samples integrated while preparing frames.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::Statistics;
    ///
    /// assert_eq!(Statistics::default().sampled(), 0);
    /// ```
    #[must_use]
    pub const fn sampled(&self) -> u64 {
        self.sampled
    }

    /// Frames the reducer has prepared; at least as many as were presented.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::Statistics;
    ///
    /// let stats = Statistics::default();
    /// assert!(stats.prepared() >= stats.presented());
    /// ```
    #[must_use]
    pub const fn prepared(&self) -> u64 {
        self.prepared
    }

    /// Frames the adapter acknowledged as actually presented.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_scene::Statistics;
    ///
    /// assert_eq!(Statistics::default().presented(), 0);
    /// ```
    #[must_use]
    pub const fn presented(&self) -> u64 {
        self.presented
    }
}
