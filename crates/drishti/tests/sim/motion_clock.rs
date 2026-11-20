//! Clock-fool sims for the motion core: the wall clock rewinds, jumps, goes
//! NaN or infinite, and stays at weird bytes (-0, subnormal, 1e300).
//!
//! The oracles are invariants (a rewound clock moves nothing, state stays
//! finite) and a reference sampler computed from the inputs alone.

use drishti::motion::{
    AccumulatorComponent, AccumulatorStore, Clock, MotionGraph, NodeKind, SpringConfig,
    integrate_spring,
};
use lgwks_std::id::Uuid;

use crate::motion_kit::{
    ensure, eval_at, linear_tween, node_id, reference_linear, scalar_of, single_node,
    spring_config, state_of, within,
};
use crate::rng::SimRng;

/// A stiff, damped UI spring on one node.
fn ui_spring(random: &mut SimRng) -> Result<(MotionGraph, Uuid, SpringConfig), String> {
    let id = node_id(random);
    let config = spring_config(170.0, 26.0, 1.0)?;
    let component = AccumulatorComponent::new_spring(config);
    Ok((
        single_node(id, NodeKind::Accumulator(component)),
        id,
        config,
    ))
}

/// Bug caught: a rewound clock integrating negative time, so a spring
/// moves backwards or diverges when the host clock steps back.
#[test]
fn sim_motion_clock_rewind_spring_holds_state() -> Result<(), String> {
    for seed in 1..=40_u64 {
        let mut random = SimRng::seed(seed);
        let (graph, id, _) = ui_spring(&mut random)?;
        let mut store = AccumulatorStore::new();
        let mut wall_ms = 5_000.0_f64;
        for _ in 0..random.below(30).saturating_add(3) {
            wall_ms += f64::from(random.below_u16(40)) + 1.0;
            scalar_of(&eval_at(&graph, wall_ms, &mut store), id)?;
        }
        let before = state_of(&store, id)?;
        let rewind = f64::from(random.below_u16(5_000)) + 1.0;
        let held = scalar_of(&eval_at(&graph, wall_ms - rewind, &mut store), id)?;
        let after = state_of(&store, id)?;
        ensure(
            after.position().to_bits() == before.position().to_bits()
                && after.velocity().to_bits() == before.velocity().to_bits()
                && held.to_bits() == before.position().to_bits(),
            &format!("seed {seed}: rewinding {rewind} ms moved the spring"),
        )?;
    }
    Ok(())
}

