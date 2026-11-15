//! Public canvas API falsifiers; headless acknowledgement is not OS
//! rendering evidence.
//!
//! Counterpart of `canvas/tests/presentation_contract.rs` (kernel#850 row
//! 15): all 37 kernel behaviors, ported to the crate's lint contract (typed
//! errors, no unwrap, bit-exact float comparison, wrapping revision math).
//! Frame, queue, token, and navigation cases live here; commit, lease,
//! journey, and recovery cases live in the commit half.

#[path = "support/fixtures.rs"]
mod fixtures;
#[path = "support/presentation.rs"]
mod presentation;

use drishti::canvas_protocol::SessionInvariant;
use drishti::canvas_scene::{Action, Artifact, Canvas, Delta, Error, Limits, View};

use fixtures::{WORK_OBJECT, session, session_in};
use presentation::{assert_bits, canvas, present, queue};

/// Artifact fixture over an explicit key and seed text.
fn digest_artifact(key: &str, text: &str, limits: Limits) -> Result<Artifact, String> {
    Artifact::new(key, 1, text, None, None, limits).map_err(|error| format!("{error}"))
}

/// Tenant scene over an explicit session and artifact at generation one.
fn tenant_scene(session: SessionInvariant, artifact: Artifact) -> Result<Canvas, String> {
    Canvas::new(session, 1, artifact, Limits::default()).map_err(|error| format!("{error}"))
}

/// An acknowledged static scene stops requesting frames with zero sampling.
#[test]
fn static_scene_stops_requesting_frames_after_acknowledgement() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    present(&mut scene, 0.0)?;
    if scene.needs_frame() {
        return Err(String::from("static scene must not need frames"));
    }
    let idle = scene
        .prepare(1.0, 900, 600, false)
        .map_err(|error| format!("prepare rejected: {error}"))?;
    if idle.is_some() {
        return Err(String::from("idle scene must prepare nothing"));
    }
    if scene.stats().sampled() != 0 {
        return Err(String::from("static scene must sample nothing"));
    }
    Ok(())
}

/// A mismatched session scope rejects without consuming revision or slots.
#[test]
fn rejected_scope_does_not_consume_revision_or_queue_space() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    let refused = scene.enqueue(Delta::new(
        session("b")?,
        1,
        0,
        Action::Navigate(View::Evidence),
    ));
    if refused != Err(Error::WrongScope) {
        return Err(String::from("cross-session delta must be WrongScope"));
    }
    if scene
        .next_base_revision()
        .map_err(|error| format!("{error}"))?
        != 0
    {
        return Err(String::from("rejection must not consume revision"));
    }
    if scene.stats().accepted() != 0 {
        return Err(String::from("rejection must not consume slots"));
    }
    Ok(())
}

/// Stale generations, duplicate revisions, and out-of-order deltas are stale.
#[test]
fn stale_generation_duplicate_and_out_of_order_commands_are_rejected() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    for (generation, base_revision) in [(2, 0), (1, 1)] {
        let refused = scene.enqueue(Delta::new(
            session("a")?,
            generation,
            base_revision,
            Action::Navigate(View::Proposal),
        ));
        if refused != Err(Error::Stale) {
            return Err(String::from("stale delta must be Stale"));
        }
    }
    queue(&mut scene, Action::Navigate(View::Proposal))?;
    let refused = scene.enqueue(Delta::new(
        session("a")?,
        1,
        0,
        Action::Navigate(View::Source),
    ));
    if refused != Err(Error::Stale) {
        return Err(String::from("duplicate revision must be Stale"));
    }
    Ok(())
}

/// Bounded inbox: the third enqueue fails until a drain releases slots.
#[test]
fn inbox_pressure_is_explicit_and_releases_after_drain() -> Result<(), String> {
    let limits = Limits::new(262_144, 2, 8_847_360).map_err(|error| format!("{error}"))?;
    let mut scene = canvas("a", limits)?;
    queue(&mut scene, Action::Navigate(View::Proposal))?;
    queue(&mut scene, Action::Navigate(View::Evidence))?;
    let refused = scene.enqueue(Delta::new(
        session("a")?,
        1,
        2,
        Action::Navigate(View::Source),
    ));
    if refused != Err(Error::QueueFull) {
        return Err(String::from("full inbox must be QueueFull"));
    }
    present(&mut scene, 0.0)?;
    queue(&mut scene, Action::Navigate(View::Source))?;
    Ok(())
}

