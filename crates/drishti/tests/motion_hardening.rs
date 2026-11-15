//! Hardening: clock clamping teleports instead of exploding, and liveness
//! leases reap zombie nodes.
//!
//! Counterpart of `motion-core/tests/hardening_verification.rs` (kernel#850
//! row 15).

use drishti::motion::{
    AccumulatorComponent, AccumulatorStore, Clock, EmptyScrubMap, MotionGraphBuilder, NodeKind,
    SpringConfig, frame,
};
use lgwks_std::id::Uuid;

/// Build a one-spring graph and return it with the node id.
fn spring_graph(node_id: Uuid, handle: &str) -> Result<drishti::motion::MotionGraph, String> {
    let config = SpringConfig::new(170.0, 26.0, 1.0)
        .map_err(|error| format!("valid spring rejected: {error}"))?;
    let accumulator = AccumulatorComponent::new_spring(config);
    Ok(MotionGraphBuilder::new()
        .add_node(
            node_id,
            String::from(handle),
            String::from("opacity"),
            NodeKind::Accumulator(accumulator),
        )
        .build())
}

/// A five-minute background jump teleports to target instead of exploding.
#[test]
fn clock_jump_teleports_instead_of_exploding() -> Result<(), String> {
    let mut accumulators = AccumulatorStore::new();
    let node_id = Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?;
    let graph = spring_graph(node_id, "target")?;
    let scrubs = EmptyScrubMap::new();
    let first_frame = frame(
        &graph,
        &Clock::deterministic(0.0),
        &mut accumulators,
        &scrubs,
    );
    let jumped_frame = frame(
        &graph,
        &Clock::deterministic(300_000.0),
        &mut accumulators,
        &scrubs,
    );
    if first_frame.bindings().len() != jumped_frame.bindings().len() {
        return Err(String::from("jump must keep binding count"));
    }
    let Some(state) = accumulators.get(&node_id) else {
        return Err(String::from("spring state must exist after frames"));
    };
    if state.position().to_bits() != 1.0_f32.to_bits()
        || state.velocity().to_bits() != 0.0_f32.to_bits()
    {
        return Err(format!(
            "jump must teleport to (1, 0): got ({}, {})",
            state.position(),
            state.velocity()
        ));
    }
    if !state.position().is_finite() {
        return Err(String::from("teleported position must be finite"));
    }
    Ok(())
}

/// Generations reap nodes untouched since the previous rotation.
#[test]
fn liveness_leases_reap_zombies() -> Result<(), String> {
    let mut accumulators = AccumulatorStore::new();
    let first_id = Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?;
    let second_id = Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?;
    let graph_first = spring_graph(first_id, "t1")?;
    let graph_second = spring_graph(second_id, "t2")?;
    let scrubs = EmptyScrubMap::new();
    let clock = Clock::deterministic(0.0);

    let first_frame = frame(&graph_first, &clock, &mut accumulators, &scrubs);
    if first_frame.bindings().len() != 1 {
        return Err(String::from("first graph must bind once"));
    }
    if accumulators.get(&first_id).is_none() {
        return Err(String::from("first node must be leased"));
    }
    accumulators.rotate_and_reap();

    let second_frame = frame(&graph_second, &clock, &mut accumulators, &scrubs);
    if second_frame.bindings().len() != 1 {
        return Err(String::from("second graph must bind once"));
    }
    if accumulators.get(&first_id).is_none() {
        return Err(String::from("first node survives one rotation"));
    }
    if accumulators.get(&second_id).is_none() {
        return Err(String::from("second node must be leased"));
    }
    accumulators.rotate_and_reap();

    if accumulators.get(&first_id).is_some() {
        return Err(String::from("first node must be reaped"));
    }
    if accumulators.get(&second_id).is_none() {
        return Err(String::from("second node must survive"));
    }
    Ok(())
}
