//! Shared kit for the canvas scene sims: scene builders, a hostile action
//! generator, and a reference reducer.
//!
//! The reference reducer is written from the documented behaviour of the
//! scene (ordered inbox, revision per drained action, clamped scroll,
//! modulo-three stepping, optimistic overlay), not from the scene's code, so
//! a sim that compares the two catches a reducer that drifts from the spec.
//! Every helper here is used by at least one canvas sim in the `sim` target.

use std::collections::VecDeque;

use drishti::canvas_scene::{Action, Artifact, Canvas, CommitStatus, Delta, Frame, Limits, View};

use crate::motion_kit::{describe, ensure};
use crate::rng::SimRng;

/// Four-line source lens shared by every kit scene.
pub const SOURCE: &str = "alpha\nbeta\ngamma\ndelta";

/// Two-line proposal lens shared by every kit scene.
pub const PROPOSAL: &str = "p1\np2";

/// An envelope with the given queue bound and the default text and pixel
/// ceilings.
pub fn limits(queued_commands: usize) -> Result<Limits, String> {
    Limits::new(262_144, queued_commands, 8_847_360).map_err(describe)
}

/// The kit artifact: four-line source, two-line proposal, absent evidence.
pub fn artifact(limits: Limits) -> Result<Artifact, String> {
    Artifact::new(
        crate::fixtures::WORK_OBJECT,
        1,
        SOURCE,
        Some(PROPOSAL),
        None,
        limits,
    )
    .map_err(describe)
}

/// A scene for one tenant in the fixed session.
pub fn scene(tenant: &str, limits: Limits) -> Result<Canvas, String> {
    Canvas::new(
        crate::fixtures::session(tenant)?,
        1,
        artifact(limits)?,
        limits,
    )
    .map_err(describe)
}

/// An envelope for the scene at its next accepted base revision.
pub fn delta(scene: &Canvas, action: Action) -> Result<Delta, String> {
    Ok(Delta::new(
        scene.session().clone(),
        scene.generation(),
        scene.next_base_revision().map_err(describe)?,
        action,
    ))
}

/// Prepare one forced frame at 900 by 600 and acknowledge its presentation.
pub fn present(scene: &mut Canvas, seconds: f64) -> Result<Frame, String> {
    let frame = scene
        .prepare(seconds, 900, 600, true)
        .map_err(describe)?
        .ok_or("a forced prepare must yield a frame")?;
    scene.acknowledge_present(frame.token()).map_err(describe)?;
    Ok(frame)
}

/// A hostile `i32`: extremes, zero, minus one, or a small signed value.
pub fn wild_i32(random: &mut SimRng) -> i32 {
    match random.below(6) {
        0 => i32::MIN,
        1 => i32::MAX,
        2 => 0,
        3 => -1,
        _ => random.below_i32(41).wrapping_sub(20),
    }
}

/// A seeded action drawn from every variant, transitions included.
pub fn draw_action(random: &mut SimRng) -> Action {
    let view = View::ALL[random.below_len(View::ALL.len())];
    let status = CommitStatus::ALL[random.below_len(CommitStatus::ALL.len())];
    match random.below(9) {
        0 => Action::Navigate(view),
        1 => Action::Step(wild_i32(random)),
        2 => Action::ToggleReducedMotion,
        3 => Action::ReducedMotion(random.below(2) == 0),
        4 | 5 => Action::scroll(view, wild_i32(random)),
        6 => Action::Transition(status),
        7 => Action::OptimisticTransition(status),
        _ => Action::RevertOptimistic,
    }
}

/// Compare the scene with the model on every state both expose.
pub fn agrees(scene: &Canvas, model: &Model, label: &str) -> Result<(), String> {
    let (accepted, drained) = model.counts();
    ensure(
        scene.revision() == model.revision()
            && scene.next_base_revision() == Ok(model.next_base())
            && scene.stats().accepted() == accepted
            && scene.stats().drained() == drained
            && scene.target() == model.target()
            && scene.reduced_motion() == model.reduced()
            && scene.commit_status() == model.visible()
            && scene.confirmed_commit_status() == model.confirmed()
            && scene.is_optimistic() == model.is_optimistic(),
        &format!("{label}: the scene left the reference model"),
    )
}