/// Failed frame validation preserves queued work and revision.
#[test]
fn failed_frame_validation_preserves_queued_work() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    queue(&mut scene, Action::Navigate(View::Proposal))?;
    if scene.prepare(f64::NAN, 900, 600, false).is_ok() {
        return Err(String::from("NaN time must be rejected"));
    }
    if scene.prepare(0.0, u32::MAX, u32::MAX, false).is_ok() {
        return Err(String::from("oversize viewport must be rejected"));
    }
    if scene.revision() != 0 {
        return Err(String::from("rejection must not advance revision"));
    }
    if scene
        .next_base_revision()
        .map_err(|error| format!("{error}"))?
        != 1
    {
        return Err(String::from("queued work must keep its revision"));
    }
    if present(&mut scene, 0.0)?.target() != View::Proposal {
        return Err(String::from("queued navigation must survive"));
    }
    Ok(())
}

/// A foreign instance token cannot route input, even with identical fields.
#[test]
fn foreign_canvas_token_cannot_route_input_even_with_identical_session_values() -> Result<(), String>
{
    let mut first = canvas("a", Limits::default())?;
    let mut second = canvas("a", Limits::default())?;
    let foreign = present(&mut first, 0.0)?;
    present(&mut second, 0.0)?;
    if second.input(foreign.token(), Action::Navigate(View::Evidence)) != Err(Error::Stale) {
        return Err(String::from("foreign token must be Stale"));
    }
    Ok(())
}

/// A prepared-but-unpresented token cannot authorize input.
#[test]
fn unpresented_frame_cannot_authorize_input() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    let frame = scene
        .prepare(0.0, 900, 600, false)
        .map_err(|error| format!("{error}"))?
        .ok_or_else(|| String::from("expected a frame"))?;
    if scene.input(frame.token(), Action::Navigate(View::Evidence)) != Err(Error::Stale) {
        return Err(String::from("unpresented token must be Stale"));
    }
    scene
        .acknowledge_present(frame.token())
        .map_err(|error| format!("{error}"))?;
    scene
        .input(frame.token(), Action::Navigate(View::Evidence))
        .map_err(|error| format!("presented token must route: {error}"))?;
    Ok(())
}

/// A superseded prepared token cannot be acknowledged after a newer frame.
#[test]
fn superseded_presentation_acknowledgement_is_rejected() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    let old = scene
        .prepare(0.0, 900, 600, false)
        .map_err(|error| format!("{error}"))?
        .ok_or_else(|| String::from("expected a frame"))?;
    let new = scene
        .prepare(0.0, 900, 600, true)
        .map_err(|error| format!("{error}"))?
        .ok_or_else(|| String::from("expected a frame"))?;
    if scene.acknowledge_present(old.token()) != Err(Error::Stale) {
        return Err(String::from("superseded token must be Stale"));
    }
    scene
        .acknowledge_present(new.token())
        .map_err(|error| format!("{error}"))?;
    if scene.acknowledge_present(new.token()) != Err(Error::Stale) {
        return Err(String::from("double acknowledge must be Stale"));
    }
    Ok(())
}

/// Retargeting keeps the artifact pointer and continuous spring kinematics.
#[test]
fn retarget_does_not_replace_artifact_or_reset_motion() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    let first = present(&mut scene, 0.0)?;
    queue(&mut scene, Action::Navigate(View::Evidence))?;
    let moving = present(&mut scene, 0.04)?;
    queue(&mut scene, Action::Navigate(View::Source))?;
    let reversed = present(&mut scene, 0.0)?;
    assert_bits(moving.position(), reversed.position(), "retarget position")?;
    assert_bits(moving.velocity(), reversed.velocity(), "retarget velocity")?;
    if !std::sync::Arc::ptr_eq(first.artifact(), reversed.artifact()) {
        return Err(String::from("retarget must keep the artifact pointer"));
    }
    Ok(())
}

