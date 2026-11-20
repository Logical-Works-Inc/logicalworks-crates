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
pub(crate) fn describe(error: impl core::fmt::Display) -> String {
    format!("{error}")
}

/// Require a condition, naming it on failure.
pub(crate) fn ensure(valid: bool, what: &str) -> Result<(), String> {
    if valid {
        Ok(())
    } else {
        Err(String::from(what))
    }
}

/// The element of `items` at `index`; a short slice is a sim failure.
pub(crate) fn at<T>(items: &[T], index: usize) -> Result<&T, String> {
    items
        .get(index)
        .ok_or_else(|| format!("no element {index} in {} items", items.len()))
}

/// The element of `items` at `index`, mutably; a short slice is a sim failure.
pub(crate) fn at_mut<T>(items: &mut [T], index: usize) -> Result<&mut T, String> {
    let held = items.len();
    items
        .get_mut(index)
        .ok_or_else(|| format!("no element {index} in {held} items"))
}

/// A seeded element of `items`; an empty slice is a sim failure.
pub(crate) fn pick<T: Copy>(random: &mut SimRng, items: &[T]) -> Result<T, String> {
    items
        .get(random.below_len(items.len()))
        .copied()
        .ok_or_else(|| String::from("nothing to pick from"))
}

/// Replace the first scalar member `key` of compact JSON text with the raw
/// JSON `value`; fails when the member is absent or runs to the end.
pub(crate) fn with_member(text: &str, key: &str, value: &str) -> Result<String, String> {
    let needle = format!("\"{key}\":");
    let start = text.find(&needle).ok_or("member missing")?;
    let from = start.saturating_add(needle.len());
    let (head, tail) = text
        .split_at_checked(from)
        .ok_or("member offset is not a character boundary")?;
    let length = tail.find([',', '}']).ok_or("member unterminated")?;
    let (_, rest) = tail
        .split_at_checked(length)
        .ok_or("member end is not a character boundary")?;
    Ok(format!("{head}{value}{rest}"))
}

/// Seeded node identity drawn from two stream words.
pub(crate) fn node_id(random: &mut SimRng) -> Uuid {
    let high = u128::from(random.next_u64());
    let low = u128::from(random.next_u64());
    Uuid::from(((high << 64) | low).to_le_bytes())
}

/// Spring parameters, with a rejection rendered as text.
pub(crate) fn spring_config(
    tension: f32,
    friction: f32,
    mass: f32,
) -> Result<SpringConfig, String> {
    SpringConfig::new(tension, friction, mass).map_err(describe)
}

/// Linear tween over explicit timing, with a rejection rendered as text.
pub(crate) fn linear_tween(
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
pub(crate) fn single_node(id: Uuid, kind: NodeKind) -> MotionGraph {
    MotionGraphBuilder::new()
        .add_node(id, String::from(".kit"), String::from("opacity"), kind)
        .build()
}

/// Evaluate one frame at an explicit wall time against the empty scrub map.
pub(crate) fn eval_at(graph: &MotionGraph, wall_ms: f64, store: &mut AccumulatorStore) -> Frame {
    frame(
        graph,
        &Clock::deterministic(wall_ms),
        store,
        &EmptyScrubMap::new(),
    )
}

/// The scalar a frame wrote for one node.
pub(crate) fn scalar_of(output: &Frame, id: Uuid) -> Result<f32, String> {
    output
        .bindings()
        .iter()
        .find(|write| write.node_id() == id)
        .and_then(|write| write.value().as_scalar())
        .ok_or_else(|| String::from("frame must bind the node"))
}

/// The accumulator state a store holds for one node.
pub(crate) fn state_of(store: &AccumulatorStore, id: Uuid) -> Result<AccumulatorState, String> {
    store
        .get(&id)
        .copied()
        .ok_or_else(|| String::from("store must hold the node"))
}

/// Reference linear sampler: the tween value from its inputs alone.
pub(crate) fn reference_linear(
    from: f32,
    to: f32,
    start_ms: f32,
    duration_ms: f32,
    wall_ms: f64,
) -> f64 {
    let local = ((wall_ms - f64::from(start_ms)) / f64::from(duration_ms)).clamp(0.0, 1.0);
    f64::from(from) * (1.0 - local) + f64::from(to) * local
}

/// True when `actual` lies within `tolerance` of `expected`; NaN never does.
pub(crate) fn within(actual: f32, expected: f64, tolerance: f64) -> bool {
    (f64::from(actual) - expected).abs() <= tolerance
}
