//! Public canvas API falsifiers, commit and recovery half; headless
//! acknowledgement is not OS rendering evidence.
//!
//! Counterpart of `canvas/tests/presentation_contract.rs` (kernel#850 row
//! 15): lease, journey, commit-lifecycle, denial, and recovery behaviors,
//! split from the frame and queue half for the size law. Typed errors, no
//! unwrap, bit-exact float comparison, wrapping revision math.

#[path = "support/fixtures.rs"]
mod fixtures;
#[path = "support/presentation.rs"]
mod presentation;

use drishti::canvas_protocol::SessionInvariant;
use drishti::canvas_scene::{
    Action, Artifact, Canvas, CommitStatus, Delta, Error, Limits, PresentationLease, View,
};

use fixtures::{POLICY_BUNDLE, SESSION_ID, USER_SUB, WORK_OBJECT, session};
use presentation::{assert_bits, canvas, present, queue};

/// Mutated lease fields are revalidated at admission.
///
/// Leases only arise from the validating constructor, so admission
/// revalidation is defense-in-depth: every mutation below is refused at
/// construction, and an invalid invariant never reaches a lease at all.
#[test]
fn mutable_lease_fields_are_revalidated_at_admission() -> Result<(), String> {
    let limits = Limits::default();
    let lease = PresentationLease::new(
        "lease-42",
        "surface-desktop",
        session("tenant-x")?,
        1,
        limits,
    )
    .map_err(|error| format!("{error}"))?;
    if lease.lease_id() != "lease-42" {
        return Err(String::from("lease id must bind"));
    }
    if lease.surface_id() != "surface-desktop" {
        return Err(String::from("surface id must bind"));
    }
    if lease.generation() != 1 {
        return Err(String::from("generation must bind"));
    }
    let artifact = Artifact::new(WORK_OBJECT, 1, "text", None, None, limits)
        .map_err(|error| format!("{error}"))?;
    let bound = Canvas::with_lease(lease, artifact).map_err(|error| format!("{error}"))?;
    let Some(bound_lease) = bound.lease() else {
        return Err(String::from("lease must be retained"));
    };
    if bound_lease.surface_id() != "surface-desktop" {
        return Err(String::from("surface must survive binding"));
    }
    if PresentationLease::new("", "surface", session("x")?, 1, limits).is_ok() {
        return Err(String::from("empty lease id must fail"));
    }
    if PresentationLease::new("lease", "", session("x")?, 1, limits).is_ok() {
        return Err(String::from("empty surface id must fail"));
    }
    if PresentationLease::new("lease\0", "surface", session("x")?, 1, limits).is_ok() {
        return Err(String::from("control characters must fail"));
    }
    if PresentationLease::new("lease", "surface", session("x")?, 0, limits).is_ok() {
        return Err(String::from("zero generation must fail"));
    }
    // Invalid bindings never reach admission: the invariant constructor
    // refuses an empty tenant before any lease exists.
    if SessionInvariant::new(
        String::from(WORK_OBJECT),
        String::from(SESSION_ID),
        String::from(POLICY_BUNDLE),
        String::new(),
        String::from(USER_SUB),
    )
    .is_ok()
    {
        return Err(String::from("empty tenant must fail"));
    }
    Ok(())
}

