//! Spring, accumulator and critical-spring sims: hostile parameters, lease
//! rotation against a reference model, wrong-order store calls, and physical
//! invariants (fixed point, energy, no overshoot) as the oracles.

use std::collections::BTreeSet;

use drishti::motion::node::critical::CriticalSpring;
use drishti::motion::{
    AccumulatorComponent, AccumulatorState, AccumulatorStore, MotionErrorCode, NodeKind,
    SpringConfig, integrate_spring,
};

use crate::motion_kit::{
    describe, ensure, eval_at, node_id, scalar_of, single_node, spring_config, state_of, within,
};
use crate::rng::SimRng;

/// Bug caught: a spring constructor that lets NaN, infinity, negatives or a
/// zero mass through, which later integrates into garbage.
#[test]
fn sim_motion_spring_hostile_parameters_match_the_domain_predicate() -> Result<(), String> {
    const WEIRD: [f32; 12] = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        -0.0,
        0.0,
        f32::MIN_POSITIVE,
        1.0e-45,
        -1.0e-45,
        -1.0,
        1.0,
        f32::MAX,
        f32::MIN,
    ];
    for tension in WEIRD {
        for friction in WEIRD {
            for mass in WEIRD {
                let want = tension.is_finite()
                    && tension >= 0.0
                    && friction.is_finite()
                    && friction >= 0.0
                    && mass.is_finite()
                    && mass > 0.0;
                match SpringConfig::new(tension, friction, mass) {
                    Ok(config) => ensure(
                        want && config.tension().to_bits() == tension.to_bits()
                            && config.friction().to_bits() == friction.to_bits()
                            && config.mass().to_bits() == mass.to_bits(),
                        &format!("accepted ({tension}, {friction}, {mass}) against the domain"),
                    )?,
                    Err(error) => ensure(
                        !want && error.code() == MotionErrorCode::SpringUnstable,
                        &format!("refused ({tension}, {friction}, {mass}) with {error}"),
                    )?,
                }
            }
        }
    }
    Ok(())
}

