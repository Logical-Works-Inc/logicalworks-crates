//! Accumulator patches are rejected, never silently absorbed.
//!
//! Counterpart of `motion-core/tests/patch_accumulator_rejected.rs`
//! (kernel#850 row 15): tension/friction/mass/preserve-velocity patches on
//! accumulator nodes return `PatchInvalid` with the graph version unchanged.

use drishti::motion::error::MotionErrorCode;
use drishti::motion::node::accumulator::AccumulatorComponent;
use drishti::motion::node::spring::SpringConfig;
use drishti::motion::patch::{MotionPatch, PatchOp, PatchValue, apply_patch};
use drishti::motion::{AccumulatorStore, MotionGraph, MotionGraphBuilder, NodeKind};
use lgwks_std::id::Uuid;

/// Build a one-spring accumulator graph with its node id.
fn make_spring_accumulator_graph() -> Result<(MotionGraph, Uuid), String> {
    let node_id = Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?;
    let config = SpringConfig::new(180.0, 20.0, 1.0)
        .map_err(|error| format!("valid spring rejected: {error}"))?;
    let accumulator = AccumulatorComponent::new_spring(config);
    let graph = MotionGraphBuilder::new()
        .add_node(
            node_id,
            String::from(".js-logo"),
            String::from("translateY"),
            NodeKind::Accumulator(accumulator),
        )
        .build();
    Ok((graph, node_id))
}

/// Patch one accumulator field and assert `PatchInvalid` with the version
/// untouched.
fn assert_patch_invalid(path: &str) -> Result<(), String> {
    let (graph, node_id) = make_spring_accumulator_graph()?;
    let before = graph.version();
    let patch = MotionPatch::new(
        Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?,
        node_id,
        vec![PatchOp::new(String::from(path), PatchValue::Float(1.0))],
        false,
    );
    let mut accumulators = AccumulatorStore::new();
    let error = apply_patch(graph.clone(), &patch, &mut accumulators)
        .err()
        .ok_or_else(|| format!("patching {path} must fail"))?;
    (error.code() == MotionErrorCode::PatchInvalid)
        .then_some(())
        .ok_or_else(|| format!("wrong code for {path}: {}", error.code()))?;
    (graph.version() == before)
        .then_some(())
        .ok_or_else(|| format!("version must not change for {path}"))
}

/// Tension patches fail invalid with the version untouched.
#[test]
fn tension_patch_fails_invalid() -> Result<(), String> {
    assert_patch_invalid("tension")
}

/// Every valid accumulator path still fails: patching is unimplemented, and
/// an unimplemented patch is an error, never a silent no-op.
#[test]
fn all_accumulator_paths_fail_invalid() -> Result<(), String> {
    assert_patch_invalid("tension")?;
    assert_patch_invalid("friction")?;
    assert_patch_invalid("mass")?;
    assert_patch_invalid("preserve_velocity")?;
    Ok(())
}
