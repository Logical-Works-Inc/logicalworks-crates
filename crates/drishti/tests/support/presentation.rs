//! Shared presentation scaffolding for the canvas contract suites.
//!
//! One home for the scene helpers both contract halves drive, so the halves
//! cannot drift from each other. Suites include this file with `#[path]`; it
//! is never a test target itself. Session identities come from the shared
//! fixtures module.

use drishti::canvas_scene::{Action, Artifact, Canvas, Delta, Frame, Limits};

/// Canvas fixture with a three-line source artifact.
pub(crate) fn canvas(tenant: &str, limits: Limits) -> Result<Canvas, String> {
    let artifact = Artifact::new(
        crate::fixtures::WORK_OBJECT,
        1,
        "first\nsecond\nthird",
        None,
        None,
        limits,
    )
    .map_err(|error| format!("fixture artifact rejected: {error}"))?;
    Canvas::new(crate::fixtures::session(tenant)?, 1, artifact, limits)
        .map_err(|error| format!("fixture canvas rejected: {error}"))
}

/// Prepare and acknowledge one frame.
pub(crate) fn present(scene: &mut Canvas, dt: f64) -> Result<Frame, String> {
    let frame = scene
        .prepare(dt, 900, 600, false)
        .map_err(|error| format!("prepare rejected: {error}"))?
        .ok_or_else(|| String::from("expected a frame"))?;
    scene
        .acknowledge_present(frame.token())
        .map_err(|error| format!("acknowledge rejected: {error}"))?;
    Ok(frame)
}

/// Enqueue one action at the current base revision.
pub(crate) fn queue(scene: &mut Canvas, action: Action) -> Result<(), String> {
    let base = scene
        .next_base_revision()
        .map_err(|error| format!("base revision rejected: {error}"))?;
    scene
        .enqueue(Delta::new(
            scene.session().clone(),
            scene.generation(),
            base,
            action,
        ))
        .map_err(|error| format!("enqueue rejected: {error}"))
}

/// Fail unless two floats are bit-identical, so `-0.0` and `0.0` differ and a
/// NaN equals itself.
pub(crate) fn assert_bits(actual: f64, expected: f64, what: &str) -> Result<(), String> {
    crate::fixtures::ensure(actual.to_bits() == expected.to_bits(), || {
        format!("{what}: got {actual}, want {expected}")
    })
}
