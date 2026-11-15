//! Frame replay determinism: identical inputs produce identical frames.
//!
//! Counterpart of `motion-core/tests/property_replay_determinism.rs`
//! (kernel#850 row 15). Bindings sort by node id before comparison because
//! graph iteration order is an implementation detail, not a contract.

use drishti::motion::{
    AccumulatorStore, Clock, Easing, MotionGraphBuilder, NamedCurve, NodeKind, TweenComponent,
    frame, scrub::EmptyScrubMap,
};
use lgwks_std::id::Uuid;
use proptest::prelude::*;

/// Small easing pool for generated tweens.
fn arb_easing() -> impl Strategy<Value = Easing> {
    prop_oneof![
        Just(Easing::named(NamedCurve::Linear)),
        Just(Easing::named(NamedCurve::CubicOut)),
        Just(Easing::named(NamedCurve::ExpoInOut)),
    ]
}

/// Valid tween parameter tuples; construction happens in the test body.
fn arb_tween_params() -> impl Strategy<Value = (f32, f32, f32, f32, Easing)> {
    (
        -100.0_f32..100.0_f32,
        -100.0_f32..100.0_f32,
        0.0_f32..1000.0_f32,
        1.0_f32..2000.0_f32,
        arb_easing(),
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn identical_inputs_produce_identical_frames(
        params in prop::collection::vec(arb_tween_params(), 1..=20),
        wall_ms in 0.0_f64..5000.0_f64,
    ) {
        let mut builder = MotionGraphBuilder::new();
        for param in &params {
            let tween = TweenComponent::new(
                param.0,
                param.1,
                param.2,
                param.3,
                param.4.clone(),
            )
            .map_err(|error| TestCaseError::fail(format!("valid tween rejected: {error}")))?;
            builder = builder.add_node(
                Uuid::new_v4()
                    .map_err(|error| TestCaseError::fail(format!("id mint failed: {error:?}")))?,
                String::from(".target"),
                String::from("opacity"),
                NodeKind::Tween(tween),
            );
        }
        let graph = builder.build();
        let clock = Clock::deterministic(wall_ms);
        let scrubs = EmptyScrubMap::new();

        let mut first_store = AccumulatorStore::new();
        let mut second_store = AccumulatorStore::new();

        let first = frame(&graph, &clock, &mut first_store, &scrubs);
        let second = frame(&graph, &clock, &mut second_store, &scrubs);

        prop_assert_eq!(first.t_wall_ms().to_bits(), second.t_wall_ms().to_bits());
        prop_assert_eq!(first.bindings().len(), second.bindings().len());

        let mut first_bindings = first.bindings().to_vec();
        let mut second_bindings = second.bindings().to_vec();
        first_bindings.sort_by_key(|write| write.node_id());
        second_bindings.sort_by_key(|write| write.node_id());

        for (first_write, second_write) in first_bindings.iter().zip(second_bindings.iter()) {
            prop_assert_eq!(first_write.node_id(), second_write.node_id());
            let pair = (
                first_write.value().as_scalar(),
                second_write.value().as_scalar(),
            );
            prop_assert!(pair.0.is_some() && pair.1.is_some());
            if let (Some(first_v), Some(second_v)) = pair {
                prop_assert_eq!(first_v.to_bits(), second_v.to_bits());
            }
        }
    }
}