/// Reference reducer: the documented scene behaviour, written independently.
pub struct Model {
    /// Scene revision: one per drained action, one per cancelled overlay.
    revision: u64,
    /// Pending actions in arrival order.
    queue: VecDeque<Action>,
    /// Count of pending actions, kept beside the queue.
    pending: u64,
    /// Navigation target.
    target: View,
    /// Scroll offsets in view-index order.
    scroll: [usize; 3],
    /// Reduced-motion mode.
    reduced: bool,
    /// Confirmed domain status.
    status: CommitStatus,
    /// Optimistic overlay, if any.
    overlay: Option<CommitStatus>,
    /// Deltas accepted so far.
    accepted: u64,
    /// Actions drained so far.
    drained: u64,
    /// Line count of each lens, in view-index order.
    lines: [usize; 3],
}

impl Model {
    /// A fresh model over the kit artifact (4, 2 and 1 lines).
    pub fn new() -> Self {
        Self {
            revision: 0,
            queue: VecDeque::new(),
            pending: 0,
            target: View::Source,
            scroll: [0; 3],
            reduced: false,
            status: CommitStatus::Proposed,
            overlay: None,
            accepted: 0,
            drained: 0,
            lines: [4, 2, 1],
        }
    }

    /// Scene revision the model expects.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The base revision the next accepted delta must name.
    pub fn next_base(&self) -> u64 {
        self.revision.saturating_add(self.pending)
    }

    /// Count of actions waiting in the model's queue.
    pub fn pending(&self) -> u64 {
        self.pending
    }

    /// Navigation target the model expects.
    pub fn target(&self) -> View {
        self.target
    }

    /// Scroll offsets the model expects.
    pub fn scroll(&self) -> [usize; 3] {
        self.scroll
    }

    /// Reduced-motion mode the model expects.
    pub fn reduced(&self) -> bool {
        self.reduced
    }

    /// Confirmed status the model expects.
    pub fn confirmed(&self) -> CommitStatus {
        self.status
    }

    /// Whether the model has an overlay active.
    pub fn is_optimistic(&self) -> bool {
        self.overlay.is_some()
    }

    /// Deltas accepted and actions drained so far.
    pub fn counts(&self) -> (u64, u64) {
        (self.accepted, self.drained)
    }

    /// Accept one action into the queue.
    pub fn push(&mut self, action: Action) {
        self.queue.push_back(action);
        self.pending = self.pending.saturating_add(1);
        self.accepted = self.accepted.saturating_add(1);
    }

    /// The status a frame shows: the overlay when one is active.
    pub fn visible(&self) -> CommitStatus {
        match self.overlay {
            Some(status) => status,
            None => self.status,
        }
    }

    /// Drain the whole queue in order, as one prepared frame does.
    pub fn drain(&mut self) -> Result<(), String> {
        while let Some(action) = self.queue.pop_front() {
            match action {
                Action::Navigate(view) => self.target = view,
                Action::Step(amount) => {
                    let here = i64::try_from(self.target.index()).map_err(describe)?;
                    let there = here.saturating_add(i64::from(amount)).rem_euclid(3);
                    let index = usize::try_from(there).map_err(describe)?;
                    self.target = View::ALL[index];
                }
                Action::ToggleReducedMotion => self.reduced = !self.reduced,
                Action::ReducedMotion(reduced) => self.reduced = reduced,
                Action::Scroll { view, lines } => {
                    let slot = view.index();
                    let last =
                        i64::try_from(self.lines[slot].saturating_sub(1)).map_err(describe)?;
                    let here = i64::try_from(self.scroll[slot]).map_err(describe)?;
                    let moved = here.saturating_add(i64::from(lines)).clamp(0, last);
                    self.scroll[slot] = usize::try_from(moved).map_err(describe)?;
                }
                Action::Transition(status) => {
                    self.status = status;
                    self.overlay = None;
                }
                Action::OptimisticTransition(status) => self.overlay = Some(status),
                Action::RevertOptimistic => self.overlay = None,
                _ => ensure(false, "the kit has no model for this action")?,
            }
            self.revision = self.revision.saturating_add(1);
            self.pending = self.pending.saturating_sub(1);
            self.drained = self.drained.saturating_add(1);
        }
        Ok(())
    }

    /// Cancel queued and active optimistic overlays, advancing the revision
    /// past each so no accepted revision is reused.
    pub fn deny(&mut self) {
        let overlay = u64::from(self.overlay.is_some());
        let mut queued = 0_u64;
        self.queue.retain(|action| {
            let optimistic = matches!(*action, Action::OptimisticTransition(_));
            if optimistic {
                queued = queued.saturating_add(1);
            }
            !optimistic
        });
        self.overlay = None;
        self.revision = self.revision.saturating_add(queued).saturating_add(overlay);
        self.pending = self.pending.saturating_sub(queued);
    }
}
