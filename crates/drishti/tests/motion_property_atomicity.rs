//! Patch atomicity: valid patches apply wholesale with a version bump,
//! and any invalid op rejects the whole patch with the graph untouched.
//!
//! Counterpart of `motion-core/tests/property_patch_atomicity.rs`
//! (kernel#850 row 15).

use drishti::motion::error::MotionErrorCode;
use drishti::motion::patch::{MotionPatch, PatchOp, PatchValue, apply_patch};
use drishti::motion::{
    AccumulatorStore, Easing, MotionGraphBuilder, NamedCurve, NodeKind, TweenComponent,
};
use lgwks_std::id::Uuid;
use proptest::prelude::*;

#[path = "support/fixtures.rs"]
mod fixtures;

/// Build a patch over one node.
fn make_patch(
    node_id: Uuid,
    ops: Vec<PatchOp>,
    preserve_velocity: bool,
) -> Result<MotionPatch, String> {
    let patch_id = Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?;
    Ok(MotionPatch::new(patch_id, node_id, ops, preserve_velocity))
}

/// Build one float op.
fn float_op(path: &str, value: f32) -> PatchOp {
    PatchOp::new(String::from(path), PatchValue::Float(value))
}

/// A valid multi-op patch applies every op and bumps the version.
#[test]
fn valid_patch_applies_every_op() -> Result<(), String> {
    let (graph, node_id) = fixtures::tween_graph(
        Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?,
        ".target",
        600.0,
    )?;
    let before = graph.version();
    let patch = make_patch(
        node_id,
        vec![float_op("duration_ms", 300.0), float_op("to", 0.5)],
        false,
    )?;
    let mut accumulators = AccumulatorStore::new();
    let updated = apply_patch(graph, &patch, &mut accumulators)
        .map_err(|error| format!("valid patch rejected: {error}"))?;
    if updated.version() <= before {
        return Err(String::from("version must increment"));
    }
    let Some(node) = updated.get_node(&node_id) else {
        return Err(String::from("node must exist after patch"));
    };
    let NodeKind::Tween(ref tween) = *node.kind() else {
        return Err(String::from("node must stay a tween"));
    };
    if (tween.duration_ms() - 300.0).abs() >= 1e-5 {
        return Err(String::from("duration_ms must apply"));
    }
    if (tween.to_value() - 0.5).abs() >= 1e-5 {
        return Err(String::from("to must apply"));
    }
    Ok(())
}

/// One bad op rejects the entire patch with the graph untouched.
#[test]
fn one_bad_op_rejects_whole_patch() -> Result<(), String> {
    let (graph, node_id) = fixtures::tween_graph(
        Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?,
        ".target",
        600.0,
    )?;
    let before = graph.version();
    let patch = make_patch(
        node_id,
        vec![float_op("duration_ms", 300.0), float_op("tension", 200.0)],
        false,
    )?;
    let mut accumulators = AccumulatorStore::new();
    if apply_patch(graph.clone(), &patch, &mut accumulators).is_ok() {
        return Err(String::from("patch with an invalid path must fail"));
    }
    if graph.version() != before {
        return Err(String::from("rejection must not touch the graph"));
    }
    Ok(())
}

/// Immutable fields fail with the immutable-path code.
#[test]
fn immutable_field_fails_with_immutable_code() -> Result<(), String> {
    let (graph, node_id) = fixtures::tween_graph(
        Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?,
        ".target",
        600.0,
    )?;
    let patch = make_patch(
        node_id,
        vec![PatchOp::new(String::from("kind"), PatchValue::Bool(true))],
        false,
    )?;
    let mut accumulators = AccumulatorStore::new();
    let Err(error) = apply_patch(graph, &patch, &mut accumulators) else {
        return Err(String::from("immutable patch must fail"));
    };
    if error.code() != MotionErrorCode::PatchPathImmutable {
        return Err(format!("wrong code: {}", error.code()));
    }
    Ok(())
}

/// Unknown nodes fail with the not-found code.
#[test]
fn unknown_node_fails_with_not_found() -> Result<(), String> {
    let (graph, _) = fixtures::tween_graph(
        Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?,
        ".target",
        600.0,
    )?;
    let unknown = Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?;
    let patch = make_patch(unknown, vec![float_op("duration_ms", 300.0)], false)?;
    let mut accumulators = AccumulatorStore::new();
    let Err(error) = apply_patch(graph, &patch, &mut accumulators) else {
        return Err(String::from("unknown node must fail"));
    };
    if error.code() != MotionErrorCode::NodeNotFound {
        return Err(format!("wrong code: {}", error.code()));
    }
    Ok(())
}

/// Preserve-velocity patches keep position and velocity in the store.
#[test]
fn preserve_velocity_keeps_store_state() -> Result<(), String> {
    let (graph, node_id) = fixtures::tween_graph(
        Uuid::new_v4().map_err(|error| format!("id mint failed: {error:?}"))?,
        ".target",
        600.0,
    )?;
    let mut accumulators = AccumulatorStore::new();
    let mut seeded = drishti::motion::AccumulatorState::zero(node_id);
    seeded.set_position(0.7);
    seeded.set_velocity(0.3);
    accumulators.set(node_id, seeded);
    let patch = make_patch(node_id, vec![float_op("to", 2.0)], true)?;
    apply_patch(graph, &patch, &mut accumulators)
        .map_err(|error| format!("valid patch rejected: {error}"))?;
    let Some(state) = accumulators.get(&node_id) else {
        return Err(String::from("state must persist"));
    };
    if (state.position() - 0.7).abs() >= 1e-5 {
        return Err(String::from("position must be preserved"));
    }
    if (state.velocity() - 0.3).abs() >= 1e-5 {
        return Err(String::from("velocity must be preserved"));
    }
    Ok(())
}

proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(500))]

    #[test]
    fn valid_patches_always_bump_version(
        from in -100.0_f32..100.0_f32,
        to in -100.0_f32..100.0_f32,
        new_duration in 1.0_f32..5000.0_f32,
    ) {
        let node_id = Uuid::new_v4()
            .map_err(|error| TestCaseError::fail(format!("id mint failed: {error:?}")))?;
        let tween = TweenComponent::new(from, to, 0.0, 600.0, Easing::named(NamedCurve::Linear))
            .map_err(|error| TestCaseError::fail(format!("valid tween rejected: {error}")))?;
        let graph = MotionGraphBuilder::new()
            .add_node(
                node_id,
                String::from("target_handle"),
                String::from("opacity"),
                NodeKind::Tween(tween),
            )
            .build();
        let before = graph.version();
        let patch = MotionPatch::new(
            Uuid::new_v4()
                .map_err(|error| TestCaseError::fail(format!("id mint failed: {error:?}")))?,
            node_id,
            vec![PatchOp::new(String::from("duration_ms"), PatchValue::Float(new_duration))],
            false,
        );
        let mut accumulators = AccumulatorStore::new();
        let updated = apply_patch(graph, &patch, &mut accumulators)
            .map_err(|error| TestCaseError::fail(format!("valid patch rejected: {error}")))?;
        prop_assert!(updated.version() > before);
    }
}
