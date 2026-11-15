//! Graph and tween sims: giant and empty graphs, mutation counting against a
//! model, snapshot isolation between branches, and tweens judged by a
//! reference sampler that sees only the inputs.

use std::collections::{BTreeMap, BTreeSet};

use drishti::motion::{
    AccumulatorStore, BindingTarget, ConflictPolicy, Easing, MotionErrorCode, MotionGraph,
    MotionGraphBuilder, MotionNode, NamedCurve, NodeKind, TweenComponent,
};
use lgwks_std::id::Uuid;

use crate::fixtures::all_named_curves;
use crate::motion_kit::{
    describe, ensure, eval_at, linear_tween, node_id, reference_linear, scalar_of, single_node,
    within,
};
use crate::rng::SimRng;

/// Tween parameters by node: from, to, start, duration.
type Specs = BTreeMap<Uuid, (f32, f32, f32, f32)>;

/// A seeded tween node on a fixed target.
fn tween_node(random: &mut SimRng, id: Uuid) -> Result<MotionNode, String> {
    let to = 1.0 + f32::from(random.below_u16(9));
    let tween = linear_tween(0.0, to, 0.0, 100.0 + f32::from(random.below_u16(900)))?;
    Ok(MotionNode::new(
        id,
        BindingTarget::new(String::from(".m"), String::from("opacity")),
        NodeKind::Tween(tween),
    ))
}