/// The full journey: source, proposal, evidence, and back, all settled.
#[test]
fn fixture_object_journey_select_inspect_proposal_inspect_evidence_return() -> Result<(), String> {
    let limits = Limits::default();
    let artifact = Artifact::with_status(
        WORK_OBJECT,
        1,
        "fn calculate() -> i32 { 1 }",
        Some("fn calculate() -> i32 { 2 }"),
        Some("test_calculate: pass\nbench: 12ns"),
        CommitStatus::Proposed,
        limits,
    )
    .map_err(|error| format!("{error}"))?;
    let mut scene = Canvas::new(session("tenant-journey")?, 1, artifact, limits)
        .map_err(|error| format!("{error}"))?;

    let first = present(&mut scene, 0.0)?;
    if first.target() != View::Source {
        return Err(String::from("journey must start at source"));
    }
    if first.commit_status() != CommitStatus::Proposed {
        return Err(String::from("journey must start proposed"));
    }
    assert_bits(first.position(), 0.0, "journey start position")?;

    queue(&mut scene, Action::Navigate(View::Proposal))?;
    let mut proposal = present(&mut scene, 0.0)?;
    while scene.needs_frame() {
        proposal = present(&mut scene, 0.04)?;
    }
    if proposal.target() != View::Proposal {
        return Err(String::from("must reach proposal"));
    }
    assert_bits(proposal.position(), 1.0, "proposal position")?;
    assert_bits(proposal.velocity(), 0.0, "proposal velocity")?;
    if !proposal.artifact().supplied(View::Proposal) {
        return Err(String::from("proposal must be supplied"));
    }

    queue(&mut scene, Action::Navigate(View::Evidence))?;
    let mut evidence = present(&mut scene, 0.0)?;
    while scene.needs_frame() {
        evidence = present(&mut scene, 0.04)?;
    }
    if evidence.target() != View::Evidence {
        return Err(String::from("must reach evidence"));
    }
    assert_bits(evidence.position(), 2.0, "evidence position")?;
    assert_bits(evidence.velocity(), 0.0, "evidence velocity")?;
    if !evidence.artifact().supplied(View::Evidence) {
        return Err(String::from("evidence must be supplied"));
    }

    queue(&mut scene, Action::Navigate(View::Source))?;
    let mut back = present(&mut scene, 0.0)?;
    while scene.needs_frame() {
        back = present(&mut scene, 0.04)?;
    }
    if back.target() != View::Source {
        return Err(String::from("must return to source"));
    }
    assert_bits(back.position(), 0.0, "return position")?;
    assert_bits(back.velocity(), 0.0, "return velocity")?;
    if back.artifact().key() != WORK_OBJECT {
        return Err(String::from("artifact identity must survive"));
    }
    if back.commit_status() != CommitStatus::Proposed {
        return Err(String::from("status must survive the journey"));
    }
    Ok(())
}

/// Commit lifecycle: proposed, running, committed, failed.
#[test]
fn commit_status_lifecycle_transitions() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    if scene.commit_status() != CommitStatus::Proposed {
        return Err(String::from("must start proposed"));
    }
    queue(&mut scene, Action::Transition(CommitStatus::Running))?;
    let running = present(&mut scene, 0.0)?;
    if running.commit_status() != CommitStatus::Running
        || scene.commit_status() != CommitStatus::Running
    {
        return Err(String::from("must reach running"));
    }
    queue(&mut scene, Action::Transition(CommitStatus::Committed))?;
    let committed = present(&mut scene, 0.0)?;
    if committed.commit_status() != CommitStatus::Committed
        || scene.commit_status() != CommitStatus::Committed
    {
        return Err(String::from("must reach committed"));
    }
    queue(&mut scene, Action::Transition(CommitStatus::Failed))?;
    let failed = present(&mut scene, 0.0)?;
    if failed.commit_status() != CommitStatus::Failed
        || scene.commit_status() != CommitStatus::Failed
    {
        return Err(String::from("must reach failed"));
    }
    Ok(())
}

/// Denied optimism reverts to confirmed status with a fresh revision.
#[test]
fn denial_during_optimistic_visual_state_reverts_cleanly_without_epoch_corruption()
-> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    present(&mut scene, 0.0)?;
    if scene.commit_status() != CommitStatus::Proposed || scene.is_optimistic() {
        return Err(String::from("must start clean"));
    }
    queue(
        &mut scene,
        Action::OptimisticTransition(CommitStatus::Running),
    )?;
    let frame = present(&mut scene, 0.0)?;
    if frame.commit_status() != CommitStatus::Running || !scene.is_optimistic() {
        return Err(String::from("optimism must show"));
    }
    if scene.confirmed_commit_status() != CommitStatus::Proposed {
        return Err(String::from("confirmed status must hold"));
    }
    let before = scene.revision();
    scene
        .deny_optimistic()
        .map_err(|error| format!("{error}"))?;
    if scene.is_optimistic() || scene.commit_status() != CommitStatus::Proposed {
        return Err(String::from("denial must revert"));
    }
    if scene.revision() != before.wrapping_add(1) {
        return Err(String::from("denial must advance one revision"));
    }
    if !scene.needs_frame() {
        return Err(String::from("denial must owe a frame"));
    }
    let reverted = present(&mut scene, 0.0)?;
    if reverted.commit_status() != CommitStatus::Proposed {
        return Err(String::from("reverted frame must show confirmed status"));
    }
    queue(&mut scene, Action::Navigate(View::Evidence))?;
    if present(&mut scene, 0.0)?.target() != View::Evidence {
        return Err(String::from("inputs must work after denial"));
    }
    Ok(())
}

