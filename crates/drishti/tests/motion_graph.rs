//! Graph ownership: versions bump on every mutation, removal drops nodes,
//! conflict policies stick, and the store honors leases.
//!
//! No kernel counterpart file; this covers the absorbed graph, builder,
//! store-removal, component-accessor, and clock surface that the ported
//! suites exercise only in passing.

use drishti::motion::{
    AccumulatorComponent, AccumulatorState, AccumulatorStore, Clock, ConflictPolicy, Easing,
    NamedCurve, NodeKind, SpringConfig, TweenComponent,
};
use lgwks_std::id::Uuid;

#[path = "support/fixtures.rs"]
mod fixtures;

/// Every mutation bumps the version exactly once.
#[test]
fn mutations_bump_version_once() -> Result<(), String> {
    let (graph, node_id) = fixtures::tween_graph(
        Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?,
        "handle",
        100.0,
    )?;
    if graph.version() != 1 {
        return Err(String::from("one add must leave version 1"));
    }
    let tween = TweenComponent::new(0.0, 2.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))
        .map_err(|error| format!("valid tween rejected: {error}"))?;
    let node = drishti::motion::MotionNode::new(
        node_id,
        drishti::motion::BindingTarget::new(String::from("handle"), String::from("opacity")),
        NodeKind::Tween(tween),
    );
    if node.target().handle() != "handle" || node.target().property() != "opacity" {
        return Err(String::from("binding target accessors must round-trip"));
    }
    if node.node_id() != node_id {
        return Err(String::from("node id must round-trip"));
    }
    let replaced = graph
        .replace_node(node)
        .map_err(|error| format!("replace of a held node refused: {error}"))?;
    if replaced.version() != 2 {
        return Err(String::from("replace must leave version 2"));
    }
    let removed = replaced.remove_node(&node_id);
    if removed.version() != 3 {
        return Err(String::from("remove must leave version 3"));
    }
    if removed.get_node(&node_id).is_some() {
        return Err(String::from("removed node must be gone"));
    }
    if removed.node_count() != 0 {
        return Err(String::from("graph must be empty after removal"));
    }
    Ok(())
}

/// Conflict policies attach per property and read back.
#[test]
fn conflict_policies_attach_per_property() -> Result<(), String> {
    let (graph, _) = fixtures::tween_graph(
        Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?,
        "handle",
        100.0,
    )?;
    if graph.conflict_policy("opacity").is_some() {
        return Err(String::from("no policy must be set initially"));
    }
    let graph = graph.with_conflict_policy(String::from("opacity"), ConflictPolicy::Error);
    if graph.conflict_policy("opacity") != Some(&ConflictPolicy::Error) {
        return Err(String::from("policy must read back"));
    }
    if graph.version() != 2 {
        return Err(String::from("policy set must bump the version"));
    }
    Ok(())
}

/// Component accessors and store removal behave.
#[test]
fn components_and_store_removal_behave() -> Result<(), String> {
    let config =
        SpringConfig::new(10.0, 2.0, 1.0).map_err(|error| format!("valid spring: {error}"))?;
    if config.tension().to_bits() != 10.0_f32.to_bits()
        || config.friction().to_bits() != 2.0_f32.to_bits()
        || config.mass().to_bits() != 1.0_f32.to_bits()
    {
        return Err(String::from("spring accessors must round-trip"));
    }
    let spring_component = AccumulatorComponent::new_spring(config);
    if spring_component.kind() != drishti::motion::AccumulatorKind::Spring
        || spring_component.spring_config().is_none()
        || !spring_component.preserve_velocity()
    {
        return Err(String::from("spring component accessors must agree"));
    }
    let scrubbed = AccumulatorComponent::new_scrub(String::from("pointer.x"))
        .map_err(|error| format!("valid scrub rejected: {error}"))?;
    if scrubbed.scrub_source_id() != Some("pointer.x") {
        return Err(String::from("scrub source must round-trip"));
    }
    let relaxed = scrubbed.with_preserve_velocity(true);
    if !relaxed.preserve_velocity() {
        return Err(String::from("preserve flag must override"));
    }
    let drag = AccumulatorComponent::new_drag();
    if drag.kind() != drishti::motion::AccumulatorKind::Drag {
        return Err(String::from("drag kind must round-trip"));
    }
    let node_id = Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?;
    let mut store = AccumulatorStore::new();
    store.set(node_id, AccumulatorState::zero(node_id));
    if store.get(&node_id).is_none() {
        return Err(String::from("stored state must read back"));
    }
    store.remove(&node_id);
    if store.get(&node_id).is_some() {
        return Err(String::from("removed state must be gone"));
    }
    Ok(())
}

/// The clock advances purely and reports its face value.
#[test]
fn clock_advances_purely() -> Result<(), String> {
    let clock = Clock::deterministic(100.0);
    if clock.wall_clock_ms().to_bits() != 100.0_f64.to_bits() {
        return Err(String::from("clock must report its face value"));
    }
    let later = clock.advance_ms(50.0);
    if later.wall_clock_ms().to_bits() != 150.0_f64.to_bits() {
        return Err(String::from("advance must return a later clock"));
    }
    if clock.wall_clock_ms().to_bits() != 100.0_f64.to_bits() {
        return Err(String::from("advance must not mutate"));
    }
    Ok(())
}
