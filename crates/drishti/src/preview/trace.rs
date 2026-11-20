//! Headless reducer traversal, explicitly distinct from native evidence.
//!
//! Absorbed from `primitives/canvas-native/crates/preview/src/trace.rs`.
//! Drives the scripted journey (initial frame, toward-evidence, interrupt,
//! settle), asserts the motion settles and the scene goes idle, proves
//! revocation refuses, and emits one JSON object per line plus a terminal
//! summary.

use std::io::Write;

use lgwks_std::json::json;

use super::Failure;
use crate::canvas_scene::{Action, Canvas, Error, Frame, View};

/// Write one frame record and acknowledge it as presented to the headless
/// sink. Acknowledgement here is never an OS presentation claim.
///
/// # Errors
///
/// Returns acknowledgement or sink failures.
fn write_frame(
    canvas: &mut Canvas,
    frame: &Frame,
    label: &str,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    canvas.acknowledge_present(frame.token())?;
    let record = json!({
        "kind": "frame",
        "mode": "headless",
        "step": label,
        "object_key": frame.artifact().key(),
        "content_revision": frame.artifact().revision(),
        "scene_revision": frame.scene_revision(),
        "position": frame.position(),
        "velocity": frame.velocity(),
        "target": frame.target().index(),
        "reduced": frame.reduced_motion(),
        "native_presented": false,
        "source_bytes": frame.artifact().text(View::Source).len(),
        "proposal_supplied": frame.artifact().supplied(View::Proposal),
        "evidence_supplied": frame.artifact().supplied(View::Evidence),
    });
    lgwks_std::json::to_writer(&mut *out, &record)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// Navigate to a target and sample `count` frames under a label.
///
/// # Errors
///
/// Returns input, preparation, or sink failures.
fn sample_navigation(
    canvas: &mut Canvas,
    target: View,
    count: usize,
    label: &str,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let token = canvas.presented_token().ok_or("missing presented frame")?;
    canvas.input(token, Action::Navigate(target))?;
    for _ in 0..count {
        if let Some(frame) = canvas.prepare(1.0 / 60.0, 960, 640, false)? {
            write_frame(canvas, &frame, label, out)?;
        }
    }
    Ok(())
}

/// Run the full scripted traversal, writing JSONL records to `out`.
///
/// # Errors
///
/// Returns traversal, idle, or sink failures. The summary record is always
/// the last line on success.
///
/// # Examples
///
/// ```
/// use drishti::canvas_scene::{Artifact, Canvas, Limits};
/// use drishti::cli::drive::{WORK_OBJECT, session};
/// use drishti::preview::run_traversal;
/// use lgwks_std::json::{Value, from_str};
///
/// let limits = Limits::default();
/// let artifact = Artifact::new(WORK_OBJECT, 1, "fn main() {}", None, None, limits)
///     .map_err(|error| error.to_string())?;
/// let bound = session(&mut String::new(), "local-preview").map_err(|error| error.to_string())?;
/// let canvas = Canvas::new(bound, 1, artifact, limits).map_err(|error| error.to_string())?;
///
/// let mut written = Vec::new();
/// run_traversal(canvas, &mut written).map_err(|error| error.to_string())?;
/// let text = String::from_utf8(written).map_err(|error| error.to_string())?;
/// let last = text.lines().last().ok_or("no records written")?;
/// let summary: Value = from_str(last).map_err(|error| error.to_string())?;
/// assert_eq!(summary.get("kind").and_then(Value::as_str), Some("summary"));
/// assert_eq!(summary.get("native_presented").and_then(Value::as_bool), Some(false));
/// assert_eq!(summary.get("revocation_refused").and_then(Value::as_bool), Some(true));
/// # Ok::<(), String>(())
/// ```
pub fn run_traversal(mut canvas: Canvas, mut out: impl Write) -> Result<(), Failure> {
    let first = canvas
        .prepare(0.0, 960, 640, false)?
        .ok_or("initial frame missing")?;
    write_frame(&mut canvas, &first, "initial", &mut out)?;
    for (target, count, label) in [
        (View::Evidence, 5, "toward-evidence"),
        (View::Source, 5, "interrupt"),
        (View::Proposal, 240, "settle"),
    ] {
        sample_navigation(&mut canvas, target, count, label, &mut out)?;
    }
    (!canvas.needs_frame())
        .then_some(())
        .ok_or("motion did not settle within the scripted sample budget")?;
    let idle = canvas.prepare(1.0 / 60.0, 960, 640, false)?.is_none();
    canvas.revoke();
    let refused = matches!(canvas.prepare(0.0, 960, 640, false), Err(Error::Revoked));
    let summary = json!({
        "kind": "summary",
        "mode": "headless",
        "idle_no_frame": idle,
        "revocation_refused": refused,
        "native_presented": false,
        "prepared": canvas.stats().prepared(),
        "acknowledged": canvas.stats().presented(),
    });
    lgwks_std::json::to_writer(&mut out, &summary)?;
    out.write_all(b"\n")?;
    Ok(())
}