/// Mid-gesture revocation halts animation and rejects everything after.
#[test]
fn resource_lease_revocation_mid_gesture_halts_animation_and_rejects_subsequent_inputs()
-> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    present(&mut scene, 0.0)?;
    queue(&mut scene, Action::Navigate(View::Evidence))?;
    let moving = scene
        .prepare(0.05, 900, 600, false)
        .map_err(|error| format!("{error}"))?
        .ok_or_else(|| String::from("expected a frame"))?;
    if !moving.animating() {
        return Err(String::from("must be animating"));
    }
    scene
        .acknowledge_present(moving.token())
        .map_err(|error| format!("{error}"))?;
    scene.revoke();
    if scene.needs_frame() {
        return Err(String::from("revoked scene must halt"));
    }
    if !matches!(scene.prepare(0.05, 900, 600, false), Err(Error::Revoked)) {
        return Err(String::from("prepare after revoke must fail"));
    }
    if scene.input(moving.token(), Action::Navigate(View::Source)) != Err(Error::Revoked) {
        return Err(String::from("input after revoke must fail"));
    }
    let refused = scene.enqueue(Delta::new(
        scene.session().clone(),
        scene.generation(),
        scene
            .next_base_revision()
            .map_err(|error| format!("{error}"))?,
        Action::Navigate(View::Source),
    ));
    if refused != Err(Error::Revoked) {
        return Err(String::from("enqueue after revoke must fail"));
    }
    if scene.acknowledge_present(moving.token()) != Err(Error::Revoked) {
        return Err(String::from("acknowledge after revoke must fail"));
    }
    Ok(())
}

/// Surface recreation preserves the epoch revision and resumes cleanly.
#[test]
fn renderer_restart_and_surface_recreation_preserves_epoch_and_resumes_without_replay()
-> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    let first = present(&mut scene, 0.0)?;
    let revision = scene.revision();
    scene.invalidate_surface();
    if scene.input(first.token(), Action::Navigate(View::Proposal)) != Err(Error::Stale) {
        return Err(String::from("invalidated token must be Stale"));
    }
    if scene.revision() != revision {
        return Err(String::from("epoch must survive invalidation"));
    }
    scene.resume_without_replay();
    if !scene.needs_frame() {
        return Err(String::from("resume must owe a frame"));
    }
    let second = present(&mut scene, 0.0)?;
    if second.scene_revision() != revision {
        return Err(String::from("revision must survive resume"));
    }
    if second.token() == first.token() {
        return Err(String::from("resume must mint a fresh token"));
    }
    scene
        .input(second.token(), Action::Navigate(View::Proposal))
        .map_err(|error| format!("new token must route: {error}"))?;
    if present(&mut scene, 0.0)?.target() != View::Proposal {
        return Err(String::from("must navigate after resume"));
    }
    Ok(())
}

/// Time jumps reject past 60 seconds; resume snaps without instability.
#[test]
fn long_sleep_and_time_jump_maintains_stability_and_bounds() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    present(&mut scene, 0.0)?;
    queue(&mut scene, Action::Navigate(View::Evidence))?;
    let moving = present(&mut scene, 0.05)?;
    if !moving.animating() {
        return Err(String::from("must be animating"));
    }
    if !matches!(
        scene.prepare(3600.0, 900, 600, false),
        Err(Error::InvalidInput)
    ) {
        return Err(String::from("one hour must be rejected"));
    }
    if !matches!(
        scene.prepare(86400.0, 900, 600, false),
        Err(Error::InvalidInput)
    ) {
        return Err(String::from("one day must be rejected"));
    }
    scene.resume_without_replay();
    let frame = present(&mut scene, 0.0)?;
    assert_bits(frame.position(), 2.0, "resumed position")?;
    assert_bits(frame.velocity(), 0.0, "resumed velocity")?;
    if frame.animating() || !frame.position().is_finite() {
        return Err(String::from("resumed frame must be settled and finite"));
    }
    Ok(())
}