/// Reduced motion snaps to target with zero sampling.
#[test]
fn reduced_motion_preserves_navigation_and_stops_sampling() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    queue(&mut scene, Action::ReducedMotion(true))?;
    queue(&mut scene, Action::Navigate(View::Evidence))?;
    let frame = present(&mut scene, 0.0)?;
    assert_bits(frame.position(), 2.0, "reduced position")?;
    assert_bits(frame.velocity(), 0.0, "reduced velocity")?;
    if frame.animating() || scene.needs_frame() {
        return Err(String::from("reduced motion must settle"));
    }
    if scene.stats().sampled() != 0 {
        return Err(String::from("reduced motion must sample nothing"));
    }
    Ok(())
}

/// Revocation discards pending updates and invalidates tokens.
#[test]
fn revoke_discards_pending_updates_and_invalidates_tokens() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    let frame = present(&mut scene, 0.0)?;
    queue(&mut scene, Action::Navigate(View::Evidence))?;
    scene.revoke();
    if scene.input(frame.token(), Action::Navigate(View::Proposal)) != Err(Error::Revoked) {
        return Err(String::from("input after revoke must be Revoked"));
    }
    if !matches!(scene.prepare(0.0, 900, 600, true), Err(Error::Revoked)) {
        return Err(String::from("prepare after revoke must be Revoked"));
    }
    if scene.needs_frame() {
        return Err(String::from("revoked scene must need no frames"));
    }
    Ok(())
}

/// Missing lenses report absence, never fabricated supply.
#[test]
fn missing_evidence_is_not_reported_as_supplied_or_verified() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    let frame = present(&mut scene, 0.0)?;
    if frame.artifact().supplied(View::Proposal) || frame.artifact().supplied(View::Evidence) {
        return Err(String::from("missing lenses must not be supplied"));
    }
    if !frame.artifact().text(View::Evidence).contains("No tests") {
        return Err(String::from("missing evidence must carry absence text"));
    }
    Ok(())
}

/// Payload limits count UTF-8 bytes, not characters, and reject nulls.
#[test]
fn unicode_payload_limits_are_bytes_not_character_counts() -> Result<(), String> {
    let limits = Limits::new(180, 32, 8_847_360).map_err(|error| format!("{error}"))?;
    if Artifact::new(WORK_OBJECT, 1, &"é".repeat(181), None, None, limits).is_ok() {
        return Err(String::from("181 two-byte chars must exceed 180 bytes"));
    }
    Artifact::new(WORK_OBJECT, 1, "বাংলা", None, None, Limits::default())
        .map_err(|error| format!("multilingual text must fit: {error}"))?;
    if Artifact::new(WORK_OBJECT, 1, "bad\0text", None, None, Limits::default()).is_ok() {
        return Err(String::from("embedded null must be rejected"));
    }
    Ok(())
}

/// Hit testing follows the presented viewport and rejects non-finite input.
#[test]
fn hit_testing_uses_the_presented_viewport_and_rejects_nonfinite_coordinates() -> Result<(), String>
{
    let mut scene = canvas("a", Limits::default())?;
    let frame = present(&mut scene, 0.0)?;
    if frame.hit_test(850.0, 580.0) != Some(View::Evidence) {
        return Err(String::from("bottom-right must hit evidence"));
    }
    if frame.hit_test(f64::NAN, 580.0).is_some() {
        return Err(String::from("NaN must miss"));
    }
    if frame.hit_test(20.0, 20.0).is_some() {
        return Err(String::from("top area must miss"));
    }
    scene.invalidate_surface();
    if scene.input(frame.token(), Action::Navigate(View::Evidence)) != Err(Error::Stale) {
        return Err(String::from("invalidated token must be Stale"));
    }
    Ok(())
}

/// Scroll clamps in both directions, including extreme deltas.
#[test]
fn scroll_is_bounded_in_both_directions() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    queue(&mut scene, Action::scroll(View::Source, i32::MAX))?;
    if present(&mut scene, 0.0)?.scroll()[0] != 2 {
        return Err(String::from("max scroll must clamp to the last line"));
    }
    queue(&mut scene, Action::scroll(View::Source, i32::MIN))?;
    if present(&mut scene, 0.0)?.scroll()[0] != 0 {
        return Err(String::from("min scroll must clamp to zero"));
    }
    Ok(())
}