/// Bug caught: an integrator or delta clamp that lets a valid spring escape
/// the physical envelope (target 1, start 0, so position stays in [-0.5, 2.5]).
#[test]
fn sim_motion_spring_stays_bounded_for_seeded_configs() -> Result<(), String> {
    for seed in 900..940_u64 {
        let mut random = SimRng::seed(seed);
        let tension = f32::from(random.below_u16(400));
        let friction = f32::from(random.below_u16(60));
        let mass = 0.5 + f32::from(random.below_u16(45)) / 10.0;
        let id = node_id(&mut random);
        let component = AccumulatorComponent::new_spring(spring_config(tension, friction, mass)?);
        let graph = single_node(id, NodeKind::Accumulator(component));
        let mut store = AccumulatorStore::new();
        let mut wall_ms = 0.0_f64;
        for step in 0..600_u64 {
            wall_ms += f64::from(random.below_u16(64)) + 1.0;
            let position = scalar_of(&eval_at(&graph, wall_ms, &mut store), id)?;
            ensure(
                position.is_finite() && (-0.5..=2.5).contains(&position),
                &format!(
                    "seed {seed} step {step}: k={tension} c={friction} m={mass} left the envelope at {position}"
                ),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a lease sweep with an off-by-one, which reaps live springs or
/// keeps dead ones. The model: survivors are exactly the nodes touched in the
/// generation that just ended.
#[test]
fn sim_motion_accumulator_lease_survivors_match_touched_model() -> Result<(), String> {
    for seed in 1_000..1_040_u64 {
        let mut random = SimRng::seed(seed);
        let mut store = AccumulatorStore::new();
        let mut ids = Vec::new();
        for _ in 0..24 {
            ids.push(node_id(&mut random));
        }
        let mut touched = BTreeSet::new();
        for round in 0..40_u64 {
            for id in &ids {
                if random.below(3) == 0 {
                    store.get_or_default(*id);
                    touched.insert(*id);
                }
            }
            store.rotate_and_reap();
            for id in &ids {
                ensure(
                    store.get(id).is_some() == touched.contains(id),
                    &format!("seed {seed} round {round}: lease sweep disagrees with the model"),
                )?;
            }
            touched.clear();
        }
    }
    Ok(())
}

/// Bug caught: `set` trusting the identity inside the state it is given, so
/// one node's state can be filed under another node's key, and a stored
/// state escaping the lease clock.
#[test]
fn sim_motion_accumulator_set_pins_identity_and_lease() -> Result<(), String> {
    for seed in 1_050..1_090_u64 {
        let mut random = SimRng::seed(seed);
        let mut store = AccumulatorStore::new();
        let key = node_id(&mut random);
        let impostor = node_id(&mut random);
        for _ in 0..random.below(5) {
            store.rotate_and_reap();
        }
        store.set(key, AccumulatorState::zero(impostor));
        let held = state_of(&store, key)?;
        ensure(
            held.node_id() == key && store.get(&impostor).is_none(),
            &format!("seed {seed}: set filed a state under the wrong identity"),
        )?;
        store.rotate_and_reap();
        ensure(
            store.get(&key).is_some(),
            &format!("seed {seed}: a freshly set state did not survive one sweep"),
        )?;
        store.rotate_and_reap();
        ensure(
            store.get(&key).is_none(),
            &format!("seed {seed}: an unrenewed state survived two sweeps"),
        )?;
    }
    Ok(())
}

/// Bug caught: removed state resurrected with stale velocity, so a node that
/// was dropped and re-added does not restart from rest.
#[test]
fn sim_motion_accumulator_remove_restarts_spring_from_rest() -> Result<(), String> {
    for seed in 1_100..1_140_u64 {
        let mut random = SimRng::seed(seed);
        let id = node_id(&mut random);
        let config = spring_config(120.0 + f32::from(random.below_u16(100)), 12.0, 1.0)?;
        let graph = single_node(
            id,
            NodeKind::Accumulator(AccumulatorComponent::new_spring(config)),
        );
        let mut store = AccumulatorStore::new();
        let mut wall_ms = 100.0_f64;
        for _ in 0..random.below(40).saturating_add(5) {
            wall_ms += 16.0;
            scalar_of(&eval_at(&graph, wall_ms, &mut store), id)?;
        }
        store.remove(&id);
        ensure(
            store.get(&id).is_none(),
            &format!("seed {seed}: remove left the state behind"),
        )?;
        wall_ms += 16.0;
        let restarted = scalar_of(&eval_at(&graph, wall_ms, &mut store), id)?;
        let (want, _) = integrate_spring(0.0, 0.0, 1.0, &config, 0.0166);
        ensure(
            within(restarted, f64::from(want), 1e-6),
            &format!("seed {seed}: a removed node did not restart from rest"),
        )?;
    }
    Ok(())
}

/// Bug caught: a drag accumulator integrating or reacting to the clock, so a
/// held position drifts when time rewinds, jumps, or goes NaN.
#[test]
fn sim_motion_drag_holds_stored_position_through_any_clock() -> Result<(), String> {
    for seed in 1_150..1_190_u64 {
        let mut random = SimRng::seed(seed);
        let id = node_id(&mut random);
        let graph = single_node(id, NodeKind::Accumulator(AccumulatorComponent::new_drag()));
        let mut store = AccumulatorStore::new();
        let held = f32::from(random.below_u16(1000)) / 7.0;
        let mut state = AccumulatorState::zero(id);
        state.set_position(held);
        store.set(id, state);
        for step in 0..100_u64 {
            let wall = match random.below(5) {
                0 => f64::NAN,
                1 => f64::INFINITY,
                2 => -f64::from(random.below_u32(1_000_000)),
                _ => f64::from(random.below_u32(1_000_000_000)),
            };
            let output = scalar_of(&eval_at(&graph, wall, &mut store), id)?;
            ensure(
                output.to_bits() == held.to_bits(),
                &format!("seed {seed} step {step}: drag moved at wall {wall}"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: an equilibrium that drifts, so a spring at rest on its target
/// creeps because the force term has a stray constant.
#[test]
fn sim_motion_spring_equilibrium_is_a_fixed_point() -> Result<(), String> {
    for seed in 1_200..1_300_u64 {
        let mut random = SimRng::seed(seed);
        let config = spring_config(
            f32::from(random.below_u16(500)),
            f32::from(random.below_u16(80)),
            0.25 + f32::from(random.below_u16(80)) / 8.0,
        )?;
        let dt = f32::from(random.below_u16(64)) / 1000.0;
        let (position, velocity) = integrate_spring(1.0, 0.0, 1.0, &config, dt);
        ensure(
            position.to_bits() == 1.0_f32.to_bits() && velocity.to_bits() == 0.0_f32.to_bits(),
            &format!("seed {seed}: rest on target moved to ({position}, {velocity})"),
        )?;
    }
    Ok(())
}

/// Bug caught: a sign error on the damping term, which makes friction add
/// energy so a damped spring rings up instead of settling.
#[test]
fn sim_motion_spring_energy_never_grows_under_friction() -> Result<(), String> {
    for seed in 1_300..1_340_u64 {
        let mut random = SimRng::seed(seed);
        let tension = 40.0 + f32::from(random.below_u16(260));
        let friction = 5.0 + f32::from(random.below_u16(55));
        let mass = 0.5 + f32::from(random.below_u16(15)) / 10.0;
        let config = spring_config(tension, friction, mass)?;
        let energy = |position: f32, velocity: f32| -> f64 {
            let offset = f64::from(position) - 1.0;
            0.5 * f64::from(mass) * f64::from(velocity) * f64::from(velocity)
                + 0.5 * f64::from(tension) * offset * offset
        };
        let (mut position, mut velocity) = (0.0_f32, f32::from(random.below_u16(40)) - 20.0);
        let mut last = energy(position, velocity);
        for step in 0..120_u64 {
            (position, velocity) = integrate_spring(position, velocity, 1.0, &config, 0.016);
            let now = energy(position, velocity);
            ensure(
                now <= last * 1.0001 + 1e-8,
                &format!("seed {seed} step {step}: energy rose from {last} to {now}"),
            )?;
            last = now;
        }
    }
    Ok(())
}

/// Bug caught: retargeting that resets momentum or accepts a hostile target,
/// which snaps a moving presentation or lets NaN into the spring.
#[test]
fn sim_motion_critical_retarget_preserves_momentum() -> Result<(), String> {
    for seed in 1_400..1_440_u64 {
        let mut random = SimRng::seed(seed);
        let start = f64::from(random.below_u16(2_000)) - 1_000.0;
        let response = 0.1 + f64::from(random.below_u16(90)) / 10.0;
        let mut spring = CriticalSpring::new(start, response).map_err(describe)?;
        spring.retarget(start + 100.0).map_err(describe)?;
        spring
            .advance(0.05 + f64::from(random.below_u16(100)) / 100.0)
            .map_err(describe)?;
        let (position, velocity) = (spring.position(), spring.velocity());
        let aim = f64::from(random.below_u16(500));
        spring.retarget(aim).map_err(describe)?;
        ensure(
            spring.position().to_bits() == position.to_bits()
                && spring.velocity().to_bits() == velocity.to_bits()
                && spring.target().to_bits() == aim.to_bits(),
            &format!("seed {seed}: retarget disturbed position or velocity"),
        )?;
        for hostile in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            1.0e9 + 1.0,
            -1.0e9 - 1.0,
        ] {
            match spring.retarget(hostile) {
                Ok(()) => return Err(format!("seed {seed}: accepted target {hostile}")),
                Err(error) => ensure(
                    error.code() == MotionErrorCode::SpringUnstable
                        && spring.target().to_bits() == aim.to_bits(),
                    &format!("seed {seed}: refusal of {hostile} changed the target"),
                )?,
            }
        }
    }
    Ok(())
}

/// Bug caught: a closed form that is not a semigroup (advance a then b is not
/// advance a+b), which means the picture depends on the frame rate.
#[test]
fn sim_motion_critical_advance_composes_like_a_semigroup() -> Result<(), String> {
    for seed in 1_500..1_560_u64 {
        let mut random = SimRng::seed(seed);
        let start = f64::from(random.below_u16(1_000));
        let response = 0.05 + f64::from(random.below_u16(100)) / 10.0;
        let first = f64::from(random.below_u16(500)) / 100.0;
        let second = f64::from(random.below_u16(500)) / 100.0;
        let mut split = CriticalSpring::new(start, response.max(0.016)).map_err(describe)?;
        let mut whole = split;
        let aim = start + 250.0;
        split.retarget(aim).map_err(describe)?;
        whole.retarget(aim).map_err(describe)?;
        split.advance(first).map_err(describe)?;
        split.advance(second).map_err(describe)?;
        whole.advance(first + second).map_err(describe)?;
        let slack = 1e-8 * (1.0 + start + 250.0);
        ensure(
            (split.position() - whole.position()).abs() <= slack
                && (split.velocity() - whole.velocity()).abs() <= slack * 100.0,
            &format!("seed {seed}: advance({first}) then advance({second}) differs from the sum"),
        )?;
    }
    Ok(())
}

/// Bug caught: an out-of-domain elapsed time half applied, so the spring
/// moves even though the call reports an error.
#[test]
fn sim_motion_critical_rejects_out_of_domain_time_untouched() -> Result<(), String> {
    const HOSTILE: [f64; 7] = [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -0.001,
        60.000_001,
        f64::MAX,
        f64::MIN,
    ];
    for seed in 1_600..1_640_u64 {
        let mut random = SimRng::seed(seed);
        let mut spring =
            CriticalSpring::new(f64::from(random.below_u16(100)), 0.45).map_err(describe)?;
        spring.retarget(500.0).map_err(describe)?;
        spring.advance(0.1).map_err(describe)?;
        let (position, velocity) = (spring.position(), spring.velocity());
        for hostile in HOSTILE {
            ensure(
                spring.advance(hostile).is_err()
                    && spring.position().to_bits() == position.to_bits()
                    && spring.velocity().to_bits() == velocity.to_bits(),
                &format!("seed {seed}: advance({hostile}) was not refused cleanly"),
            )?;
        }
        for quiet in [0.0, -0.0] {
            spring.advance(quiet).map_err(describe)?;
            ensure(
                spring.position().to_bits() == position.to_bits(),
                &format!("seed {seed}: a zero-length advance moved the spring"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: constructor bounds off by one at the exact edges of the
/// declared domain (1e9 position, 0.016 to 10 second response).
#[test]
fn sim_motion_critical_construction_matches_the_domain_edges() -> Result<(), String> {
    let positions = [
        0.0,
        1.0e9,
        -1.0e9,
        1.0e9 + 1.0,
        -1.0e9 - 1.0,
        f64::NAN,
        f64::INFINITY,
    ];
    let responses = [
        0.016,
        0.015_999_9,
        10.0,
        10.000_000_1,
        f64::NAN,
        f64::INFINITY,
        0.0,
        -1.0,
    ];
    for position in positions {
        for response in responses {
            let want = position.is_finite()
                && position.abs() <= 1.0e9
                && response.is_finite()
                && (0.016..=10.0).contains(&response);
            ensure(
                CriticalSpring::new(position, response).is_ok() == want,
                &format!("new({position}, {response}) disagrees with the domain"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a sign error in the closed form that overshoots, since a
/// critically damped spring released at rest must approach monotonically.
#[test]
fn sim_motion_critical_approach_is_monotone_and_snap_is_exact() -> Result<(), String> {
    for seed in 1_700..1_740_u64 {
        let mut random = SimRng::seed(seed);
        let start = f64::from(random.below_u16(1_000)) - 500.0;
        let aim = f64::from(random.below_u16(1_000)) - 500.0;
        let mut spring = CriticalSpring::new(start, 0.2 + f64::from(random.below_u16(80)) / 10.0)
            .map_err(describe)?;
        spring.retarget(aim).map_err(describe)?;
        let mut gap = (spring.position() - aim).abs();
        for step in 0..600_u64 {
            spring.advance(0.1).map_err(describe)?;
            let now = (spring.position() - aim).abs();
            ensure(
                now <= gap + 1e-9,
                &format!("seed {seed} step {step}: overshoot, gap grew from {gap} to {now}"),
            )?;
            gap = now;
        }
        ensure(
            spring.is_settled(),
            &format!("seed {seed}: never settled in 60 s"),
        )?;
        spring.snap();
        ensure(
            spring.position().to_bits() == aim.to_bits()
                && spring.velocity().to_bits() == 0.0_f64.to_bits(),
            &format!("seed {seed}: snap did not land exactly on the target"),
        )?;
    }
    Ok(())
}
