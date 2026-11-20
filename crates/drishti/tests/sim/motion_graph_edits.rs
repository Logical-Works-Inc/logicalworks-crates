//! Graph-edit sims: replacing a node and patching with velocity preservation
//! are checked against what the graph and the accumulator store may hold
//! afterwards, so an edit can neither invent a node nor invent state.

use std::collections::BTreeSet;

use drishti::motion::{
    AccumulatorState, AccumulatorStore, BindingTarget, MotionError, MotionErrorCode, MotionGraph,
    MotionNode, MotionPatch, NodeKind, PatchOp, PatchValue, apply_patch,
};
use lgwks_std::id::Uuid;

use crate::motion_kit::{
    describe, ensure, eval_at, linear_tween, node_id, scalar_of, state_of, within,
};
use crate::rng::SimRng;

/// A linear tween node from zero to `to` over 100 ms.
fn tween_to(id: Uuid, to: f32) -> Result<MotionNode, String> {
    Ok(MotionNode::new(
        id,
        BindingTarget::new(String::from(".edit"), String::from("opacity")),
        NodeKind::Tween(linear_tween(0.0, to, 0.0, 100.0)?),
    ))
}

/// Replace a node through the call under test.
fn replaced(graph: MotionGraph, node: MotionNode) -> Result<MotionGraph, MotionError> {
    graph.replace_node(node)
}

/// Bug caught: replacing a node the graph does not hold inserts it, so a
/// stale or forged handle grows the graph, bumps its version and starts
/// animating a node nobody added; replacing a held node must swap exactly
/// that node and nothing else.
#[test]
fn sim_motion_graph_replace_refuses_a_missing_node() -> Result<(), String> {
    for seed in 0..60_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(4_000));
        let mut graph = MotionGraph::default();
        let mut live = BTreeSet::new();
        for _ in 0..random.below_len(8) {
            let id = node_id(&mut random);
            graph = graph.add_node(tween_to(id, 1.0)?);
            live.insert(id);
        }
        let version = graph.version();
        let count = graph.node_count();
        let missing = node_id(&mut random);
        ensure(!live.contains(&missing), "the probe handle must be fresh")?;
        match replaced(graph.clone(), tween_to(missing, 5.0)?) {
            Ok(grown) => {
                return Err(format!(
                    "seed {seed}: replacing a missing node gave a graph of {} nodes at version {}",
                    grown.node_count(),
                    grown.version()
                ));
            }
            Err(error) => ensure(
                error.code() == MotionErrorCode::NodeNotFound,
                &format!("seed {seed}: a missing node drew {error}"),
            )?,
        }
        ensure(
            graph.node_count() == count && graph.version() == version,
            "the graph that was probed must not change",
        )?;
        if let Some(held) = live.iter().next().copied() {
            let next = replaced(graph, tween_to(held, 7.0)?).map_err(describe)?;
            ensure(
                next.node_count() == count && next.version() == version.saturating_add(1),
                &format!(
                    "seed {seed}: replacing a held node changed the count or skipped a version"
                ),
            )?;
            let mut store = AccumulatorStore::new();
            let output = eval_at(&next, 100.0, &mut store);
            ensure(
                within(scalar_of(&output, held)?, 7.0, 1e-3),
                &format!("seed {seed}: the held node was not swapped for the new one"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a velocity-preserving patch on a tween creates an accumulator
/// slot for a node that has none, so the store grows with every patched
/// tween (and a later spring on that handle would start from a phantom
/// state); a bystander's state must also be left alone.
#[test]
fn sim_motion_patch_preserve_velocity_creates_no_state_for_tweens() -> Result<(), String> {
    for seed in 0..40_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(4_500));
        let mut graph = MotionGraph::default();
        let mut ids = Vec::new();
        for _ in 0..random.below_len(5).saturating_add(1) {
            let id = node_id(&mut random);
            graph = graph.add_node(tween_to(id, 1.0)?);
            ids.push(id);
        }
        let bystander = node_id(&mut random);
        let mut held = AccumulatorState::zero(bystander);
        held.set_position(0.75);
        let mut store = AccumulatorStore::new();
        store.set(bystander, held);
        for step in 0..12_u64 {
            let id = *ids
                .get(random.below_len(ids.len()))
                .ok_or("a graph must hold a tween id")?;
            let to = f32::from(random.below_u16(40));
            let patch = MotionPatch::new(
                node_id(&mut random),
                id,
                vec![PatchOp::new(String::from("to"), PatchValue::Float(to))],
                random.below(2) == 0,
            );
            graph = apply_patch(graph, &patch, &mut store).map_err(describe)?;
            for each in &ids {
                ensure(
                    store.get(each).is_none(),
                    &format!("seed {seed} step {step}: a patched tween gained accumulator state"),
                )?;
            }
        }
        ensure(
            state_of(&store, bystander)?.position().to_bits() == 0.75_f32.to_bits(),
            &format!("seed {seed}: the bystander's state changed"),
        )?;
    }
    Ok(())
}