/// Bug caught: a NaN wall clock poisoning the spring state for good, because
/// the NaN is recorded as the last time and every later delta is NaN too.
#[test]
fn sim_motion_clock_nan_never_poisons_spring() -> Result<(), String> {
    const NANS: [u64; 3] = [
        0x7ff8_0000_0000_0000,
        0x7ff8_dead_beef_0001,
        0xfff8_0000_0000_0000,
    ];
    for seed in 100..140_u64 {
        let mut random = SimRng::seed(seed);
        let (graph, id, _) = ui_spring(&mut random)?;
        let mut store = AccumulatorStore::new();
        let mut wall_ms = 1_000.0_f64;
        for step in 0..120_u64 {
            let clock = if random.below(6) == 0 {
                f64::from_bits(
                    *NANS
                        .get(random.below_len(NANS.len()))
                        .ok_or("NaN index out of range")?,
                )
            } else {
                wall_ms += 16.0;
                wall_ms
            };
            let position = scalar_of(&eval_at(&graph, clock, &mut store), id)?;
            ensure(
                position.is_finite() && position.abs() < 3.0,
                &format!("seed {seed} step {step}: NaN clock poisoned the spring"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: an infinite wall clock stored as the last time, which makes
/// every later delta infinite and teleports or freezes the spring forever.
#[test]
fn sim_motion_clock_infinity_is_never_remembered() -> Result<(), String> {
    for seed in 150..190_u64 {
        let mut random = SimRng::seed(seed);
        let (graph, id, _) = ui_spring(&mut random)?;
        let mut store = AccumulatorStore::new();
        scalar_of(&eval_at(&graph, 2_000.0, &mut store), id)?;
        let hostile = if random.below(2) == 0 {
            f64::INFINITY
        } else {
            f64::NEG_INFINITY
        };
        scalar_of(&eval_at(&graph, hostile, &mut store), id)?;
        let remembered = state_of(&store, id)?.last_t_wall_ms();
        ensure(
            remembered.is_some_and(f64::is_finite),
            &format!("seed {seed}: an infinite clock was recorded as the last time"),
        )?;
        let settled = scalar_of(&eval_at(&graph, 2_016.0, &mut store), id)?;
        ensure(
            settled.is_finite() && settled.abs() < 3.0,
            &format!("seed {seed}: spring left unstable after an infinite clock"),
        )?;
    }
    Ok(())
}

/// Bug caught: a frame gap above the explode bound integrating instead of
/// teleporting, which blows a spring up after a long suspend.
#[test]
fn sim_motion_clock_huge_jump_teleports_spring_to_target() -> Result<(), String> {
    for seed in 200..260_u64 {
        let mut random = SimRng::seed(seed);
        let (graph, id, _) = ui_spring(&mut random)?;
        let mut store = AccumulatorStore::new();
        scalar_of(&eval_at(&graph, 1_000.0, &mut store), id)?;
        scalar_of(&eval_at(&graph, 1_016.0, &mut store), id)?;
        let jump = 251.0 + f64::from(random.below_u32(400_000));
        let landed = scalar_of(&eval_at(&graph, 1_016.0 + jump, &mut store), id)?;
        let state = state_of(&store, id)?;
        ensure(
            landed.to_bits() == 1.0_f32.to_bits()
                && state.position().to_bits() == 1.0_f32.to_bits()
                && state.velocity().to_bits() == 0.0_f32.to_bits(),
            &format!("seed {seed}: a {jump} ms gap did not teleport to the target"),
        )?;
    }
    Ok(())
}

/// Bug caught: gaps between the clamp and the explode bound being teleported
/// (or left unclamped) instead of integrated with the capped delta.
#[test]
fn sim_motion_clock_mid_band_gap_is_clamped_not_teleported() -> Result<(), String> {
    for seed in 300..360_u64 {
        let mut random = SimRng::seed(seed);
        // A gentle spring: one RK4 step covers the whole capped delta.
        let id = node_id(&mut random);
        let config = spring_config(50.0, 8.0, 1.0)?;
        let graph = single_node(
            id,
            NodeKind::Accumulator(AccumulatorComponent::new_spring(config)),
        );
        let mut store = AccumulatorStore::new();
        scalar_of(&eval_at(&graph, 1_000.0, &mut store), id)?;
        scalar_of(&eval_at(&graph, 1_016.0, &mut store), id)?;
        let before = state_of(&store, id)?;
        let gap = match random.below(3) {
            0 => 65.0,
            1 => 250.0,
            _ => 65.0 + f64::from(random.below_u16(185)),
        };
        scalar_of(&eval_at(&graph, 1_016.0 + gap, &mut store), id)?;
        let after = state_of(&store, id)?;
        let (want_position, want_velocity) =
            integrate_spring(before.position(), before.velocity(), 1.0, &config, 0.064);
        ensure(
            within(after.position(), f64::from(want_position), 1e-5)
                && within(after.velocity(), f64::from(want_velocity), 1e-4),
            &format!("seed {seed}: a {gap} ms gap was not integrated at the 64 ms cap"),
        )?;
    }
    Ok(())
}

/// Bug caught: signed zero and a zero delta treated as elapsed time, so two
/// frames stamped the same instant (0.0 and -0.0) still move the spring.
#[test]
fn sim_motion_clock_same_instant_twice_is_idempotent() -> Result<(), String> {
    for seed in 400..430_u64 {
        let mut random = SimRng::seed(seed);
        let (graph, id, _) = ui_spring(&mut random)?;
        let mut store = AccumulatorStore::new();
        scalar_of(&eval_at(&graph, -0.0, &mut store), id)?;
        let first = state_of(&store, id)?;
        for _ in 0..random.below(20).saturating_add(2) {
            let stamp = if random.below(2) == 0 { 0.0 } else { -0.0 };
            scalar_of(&eval_at(&graph, stamp, &mut store), id)?;
            let again = state_of(&store, id)?;
            ensure(
                again.position().to_bits() == first.position().to_bits()
                    && again.velocity().to_bits() == first.velocity().to_bits(),
                &format!("seed {seed}: a zero delta moved the spring"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: extreme or subnormal wall times escaping the tween clamp, so
/// a binding lands outside the endpoints the author declared.
#[test]
fn sim_motion_clock_extreme_wall_values_clamp_tween_to_endpoints() -> Result<(), String> {
    const WALLS: [f64; 12] = [
        1e300,
        -1e300,
        f64::MAX,
        f64::MIN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        1e-310,
        -1e-310,
        5e-324,
        -5e-324,
        0.0,
        -0.0,
    ];
    for seed in 500..540_u64 {
        let mut random = SimRng::seed(seed);
        let from = f32::from(random.below_u16(200)) - 100.0;
        let to = from + f32::from(random.below_u16(200)) + 1.0;
        let id = node_id(&mut random);
        let graph = single_node(id, NodeKind::Tween(linear_tween(from, to, 0.0, 1_000.0)?));
        let mut store = AccumulatorStore::new();
        for wall in WALLS {
            let got = scalar_of(&eval_at(&graph, wall, &mut store), id)?;
            let want = reference_linear(from, to, 0.0, 1_000.0, wall);
            ensure(
                within(got, want, 1e-3),
                &format!("seed {seed}: wall {wall:e} gave {got}, reference {want}"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a NaN frame leaving hidden state in the stateless tween path,
/// so the frame after the bad clock is wrong too.
#[test]
fn sim_motion_clock_nan_tween_recovers_on_next_frame() -> Result<(), String> {
    for seed in 600..640_u64 {
        let mut random = SimRng::seed(seed);
        let from = f32::from(random.below_u16(100));
        let to = from + 50.0;
        let id = node_id(&mut random);
        let graph = single_node(id, NodeKind::Tween(linear_tween(from, to, 100.0, 400.0)?));
        let mut store = AccumulatorStore::new();
        let healthy = 100.0 + f64::from(random.below_u16(400));
        let before = scalar_of(&eval_at(&graph, healthy, &mut store), id)?;
        scalar_of(&eval_at(&graph, f64::NAN, &mut store), id)?;
        let after = scalar_of(&eval_at(&graph, healthy, &mut store), id)?;
        ensure(
            before.to_bits() == after.to_bits()
                && within(
                    after,
                    reference_linear(from, to, 100.0, 400.0, healthy),
                    1e-3,
                ),
            &format!("seed {seed}: the tween kept state across a NaN frame"),
        )?;
        ensure(
            store.get(&id).is_none(),
            &format!("seed {seed}: a tween frame created accumulator state"),
        )?;
    }
    Ok(())
}

/// Bug caught: `advance_ms` mutating the clock in place, or dropping the sign
/// of a negative delta, so replays drift from the declared timeline.
#[test]
fn sim_motion_clock_advance_is_pure_and_additive() -> Result<(), String> {
    for seed in 700..760_u64 {
        let mut random = SimRng::seed(seed);
        let base = f64::from(random.below_u32(1_000_000));
        let origin = Clock::deterministic(base);
        let mut moving = origin.clone();
        let mut expected = base;
        for _ in 0..200 {
            let delta = f64::from(random.below_u16(2_000)) - 1_000.0;
            let previous = moving.wall_clock_ms().to_bits();
            let next = moving.advance_ms(delta);
            ensure(
                moving.wall_clock_ms().to_bits() == previous,
                &format!("seed {seed}: advance_ms mutated its receiver"),
            )?;
            expected += delta;
            moving = next;
            ensure(
                moving.wall_clock_ms().to_bits() == expected.to_bits(),
                &format!("seed {seed}: clock drifted from the f64 running sum"),
            )?;
        }
        ensure(
            origin.wall_clock_ms().to_bits() == base.to_bits(),
            &format!("seed {seed}: the origin clock moved"),
        )?;
    }
    Ok(())
}

/// Bug caught: the first spring frame using zero (a stall) or the wall time
/// itself as its delta, which depends on when the process started.
#[test]
fn sim_motion_clock_first_frame_uses_one_sixtieth_regardless_of_epoch() -> Result<(), String> {
    for seed in 800..860_u64 {
        let mut random = SimRng::seed(seed);
        let id = node_id(&mut random);
        let tension = 50.0 + f32::from(random.below_u16(400));
        let friction = f32::from(random.below_u16(60));
        let config = spring_config(tension, friction, 1.0)?;
        let graph = single_node(
            id,
            NodeKind::Accumulator(AccumulatorComponent::new_spring(config)),
        );
        let epoch = f64::from(random.below_u32(u32::MAX)) * 1_000.0 - 1.0e12;
        let mut store = AccumulatorStore::new();
        let got = scalar_of(&eval_at(&graph, epoch, &mut store), id)?;
        let (want, _) = integrate_spring(0.0, 0.0, 1.0, &config, 0.0166);
        ensure(
            within(got, f64::from(want), 1e-6) && got > 0.0,
            &format!("seed {seed}: first frame at epoch {epoch} used the wrong delta"),
        )?;
    }
    Ok(())
}