/// Bug caught: a frame that drops, repeats or reorders bindings at the
/// edges of size (0, 1, 2, 1,000 and 20,000 nodes), or that disagrees with
/// the reference sampler on any one of them.
#[test]
fn sim_motion_graph_giant_and_empty_frames_match_reference() -> Result<(), String> {
    let mut store = AccumulatorStore::new();
    let empty = eval_at(&MotionGraph::default(), 123.0, &mut store);
    ensure(empty.bindings().is_empty(), "an empty graph wrote bindings")?;
    ensure(
        empty.t_wall_ms().to_bits() == 123.0_f64.to_bits(),
        "an empty frame must still carry its time",
    )?;
    let mut random = SimRng::seed(2_100);
    for count in [1_usize, 2, 1_000, 20_000] {
        let mut builder = MotionGraphBuilder::new();
        let mut specs = Specs::new();
        for _ in 0..count {
            let id = node_id(&mut random);
            let from = f32::from(random.below_u16(400)) - 200.0;
            let to = from + f32::from(random.below_u16(400)) - 200.0;
            let start = f32::from(random.below_u16(1_000));
            let duration = f32::from(random.below_u16(2_000)) + 100.0;
            let tween = linear_tween(from, to, start, duration)?;
            builder = builder.add_node(
                id,
                String::from(".giant"),
                String::from("x"),
                NodeKind::Tween(tween),
            );
            specs.insert(id, (from, to, start, duration));
        }
        let graph = builder.build();
        let wall = f64::from(random.below_u16(3_000));
        let output = eval_at(&graph, wall, &mut store);
        ensure(
            graph.node_count() == specs.len() && output.bindings().len() == specs.len(),
            &format!("{count} nodes: the frame lost or repeated a binding"),
        )?;
        for (write, (id, spec)) in output.bindings().iter().zip(specs.iter()) {
            let want = reference_linear(spec.0, spec.1, spec.2, spec.3, wall);
            let got = write.value().as_scalar().ok_or("binding must be scalar")?;
            ensure(
                write.node_id() == *id && within(got, want, 3e-3),
                &format!("{count} nodes: a binding left key order or the reference"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a version counter that skips or repeats, or a node set that
/// drifts from what the calls asked for, under a monkey walk of the graph API.
#[test]
fn sim_motion_graph_version_counts_every_mutation() -> Result<(), String> {
    const PROPERTIES: [&str; 3] = ["opacity", "translateY", "scale"];
    for seed in 2_200..2_230_u64 {
        let mut random = SimRng::seed(seed);
        let mut pool = Vec::new();
        for _ in 0..12 {
            pool.push(node_id(&mut random));
        }
        let mut graph = MotionGraph::default();
        let mut live = BTreeSet::new();
        let mut policies = BTreeMap::new();
        let mut version = 0_u64;
        for step in 0..300_u64 {
            let id = pool[random.below_len(pool.len())];
            match random.below(4) {
                0 => {
                    graph = graph.add_node(tween_node(&mut random, id)?);
                    live.insert(id);
                }
                1 => {
                    graph = graph.remove_node(&id);
                    live.remove(&id);
                }
                2 => {
                    let node = tween_node(&mut random, id)?;
                    if live.contains(&id) {
                        graph = graph.replace_node(node).map_err(describe)?;
                    } else {
                        // A missing handle is refused and the version stays.
                        ensure(
                            graph.clone().replace_node(node).is_err(),
                            &format!("seed {seed} step {step}: replacing a missing node worked"),
                        )?;
                        continue;
                    }
                }
                _ => {
                    let property = PROPERTIES[random.below_len(PROPERTIES.len())];
                    let policy = match random.below(3) {
                        0 => ConflictPolicy::LastWrite,
                        1 => ConflictPolicy::Compose,
                        _ => ConflictPolicy::Error,
                    };
                    graph = graph.with_conflict_policy(String::from(property), policy.clone());
                    policies.insert(property, policy);
                }
            }
            version = version.saturating_add(1);
            let walked: Vec<Uuid> = graph.nodes().map(MotionNode::node_id).collect();
            let wanted: Vec<Uuid> = live.iter().copied().collect();
            ensure(
                graph.version() == version && graph.node_count() == live.len() && walked == wanted,
                &format!("seed {seed} step {step}: graph disagrees with the model"),
            )?;
            for property in PROPERTIES {
                ensure(
                    graph.conflict_policy(property) == policies.get(property),
                    &format!("seed {seed} step {step}: policy for {property} drifted"),
                )?;
            }
        }
    }
    Ok(())
}

/// Bug caught: shared mutable structure behind `clone`, so a branch edited
/// for one tenant shows up in its siblings or in the base snapshot.
#[test]
fn sim_motion_graph_branches_never_leak_into_siblings() -> Result<(), String> {
    for seed in 2_300..2_320_u64 {
        let mut random = SimRng::seed(seed);
        let mut pool = Vec::new();
        for _ in 0..10 {
            pool.push(node_id(&mut random));
        }
        let base = single_node(pool[0], tween_node(&mut random, pool[0])?.kind().clone());
        let mut branches: Vec<(MotionGraph, BTreeSet<Uuid>, u64)> = Vec::new();
        for _ in 0..8 {
            branches.push((base.clone(), BTreeSet::from([pool[0]]), base.version()));
        }
        for _ in 0..400 {
            let index = random.below_len(branches.len());
            let id = pool[random.below_len(pool.len())];
            let node = tween_node(&mut random, id)?;
            let add = random.below(2) == 0;
            let Some(branch) = branches.get_mut(index) else {
                return Err(String::from("branch index out of range"));
            };
            let owned = std::mem::take(&mut branch.0);
            branch.0 = if add {
                branch.1.insert(id);
                owned.add_node(node)
            } else {
                branch.1.remove(&id);
                owned.remove_node(&id)
            };
            branch.2 = branch.2.saturating_add(1);
        }
        ensure(
            base.node_count() == 1 && base.version() == 1,
            &format!("seed {seed}: the base snapshot was edited through a branch"),
        )?;
        for branch in &branches {
            let seen: BTreeSet<Uuid> = branch.0.nodes().map(MotionNode::node_id).collect();
            ensure(
                branch.0.version() == branch.2 && seen == branch.1,
                &format!("seed {seed}: a branch absorbed a sibling's edits"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a second node with the same identity appended instead of
/// replacing the first, so one target is driven by two bindings.
#[test]
fn sim_motion_graph_duplicate_identity_binds_once_with_the_last_tween() -> Result<(), String> {
    for seed in 2_400..2_440_u64 {
        let mut random = SimRng::seed(seed);
        let id = node_id(&mut random);
        let first = linear_tween(0.0, 10.0, 0.0, 100.0)?;
        let last_to = 100.0 + f32::from(random.below_u16(100));
        let last = linear_tween(100.0, last_to, 0.0, 100.0)?;
        let graph = single_node(id, NodeKind::Tween(first)).add_node(MotionNode::new(
            id,
            BindingTarget::new(String::from(".dup"), String::from("opacity")),
            NodeKind::Tween(last),
        ));
        let mut store = AccumulatorStore::new();
        let output = eval_at(&graph, 1_000_000.0, &mut store);
        ensure(
            graph.node_count() == 1 && graph.version() == 2 && output.bindings().len() == 1,
            &format!(
                "seed {seed}: duplicate identity produced {} bindings",
                output.bindings().len()
            ),
        )?;
        ensure(
            scalar_of(&output, id)?.to_bits() == last_to.to_bits(),
            &format!("seed {seed}: the first tween outlived its replacement"),
        )?;
    }
    Ok(())
}

/// Bug caught: the builder keeping the first declared conflict policy
/// instead of the last, or leaking a policy onto a property never declared.
#[test]
fn sim_motion_graph_builder_policy_is_last_declaration_per_property() -> Result<(), String> {
    const PROPERTIES: [&str; 4] = ["opacity", "translateX", "translateY", "rotate"];
    for seed in 2_500..2_540_u64 {
        let mut random = SimRng::seed(seed);
        let mut builder = MotionGraphBuilder::new();
        let mut last: BTreeMap<&str, ConflictPolicy> = BTreeMap::new();
        let declarations = random.below(12);
        for _ in 0..declarations {
            let property = PROPERTIES[random.below_len(PROPERTIES.len())];
            let policy = if random.below(2) == 0 {
                ConflictPolicy::Compose
            } else {
                ConflictPolicy::Error
            };
            builder = builder.with_conflict_policy(String::from(property), policy.clone());
            last.insert(property, policy);
        }
        let graph = builder.build();
        ensure(
            graph.version() == declarations && graph.node_count() == 0,
            &format!("seed {seed}: declaring policies changed the node set or miscounted"),
        )?;
        for property in PROPERTIES {
            ensure(
                graph.conflict_policy(property) == last.get(property),
                &format!("seed {seed}: policy for {property} is not the last declared"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a tween frame that depends on history, so the incremental
/// frame N differs from frame N computed from the inputs alone.
#[test]
fn sim_motion_tween_incremental_frames_equal_reference_sampler() -> Result<(), String> {
    for seed in 2_600..2_640_u64 {
        let mut random = SimRng::seed(seed);
        let from = f32::from(random.below_u16(400)) - 200.0;
        let to = f32::from(random.below_u16(400)) - 200.0;
        let start = f32::from(random.below_u16(500));
        let duration = 100.0 + f32::from(random.below_u16(900));
        let id = node_id(&mut random);
        let graph = single_node(
            id,
            NodeKind::Tween(linear_tween(from, to, start, duration)?),
        );
        let mut incremental = AccumulatorStore::new();
        let mut wall = f64::from(start) - 200.0;
        for step in 0..80_u64 {
            wall += f64::from(random.below_u16(60));
            let live = scalar_of(&eval_at(&graph, wall, &mut incremental), id)?;
            let solo = scalar_of(&eval_at(&graph, wall, &mut AccumulatorStore::new()), id)?;
            ensure(
                live.to_bits() == solo.to_bits()
                    && within(
                        live,
                        reference_linear(from, to, start, duration, wall),
                        3e-3,
                    ),
                &format!("seed {seed} step {step}: frame at {wall} left the reference"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: hidden ordering state in the tween path, so sampling the same
/// instants in a shuffled order gives different values than in time order.
#[test]
fn sim_motion_tween_sampling_order_does_not_matter() -> Result<(), String> {
    for seed in 2_700..2_730_u64 {
        let mut random = SimRng::seed(seed);
        let id = node_id(&mut random);
        let graph = single_node(
            id,
            NodeKind::Tween(linear_tween(-50.0, 250.0, 100.0, 700.0)?),
        );
        let mut instants = Vec::new();
        let mut wall = 0.0_f64;
        for _ in 0..64 {
            wall += f64::from(random.below_u16(30));
            instants.push(wall);
        }
        let mut in_order = Vec::new();
        let mut store = AccumulatorStore::new();
        for instant in &instants {
            in_order.push(scalar_of(&eval_at(&graph, *instant, &mut store), id)?.to_bits());
        }
        let mut visit: Vec<usize> = (0..instants.len()).collect();
        for index in (1..visit.len()).rev() {
            let swap = random.below_len(index.saturating_add(1));
            visit.swap(index, swap);
        }
        let mut shuffled = vec![0_u32; instants.len()];
        let mut fresh = AccumulatorStore::new();
        for position in visit {
            shuffled[position] =
                scalar_of(&eval_at(&graph, instants[position], &mut fresh), id)?.to_bits();
        }
        ensure(
            in_order == shuffled,
            &format!("seed {seed}: sampling order changed a tween value"),
        )?;
    }
    Ok(())
}

/// Bug caught: a long chained timeline mis-scheduling its keyframes, where
/// the count of finished, active and pending windows must match arithmetic.
#[test]
fn sim_motion_tween_timeline_activity_counts_match_arithmetic() -> Result<(), String> {
    let mut builder = MotionGraphBuilder::new();
    let mut random = SimRng::seed(2_800);
    for index in 0..20_000_u16 {
        let tween = linear_tween(0.0, 1.0, f32::from(index) * 10.0, 10.0)?;
        builder = builder.add_node(
            node_id(&mut random),
            String::from(".timeline"),
            String::from("progress"),
            NodeKind::Tween(tween),
        );
    }
    let graph = builder.build();
    let mut store = AccumulatorStore::new();
    for wall in [0.0, 5.0, 12_345.0, 199_999.0, 200_000.0, 1.0e9] {
        let (mut done, mut pending, mut active) = (0_usize, 0_usize, 0_usize);
        for write in eval_at(&graph, wall, &mut store).bindings() {
            let value = write.value().as_scalar().ok_or("scalar expected")?;
            if value >= 0.999 {
                done = done.saturating_add(1);
            } else if value <= 0.001 {
                pending = pending.saturating_add(1);
            } else {
                active = active.saturating_add(1);
            }
        }
        let (mut want_done, mut want_active) = (0_usize, 0_usize);
        for index in 0..20_000_u32 {
            let start = f64::from(index) * 10.0;
            if start + 10.0 <= wall {
                want_done = want_done.saturating_add(1);
            } else if start < wall {
                want_active = want_active.saturating_add(1);
            }
        }
        let want_pending = 20_000_usize
            .saturating_sub(want_done)
            .saturating_sub(want_active);
        ensure(
            done == want_done && active == want_active && pending == want_pending,
            &format!("wall {wall}: done {done} active {active} pending {pending}"),
        )?;
    }
    Ok(())
}

/// Bug caught: a named curve that misses its endpoints, which makes a tween
/// start or finish off its declared values.
#[test]
fn sim_motion_tween_endpoints_hold_for_every_named_curve() -> Result<(), String> {
    let mut random = SimRng::seed(2_900);
    for curve in all_named_curves() {
        let from = f32::from(random.below_u16(2_000)) - 1_000.0;
        let to = f32::from(random.below_u16(2_000)) - 1_000.0;
        let id = node_id(&mut random);
        let tween =
            TweenComponent::new(from, to, 100.0, 500.0, Easing::named(curve)).map_err(describe)?;
        let graph = single_node(id, NodeKind::Tween(tween));
        let slack = 1e-4 * (1.0 + f64::from((to - from).abs()));
        let mut store = AccumulatorStore::new();
        for (wall, want) in [
            (-1e9, from),
            (50.0, from),
            (100.0, from),
            (600.0, to),
            (601.0, to),
            (1e9, to),
        ] {
            let got = scalar_of(&eval_at(&graph, wall, &mut store), id)?;
            ensure(
                within(got, f64::from(want), slack),
                &format!("{curve:?} at {wall}: got {got}, want {want}"),
            )?;
        }
    }
    let id = node_id(&mut random);
    let step = TweenComponent::new(3.0, 9.0, 0.0, 400.0, Easing::named(NamedCurve::Instant))
        .map_err(describe)?;
    let graph = single_node(id, NodeKind::Tween(step));
    let mut store = AccumulatorStore::new();
    for (wall, want) in [(0.0, 3.0), (399.0, 3.0), (400.0, 9.0), (900.0, 9.0)] {
        ensure(
            within(
                scalar_of(&eval_at(&graph, wall, &mut store), id)?,
                want,
                1e-6,
            ),
            &format!("instant curve at {wall} is not a step at the end"),
        )?;
    }
    Ok(())
}

/// Bug caught: a sign slip in one of the monotone curves, which makes the
/// value run backwards over time (the oracle is monotonicity, not a table).
#[test]
fn sim_motion_tween_monotone_curves_never_regress() -> Result<(), String> {
    let mut random = SimRng::seed(3_000);
    let monotone: Vec<NamedCurve> = all_named_curves()
        .into_iter()
        .filter(|curve| {
            !matches!(
                *curve,
                NamedCurve::BackIn | NamedCurve::BackOut | NamedCurve::BackInOut
            )
        })
        .collect();
    for round in 0..20_u64 {
        let mut times: Vec<f32> = (0..300)
            .map(|_| f32::from(random.below_u16(10_001)) / 10_000.0)
            .collect();
        times.sort_by(f32::total_cmp);
        for curve in &monotone {
            let easing = Easing::named(*curve);
            let mut previous = easing.value_at(0.0);
            for time in &times {
                let value = easing.value_at(*time);
                ensure(
                    value >= previous - 1e-5 && (-1e-5..=1.0 + 1e-5).contains(&value),
                    &format!(
                        "round {round}: {curve:?} regressed at {time}: {previous} then {value}"
                    ),
                )?;
                previous = value;
            }
        }
    }
    Ok(())
}

/// Bug caught: back curves clamped into [0, 1] (losing their overshoot) or
/// escaping the physical bound of the overshoot constant.
#[test]
fn sim_motion_tween_back_curves_overshoot_within_bounds() -> Result<(), String> {
    for curve in [
        NamedCurve::BackIn,
        NamedCurve::BackOut,
        NamedCurve::BackInOut,
    ] {
        let easing = Easing::named(curve);
        let (mut low, mut high) = (f32::MAX, f32::MIN);
        for step in 0..=1_000_u16 {
            let value = easing.value_at(f32::from(step) / 1_000.0);
            ensure(
                (-0.2..=1.2).contains(&value),
                &format!("{curve:?} left its overshoot bound at step {step}: {value}"),
            )?;
            low = low.min(value);
            high = high.max(value);
        }
        let overshoots = match curve {
            NamedCurve::BackIn => low < -0.05,
            NamedCurve::BackOut => high > 1.05,
            _ => low < -0.05 && high > 1.05,
        };
        ensure(
            overshoots,
            &format!("{curve:?} never overshot: range {low}..{high}"),
        )?;
    }
    Ok(())
}

/// Bug caught: a cubic-bezier that accepts x controls outside [0, 1] (NaN
/// included) or leaves the convex hull of its y controls.
#[test]
fn sim_motion_tween_cubic_bezier_domain_and_hull() -> Result<(), String> {
    for seed in 3_100..3_160_u64 {
        let mut random = SimRng::seed(seed);
        let p1x = f32::from(random.below_u16(1_001)) / 1_000.0;
        let p2x = f32::from(random.below_u16(1_001)) / 1_000.0;
        let p1y = f32::from(random.below_u16(3_001)) / 1_000.0 - 1.0;
        let p2y = f32::from(random.below_u16(3_001)) / 1_000.0 - 1.0;
        let easing = Easing::cubic_bezier(p1x, p1y, p2x, p2y).map_err(describe)?;
        ensure(
            within(easing.value_at(0.0), 0.0, 1e-4) && within(easing.value_at(1.0), 1.0, 1e-4),
            &format!("seed {seed}: bezier endpoints moved"),
        )?;
        let low = f64::from(p1y.min(p2y).min(0.0)) - 1e-3;
        let high = f64::from(p1y.max(p2y).max(1.0)) + 1e-3;
        for _ in 0..200 {
            let value = f64::from(easing.value_at(f32::from(random.below_u16(1_001)) / 1_000.0));
            ensure(
                (low..=high).contains(&value),
                &format!("seed {seed}: bezier value {value} left the hull {low}..{high}"),
            )?;
        }
        for bad in [-0.001, 1.001, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let first = Easing::cubic_bezier(bad, p1y, p2x, p2y);
            let second = Easing::cubic_bezier(p1x, p1y, bad, p2y);
            ensure(
                first.is_err_and(|error| error.code() == MotionErrorCode::EasingDomainError)
                    && second
                        .is_err_and(|error| error.code() == MotionErrorCode::EasingDomainError),
                &format!("seed {seed}: x control {bad} was not refused"),
            )?;
        }
    }
    Ok(())
}
