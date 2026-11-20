//! Trace checker: Rust rewrite of `tools/check_trace.py`.
//!
//! The kernel checked headless JSONL traces with Python; Python is banned in
//! product repositories, so the checker lives here. It validates actual
//! preview output against the independently stated analytic trajectory for
//! the critically-damped spring (omega = 6 / 0.45, 60 Hz steps, settle snap
//! at |d| < 1e-5 and |v| < 1e-4). This is headless reducer evidence only: not
//! native rendering, user testing, authorization, or memory evidence.

use std::collections::BTreeMap;

use lgwks_deps::libm;
use lgwks_macros::Error;
use lgwks_std::json::Value;

/// Analytic spring constant: 6 / 0.45, matching the preview's
/// `CriticalSpring::new(0.0, 0.45)`.
const OMEGA: f64 = 6.0 / 0.45;

/// Scripted frame step.
const STEP_SECONDS: f64 = 1.0 / 60.0;

/// Position settle threshold.
const SETTLE_POSITION: f64 = 1e-5;

/// Velocity settle threshold.
const SETTLE_VELOCITY: f64 = 1e-4;

/// Comparison tolerance, relative and absolute.
const CLOSE_TOLERANCE: f64 = 1e-9;

/// Every way a trace can fail the check. Each variant names the first
/// invariant the trace broke, so a rejection tells the operator where to look.
///
/// # Examples
///
/// ```
/// use drishti::preview::{TraceCheckError, check_records};
///
/// assert_eq!(check_records(&[], 0, false), Err(TraceCheckError::MissingSummary));
/// assert_eq!(
///     TraceCheckError::MissingSummary.to_string(),
///     "missing terminal summary"
/// );
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TraceCheckError {
    /// Last record is not a summary.
    #[error("missing terminal summary")]
    MissingSummary,
    /// Fewer than four frames, or a non-frame record among them.
    #[error("missing frames or unexpected record")]
    MissingFrames,
    /// Unknown script phase.
    #[error("unknown script phase: {0}")]
    UnknownStep(String),
    /// The `reduced` flag does not match the requested mode.
    #[error("requested motion mode was not honored")]
    ModeMismatch,
    /// Signal identity, revision, or target drifted.
    #[error("identity, revision or target drift")]
    IdentityDrift,
    /// A false native-presentation claim or wrong input byte count.
    #[error("false native claim or incorrect input byte count")]
    NativeOrBytes,
    /// Position or velocity differs from the analytic oracle.
    #[error("trajectory differs from analytic oracle at {step} frame {index}")]
    Trajectory {
        /// Script phase of the offending frame.
        step: String,
        /// Frame index within the trace.
        index: usize,
    },
    /// Idle/revocation boundary failed.
    #[error("idle/revocation boundary failed")]
    IdleBoundary,
    /// Prepared/acknowledged accounting drifted from the frame count.
    #[error("frame accounting drift")]
    AccountingDrift,
    /// A frame-count conversion overflowed its target.
    #[error("frame count conversion overflow")]
    FrameCountConversion(#[from] core::num::TryFromIntError),
    /// Phases incomplete or the final frame is not settled at 1.0.
    #[error("incomplete journey")]
    IncompleteJourney,
    /// Script phase order changed.
    #[error("script phase order changed")]
    PhaseOrder,
    /// A script phase was truncated or repeated.
    #[error("script phase count changed")]
    PhaseCount,
}

/// Extract a string field or fail the trace.
fn field_str<'a>(record: &'a Value, name: &str) -> Result<&'a str, TraceCheckError> {
    record
        .get(name)
        .and_then(Value::as_str)
        .ok_or(TraceCheckError::MissingFrames)
}

/// Extract a float field or fail the trace.
fn field_f64(record: &Value, name: &str) -> Result<f64, TraceCheckError> {
    record
        .get(name)
        .and_then(Value::as_f64)
        .ok_or(TraceCheckError::MissingFrames)
}