/// Queued relative steps accumulate in order across wrap boundaries.
#[test]
fn queued_relative_navigation_is_not_collapsed_against_stale_target() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    queue(&mut scene, Action::Step(1))?;
    queue(&mut scene, Action::Step(1))?;
    if present(&mut scene, 0.0)?.target() != View::Evidence {
        return Err(String::from("two steps must reach evidence"));
    }
    Ok(())
}

/// Sequential motion toggles process in exact queue order.
#[test]
fn queued_motion_toggles_are_ordered() -> Result<(), String> {
    let mut scene = canvas("a", Limits::default())?;
    queue(&mut scene, Action::ToggleReducedMotion)?;
    queue(&mut scene, Action::ToggleReducedMotion)?;
    if present(&mut scene, 0.0)?.reduced_motion() {
        return Err(String::from("double toggle must restore motion"));
    }
    Ok(())
}

/// An artifact from another work object is out of scope.
#[test]
fn session_cannot_present_another_work_objects_artifact() -> Result<(), String> {
    let limits = Limits::default();
    let artifact = digest_artifact("99999999-9999-9999-9999-999999999999", "private", limits)?;
    if !matches!(
        Canvas::new(session("a")?, 1, artifact, limits),
        Err(Error::WrongScope)
    ) {
        return Err(String::from("foreign artifact must be WrongScope"));
    }
    Ok(())
}

/// Identical object ids across tenants refuse cross-routing.
#[test]
fn two_tenants_same_object_digest_cannot_cross_route_deltas_or_tokens() -> Result<(), String> {
    let limits = Limits::default();
    let mut scene_a = tenant_scene(
        session_in("tenant-alpha", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa1")?,
        digest_artifact(WORK_OBJECT, "data", limits)?,
    )?;
    let mut scene_b = tenant_scene(
        session_in("tenant-beta", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa2")?,
        digest_artifact(WORK_OBJECT, "data", limits)?,
    )?;

    let frame_a = present(&mut scene_a, 0.0)?;
    let frame_b = present(&mut scene_b, 0.0)?;

    let delta_b = Delta::new(
        scene_b.session().clone(),
        scene_b.generation(),
        0,
        Action::Navigate(View::Proposal),
    );
    if scene_a.enqueue(delta_b) != Err(Error::WrongScope) {
        return Err(String::from("cross-tenant delta must be WrongScope"));
    }
    if scene_a.input(frame_b.token(), Action::Navigate(View::Proposal)) != Err(Error::Stale) {
        return Err(String::from("cross-tenant token must be Stale"));
    }
    if scene_b.input(frame_a.token(), Action::Navigate(View::Proposal)) != Err(Error::Stale) {
        return Err(String::from("reverse cross-tenant token must be Stale"));
    }
    Ok(())
}

/// Two instances for one session keep distinct token authority.
#[test]
fn two_presentations_same_object_and_session_maintain_distinct_token_authority()
-> Result<(), String> {
    let limits = Limits::default();
    let make = |tenant: &str| -> Result<Canvas, String> {
        let artifact = Artifact::new(WORK_OBJECT, 1, "src", None, None, limits)
            .map_err(|error| format!("{error}"))?;
        Canvas::new(session(tenant)?, 1, artifact, limits).map_err(|error| format!("{error}"))
    };
    let mut first = make("tenant-isolated")?;
    let mut second = make("tenant-isolated")?;
    let frame_first = present(&mut first, 0.0)?;
    let frame_second = present(&mut second, 0.0)?;
    if first.input(frame_second.token(), Action::Navigate(View::Proposal)) != Err(Error::Stale) {
        return Err(String::from("peer token must be Stale"));
    }
    if second.input(frame_first.token(), Action::Navigate(View::Proposal)) != Err(Error::Stale) {
        return Err(String::from("reverse peer token must be Stale"));
    }
    Ok(())
}