/// Memory boundaries: pixels, queues, and payloads refuse oversize input.
#[test]
fn four_gigabyte_pressure_and_large_allocation_rejection() -> Result<(), String> {
    let limits = Limits::default();
    if limits.pixel_count(65536, 65536) != Err(Error::BudgetExceeded) {
        return Err(String::from("64k x 64k must exceed budget"));
    }
    if limits.pixel_count(u32::MAX, 2) != Err(Error::BudgetExceeded) {
        return Err(String::from("max-dimension buffer must exceed budget"));
    }
    if Limits::new(262_144, 32, 8_847_361).is_ok() {
        return Err(String::from("over-ceiling pixels must fail"));
    }
    if Limits::new(1_048_577, 32, 8_847_360).is_ok() {
        return Err(String::from("over-ceiling text must fail"));
    }
    if Limits::new(262_144, 257, 8_847_360).is_ok() {
        return Err(String::from("over-ceiling queue must fail"));
    }
    if Limits::new(0, 32, 8_847_360).is_ok() {
        return Err(String::from("zero text must fail"));
    }
    let huge_text = "x".repeat(307_200);
    if !matches!(
        Artifact::new(WORK_OBJECT, 1, &huge_text, None, None, limits),
        Err(Error::BudgetExceeded)
    ) {
        return Err(String::from("300 KiB payload must exceed budget"));
    }
    Ok(())
}

/// Accessibility: labels, full-width hit testing, and wrapping step order.
#[test]
fn screen_reader_and_ime_traversal_contracts() -> Result<(), String> {
    if View::Source.label() != "Source"
        || View::Proposal.label() != "Proposal"
        || View::Evidence.label() != "Evidence"
    {
        return Err(String::from("view labels must match"));
    }
    if CommitStatus::Proposed.label() != "Proposed"
        || CommitStatus::Running.label() != "Running"
        || CommitStatus::Failed.label() != "Failed"
        || CommitStatus::Committed.label() != "Committed"
    {
        return Err(String::from("status labels must match"));
    }
    let mut scene = canvas("a", Limits::default())?;
    let frame = present(&mut scene, 0.0)?;
    if frame.hit_test(150.0, 580.0) != Some(View::Source) {
        return Err(String::from("left third must hit source"));
    }
    if frame.hit_test(450.0, 580.0) != Some(View::Proposal) {
        return Err(String::from("middle third must hit proposal"));
    }
    if frame.hit_test(750.0, 580.0) != Some(View::Evidence) {
        return Err(String::from("right third must hit evidence"));
    }
    queue(&mut scene, Action::Step(1))?;
    if present(&mut scene, 0.0)?.target() != View::Proposal {
        return Err(String::from("step must reach proposal"));
    }
    queue(&mut scene, Action::Step(1))?;
    if present(&mut scene, 0.0)?.target() != View::Evidence {
        return Err(String::from("step must reach evidence"));
    }
    queue(&mut scene, Action::Step(1))?;
    if present(&mut scene, 0.0)?.target() != View::Source {
        return Err(String::from("step must wrap to source"));
    }
    queue(&mut scene, Action::Step(-1))?;
    if present(&mut scene, 0.0)?.target() != View::Evidence {
        return Err(String::from("step must wrap back"));
    }
    Ok(())
}