/// Extract an integer field or fail the trace.
fn field_u64(record: &Value, name: &str) -> Result<u64, TraceCheckError> {
    record
        .get(name)
        .and_then(Value::as_u64)
        .ok_or(TraceCheckError::MissingFrames)
}

/// Extract a bool field or fail the trace.
fn field_bool(record: &Value, name: &str) -> Result<bool, TraceCheckError> {
    record
        .get(name)
        .and_then(Value::as_bool)
        .ok_or(TraceCheckError::MissingFrames)
}

/// Fail with `error` unless `holds`; the one place a failed invariant becomes
/// a typed rejection.
fn require(holds: bool, error: TraceCheckError) -> Result<(), TraceCheckError> {
    holds.then_some(()).ok_or(error)
}

/// `math.isclose` with 1e-9 relative and absolute tolerance.
fn is_close(actual: f64, expected: f64) -> bool {
    if !actual.is_finite() || !expected.is_finite() {
        return false;
    }
    let allowed = (CLOSE_TOLERANCE * actual.abs().max(expected.abs())).max(CLOSE_TOLERANCE);
    (actual - expected).abs() <= allowed
}

/// Target view index for each script phase.
fn phase_target(step: &str) -> Result<f64, TraceCheckError> {
    match step {
        "initial" => Ok(0.0),
        "toward-evidence" => Ok(2.0),
        "interrupt" => Ok(0.0),
        "settle" => Ok(1.0),
        _ => Err(TraceCheckError::UnknownStep(String::from(step))),
    }
}

