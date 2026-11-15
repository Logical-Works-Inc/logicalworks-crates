//! Shared kit for the motion sims: small builders and reference samplers.
//!
//! The reference sampler computes a linear tween from its inputs alone, in
//! `f64`, so no motion sim takes the code's own output as its oracle. Every
//! helper here is used by at least one motion sim in the unified `sim` target.

use drishti::motion::{
    AccumulatorState, AccumulatorStore, Clock, Easing, EmptyScrubMap, Frame, MotionGraph,
    MotionGraphBuilder, NamedCurve, NodeKind, SpringConfig, TweenComponent, frame,
};
use lgwks_std::id::Uuid;

use crate::rng::SimRng;

/// Render any displayable error as the sim failure text.
pub fn describe(error: impl core::fmt::Display) -> String {
    format!("{error}")
}

/// Require a condition, naming it on failure.
pub fn ensure(valid: bool, what: &str) -> Result<(), String> {
    if valid {
        Ok(())
    } else {
        Err(String::from(what))
    }
}

/// Replace the first scalar member `key` of compact JSON text with the raw
/// JSON `value`; fails when the member is absent or runs to the end.
pub fn with_member(text: &str, key: &str, value: &str) -> Result<String, String> {
    let needle = format!("\"{key}\":");
    let start = text.find(&needle).ok_or("member missing")?;
    let from = start.saturating_add(needle.len());
    let tail = &text[from..];
    let length = tail.find([',', '}']).ok_or("member unterminated")?;
    Ok(format!("{}{value}{}", &text[..from], &tail[length..]))
}

/// Seeded node identity drawn from two stream words.
pub fn node_id(random: &mut SimRng) -> Uuid {
    let high = u128::from(random.next_u64());
    let low = u128::from(random.next_u64());
    Uuid::from(((high << 64) | low).to_le_bytes())
}

/// Spring parameters, with a rejection rendered as text.
pub fn spring_config(tension: f32, friction: f32, mass: f32) -> Result<SpringConfig, String> {
    SpringConfig::new(tension, friction, mass).map_err(describe)
}

/// Linear tween over explicit timing, with a rejection rendered as text.
pub fn linear_tween(
    from: f32,
    to: f32,
    start_ms: f32,
    duration_ms: f32,
) -> Result<TweenComponent, String> {
    TweenComponent::new(
        from,
        to,
        start_ms,
        duration_ms,
        Easing::named(NamedCurve::Linear),
    )
    .map_err(describe)
}

/// A graph holding exactly one node.
pub fn single_node(id: Uuid, kind: NodeKind) -> MotionGraph {
    MotionGraphBuilder::new()
        .add_node(id, String::from(".kit"), String::from("opacity"), kind)
        .build()
}

/// Evaluate one frame at an explicit wall time against the empty scrub map.
pub fn eval_at(graph: &MotionGraph, wall_ms: f64, store: &mut AccumulatorStore) -> Frame {
    frame(
        graph,
        &Clock::deterministic(wall_ms),
        store,
        &EmptyScrubMap::new(),
    )
}

/// The scalar a frame wrote for one node.
pub fn scalar_of(output: &Frame, id: Uuid) -> Result<f32, String> {
    output
        .bindings()
        .iter()
        .find(|write| write.node_id() == id)
        .and_then(|write| write.value().as_scalar())
        .ok_or_else(|| String::from("frame must bind the node"))
}

/// The accumulator state a store holds for one node.
pub fn state_of(store: &AccumulatorStore, id: Uuid) -> Result<AccumulatorState, String> {
    store
        .get(&id)
        .copied()
        .ok_or_else(|| String::from("store must hold the node"))
}

/// Reference linear sampler: the tween value from its inputs alone.
pub fn reference_linear(from: f32, to: f32, start_ms: f32, duration_ms: f32, wall_ms: f64) -> f64 {
    let local = ((wall_ms - f64::from(start_ms)) / f64::from(duration_ms)).clamp(0.0, 1.0);
    f64::from(from) * (1.0 - local) + f64::from(to) * local
}

/// True when `actual` lies within `tolerance` of `expected`; NaN never does.
pub fn within(actual: f32, expected: f64, tolerance: f64) -> bool {
    (f64::from(actual) - expected).abs() <= tolerance
}