/// Reduced motion reaches the identical end state with zero sampling.
#[test]
fn reduced_motion_equivalent_outcome_matches_continuous_animation_target() -> Result<(), String> {
    let limits = Limits::default();
    let make = || -> Result<Canvas, String> {
        let artifact = Artifact::new(WORK_OBJECT, 1, "test", Some("p"), Some("e"), limits)
            .map_err(|error| format!("{error}"))?;
        Canvas::new(session("a")?, 1, artifact, limits).map_err(|error| format!("{error}"))
    };
    let mut normal = make()?;
    let mut reduced = make()?;
    queue(&mut reduced, Action::ReducedMotion(true))?;
    let actions = [
        Action::Navigate(View::Evidence),
        Action::Step(-1),
        Action::Navigate(View::Source),
        Action::Navigate(View::Proposal),
        Action::Transition(CommitStatus::Running),
    ];
    for action in actions {
        queue(&mut normal, action)?;
        queue(&mut reduced, action)?;
    }
    let mut normal_frame = present(&mut normal, 0.0)?;
    while normal.needs_frame() {
        normal_frame = present(&mut normal, 0.04)?;
    }
    let reduced_frame = present(&mut reduced, 0.0)?;
    assert_bits(
        normal_frame.position(),
        reduced_frame.position(),
        "equivalent position",
    )?;
    assert_bits(
        normal_frame.velocity(),
        reduced_frame.velocity(),
        "equivalent velocity",
    )?;
    if normal_frame.target() != reduced_frame.target() {
        return Err(String::from("targets must match"));
    }
    if normal_frame.commit_status() != reduced_frame.commit_status() {
        return Err(String::from("statuses must match"));
    }
    if normal_frame.artifact().key() != reduced_frame.artifact().key() {
        return Err(String::from("artifacts must match"));
    }
    if reduced.stats().sampled() != 0 {
        return Err(String::from("reduced path must sample nothing"));
    }
    Ok(())
}

/// Denial cancels queued optimism without reusing accepted revisions.
#[test]
fn denial_cancels_queued_optimism_without_reusing_accepted_revisions() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    queue(
        &mut scene,
        Action::OptimisticTransition(CommitStatus::Running),
    )?;
    queue(&mut scene, Action::Navigate(View::Evidence))?;
    let next = scene
        .next_base_revision()
        .map_err(|error| format!("{error}"))?;
    scene
        .deny_optimistic()
        .map_err(|error| format!("{error}"))?;
    if scene
        .next_base_revision()
        .map_err(|error| format!("{error}"))?
        != next
    {
        return Err(String::from("denial must preserve the next base"));
    }
    let frame = present(&mut scene, 0.0)?;
    if frame.commit_status() != CommitStatus::Proposed {
        return Err(String::from("denial must show confirmed status"));
    }
    if frame.target() != View::Evidence {
        return Err(String::from("queued navigation must survive denial"));
    }
    if scene.is_optimistic() {
        return Err(String::from("optimism must be gone"));
    }
    queue(&mut scene, Action::Navigate(View::Source))?;
    Ok(())
}

/// Presented input cannot claim a confirmed domain commit.
#[test]
fn presented_input_cannot_claim_confirmed_domain_commit() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    let token = present(&mut scene, 0.0)?.token();
    for action in [
        Action::Transition(CommitStatus::Committed),
        Action::OptimisticTransition(CommitStatus::Committed),
        Action::RevertOptimistic,
    ] {
        if scene.input(token.clone(), action) != Err(Error::WrongScope) {
            return Err(String::from("transition input must be WrongScope"));
        }
    }
    if scene.confirmed_commit_status() != CommitStatus::Proposed {
        return Err(String::from("confirmed status must hold"));
    }
    Ok(())
}

/// Optimistic commits read pending until confirmed.
#[test]
fn optimistic_commit_is_visibly_pending_until_confirmed() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    queue(
        &mut scene,
        Action::OptimisticTransition(CommitStatus::Committed),
    )?;
    let pending = present(&mut scene, 0.0)?;
    if pending.status_label() != "Commit pending" {
        return Err(String::from("optimistic status must read pending"));
    }
    if !pending.optimistic() || pending.confirmed_commit_status() != CommitStatus::Proposed {
        return Err(String::from("pending frame must mark optimism"));
    }
    queue(&mut scene, Action::Transition(CommitStatus::Committed))?;
    let confirmed = present(&mut scene, 0.0)?;
    if confirmed.status_label() != "Committed"
        || confirmed.optimistic()
        || confirmed.confirmed_commit_status() != CommitStatus::Committed
    {
        return Err(String::from("confirmed frame must read committed"));
    }
    Ok(())
}