/// Check parsed JSONL records against the analytic oracle.
///
/// `records` holds one parsed JSON value per line with the summary last;
/// `source_bytes` is the byte length of the `--source` file;
/// `expected_reduced` selects the `--reduced` script shape.
///
/// Returns the number of frames checked.
///
/// # Errors
///
/// Returns the first [`TraceCheckError`] found, in record order.
///
/// # Examples
///
/// A trace the headless traversal wrote passes its own checker, and the
/// count is every record but the summary:
///
/// ```
/// use drishti::canvas_scene::{Artifact, Canvas, Limits};
/// use drishti::cli::drive::{WORK_OBJECT, session};
/// use drishti::preview::{check_records, run_traversal};
/// use lgwks_std::json::{Value, from_str};
///
/// let limits = Limits::default();
/// let source = "fn main() {}";
/// let artifact = Artifact::new(WORK_OBJECT, 1, source, None, None, limits)
///     .map_err(|error| error.to_string())?;
/// let bound = session(&mut String::new(), "local-preview").map_err(|error| error.to_string())?;
/// let canvas = Canvas::new(bound, 1, artifact, limits).map_err(|error| error.to_string())?;
///
/// let mut written = Vec::new();
/// run_traversal(canvas, &mut written).map_err(|error| error.to_string())?;
/// let text = String::from_utf8(written).map_err(|error| error.to_string())?;
/// let mut records: Vec<Value> = Vec::new();
/// for line in text.lines() {
///     records.push(from_str(line).map_err(|error| error.to_string())?);
/// }
/// let source_bytes = u64::try_from(source.len()).map_err(|error| error.to_string())?;
///
/// assert_eq!(check_records(&records, source_bytes, false), Ok(records.len() - 1));
/// // Dropping the summary makes the last record a frame, which is refused.
/// records.pop();
/// assert!(check_records(&records, source_bytes, false).is_err());
/// # Ok::<(), String>(())
/// ```
pub fn check_records(
    records: &[Value],
    source_bytes: u64,
    expected_reduced: bool,
) -> Result<usize, TraceCheckError> {
    let (summary, frames) = records
        .split_last()
        .ok_or(TraceCheckError::MissingSummary)?;
    require(
        summary.get("kind").and_then(Value::as_str) == Some("summary"),
        TraceCheckError::MissingSummary,
    )?;
    require(
        frames.len() >= 4
            && frames
                .iter()
                .all(|frame| frame.get("kind").and_then(Value::as_str) == Some("frame")),
        TraceCheckError::MissingFrames,
    )?;

    let first = frames.first().ok_or(TraceCheckError::MissingFrames)?;
    let key = String::from(field_str(first, "object_key")?);
    let mut position = 0.0_f64;
    let mut velocity = 0.0_f64;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut order: Vec<&str> = Vec::new();

    for (index, frame) in frames.iter().enumerate() {
        let step = field_str(frame, "step")?;
        counts
            .entry(step)
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);
        if order.last() != Some(&step) {
            order.push(step);
        }
        require(
            field_bool(frame, "reduced")? == expected_reduced,
            TraceCheckError::ModeMismatch,
        )?;
        let target = phase_target(step)?;
        require(
            keeps_identity(frame, target, &key)?,
            TraceCheckError::IdentityDrift,
        )?;
        require(
            !field_bool(frame, "native_presented")?
                && field_u64(frame, "source_bytes")? == source_bytes,
            TraceCheckError::NativeOrBytes,
        )?;
        if field_bool(frame, "reduced")? {
            position = target;
            velocity = 0.0;
        } else if step != "initial" {
            let displacement = position - target;
            let coefficient = velocity + OMEGA * displacement;
            let decay = libm::exp(-OMEGA * STEP_SECONDS);
            position = target + (displacement + coefficient * STEP_SECONDS) * decay;
            velocity = (velocity - OMEGA * coefficient * STEP_SECONDS) * decay;
            if (position - target).abs() < SETTLE_POSITION && velocity.abs() < SETTLE_VELOCITY {
                position = target;
                velocity = 0.0;
            }
        }
        (is_close(field_f64(frame, "position")?, position)
            && is_close(field_f64(frame, "velocity")?, velocity))
        .then_some(())
        .ok_or_else(|| TraceCheckError::Trajectory {
            step: String::from(step),
            index,
        })?;
    }

    require(summary_is_idle(summary)?, TraceCheckError::IdleBoundary)?;
    // `u64` holds every frame count: no trace holds 2^64 frames, and the
    // mistyped length refuses as accounting drift rather than comparing
    // against a sentinel.
    let frame_count = u64::try_from(frames.len())?;
    require(
        field_u64(summary, "prepared")? == frame_count
            && field_u64(summary, "acknowledged")? == frame_count,
        TraceCheckError::AccountingDrift,
    )?;
    // First-seen phase order, not the `counts` map order: `BTreeMap`
    // iterates alphabetically, which never matches script order.
    let phases: Vec<&str> = order.clone();
    require(
        phases == ["initial", "toward-evidence", "interrupt", "settle"],
        TraceCheckError::IncompleteJourney,
    )?;
    let last = frames.last().ok_or(TraceCheckError::IncompleteJourney)?;
    require(
        field_f64(last, "position")?.to_bits() == 1.0_f64.to_bits(),
        TraceCheckError::IncompleteJourney,
    )?;
    let expected_count = if expected_reduced { 1 } else { 5 };
    require(
        order == ["initial", "toward-evidence", "interrupt", "settle"]
            && counts.get("initial") == Some(&1),
        TraceCheckError::PhaseOrder,
    )?;
    require(
        counts.get("toward-evidence") == Some(&expected_count)
            && counts.get("interrupt") == Some(&expected_count),
        TraceCheckError::PhaseCount,
    )?;
    Ok(frames.len())
}

/// Whether a frame still names the traced object at the phase's target and the
/// first content revision. Each field is read only when the one before held.
fn keeps_identity(frame: &Value, target: f64, key: &str) -> Result<bool, TraceCheckError> {
    Ok(field_f64(frame, "target")?.to_bits() == target.to_bits()
        && field_str(frame, "object_key")? == key
        && field_u64(frame, "content_revision")? == 1)
}

/// Whether the summary reports an idle, revoked, native-free run.
fn summary_is_idle(summary: &Value) -> Result<bool, TraceCheckError> {
    Ok(field_bool(summary, "idle_no_frame")?
        && field_bool(summary, "revocation_refused")?
        && !field_bool(summary, "native_presented")?)
}
