//! Accumulator replay determinism: replaying from frame zero with the same
//! inputs reproduces the final state byte-equally.
//!
//! Counterpart of `motion-core/tests/property_accumulator_replay.rs`
//! (kernel#850 row 15). Strategies yield raw tuples; construction happens in
//! the test bodies so rejections surface as test failures, never panics.

use drishti::motion::node::accumulator::{AccumulatorState, AccumulatorStore};
use drishti::motion::node::spring::{SpringConfig, integrate_spring};
use lgwks_std::id::Uuid;
use proptest::prelude::*;

/// Valid spring parameter ranges (tension, friction, mass).
fn arb_spring_params() -> impl Strategy<Value = (f32, f32, f32)> {
    (1.0_f32..500.0_f32, 1.0_f32..100.0_f32, 0.1_f32..10.0_f32)
}

/// Build a config or fail the case.
fn make_config(tension: f32, friction: f32, mass: f32) -> Result<SpringConfig, TestCaseError> {
    SpringConfig::new(tension, friction, mass)
        .map_err(|error| TestCaseError::fail(format!("valid spring rejected: {error}")))
}

/// Replay a spring for `steps` frames from rest.
fn replay_spring(config: &SpringConfig, steps: u32) -> (f32, f32) {
    let mut position = 0.0_f32;
    let mut velocity = 0.0_f32;
    let dt = 0.0166_f32;
    for _ in 0..steps {
        let (next_position, next_velocity) = integrate_spring(position, velocity, 1.0, config, dt);
        position = next_position;
        velocity = next_velocity;
    }
    (position, velocity)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn replay_reproduces_final_state_byte_equal(
        (tension, friction, mass) in arb_spring_params(),
        steps in 1_u32..300_u32,
    ) {
        let config = make_config(tension, friction, mass)?;
        let (first_position, first_velocity) = replay_spring(&config, steps);
        let (second_position, second_velocity) = replay_spring(&config, steps);
        prop_assert_eq!(first_position.to_bits(), second_position.to_bits());
        prop_assert_eq!(first_velocity.to_bits(), second_velocity.to_bits());
    }

    #[test]
    fn replay_preserves_velocity_trajectory_with_store(
        (tension, friction, mass) in arb_spring_params(),
        steps in 1_u32..200_u32,
    ) {
        let config = make_config(tension, friction, mass)?;
        let node_id = Uuid::new_v4()
            .map_err(|error| TestCaseError::fail(format!("id mint failed: {error:?}")))?;
        let mut first_store = AccumulatorStore::new();
        let mut second_store = AccumulatorStore::new();
        let dt = 0.0166_f32;

        for _ in 0..steps {
            let first_state = first_store.get_or_default(node_id);
            let (first_p, first_v) = integrate_spring(
                first_state.position(),
                first_state.velocity(),
                1.0,
                &config,
                dt,
            );
            first_state.set_position(first_p);
            first_state.set_velocity(first_v);

            let second_state = second_store.get_or_default(node_id);
            let (second_p, second_v) = integrate_spring(
                second_state.position(),
                second_state.velocity(),
                1.0,
                &config,
                dt,
            );
            second_state.set_position(second_p);
            second_state.set_velocity(second_v);
        }

        let first = match first_store.get(&node_id) {
            Some(state) => *state,
            None => AccumulatorState::zero(node_id),
        };
        let second = match second_store.get(&node_id) {
            Some(state) => *state,
            None => AccumulatorState::zero(node_id),
        };
        prop_assert_eq!(first.velocity().to_bits(), second.velocity().to_bits());
    }
}
