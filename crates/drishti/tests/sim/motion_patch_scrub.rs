//! Patch, scrub and monkey sims: precedence of patch refusals against an
//! ordered reference predicate, no side effect from a failed patch, scrub
//! sources judged by clamp invariants, and a monkey that walks the motion
//! API on one seed while a model tracks what must be bound.

use std::collections::BTreeMap;

use drishti::motion::{
    AccumulatorComponent, AccumulatorState, AccumulatorStore, Clock, ConstantScrub, Easing,
    HashScrubMap, MotionErrorCode, MotionGraph, MotionNode, MotionPatch, NamedCurve, NodeKind,
    PatchOp, PatchValue, ScrubSourceMap, apply_patch, frame,
};
use lgwks_std::id::Uuid;

use crate::motion_kit::{
    describe, ensure, eval_at, linear_tween, node_id, reference_linear, scalar_of, single_node,
    spring_config, state_of, within,
};
use crate::rng::{SimRng, TraceHash};

/// Sample one node of a graph against an explicit scrub map at time zero.
fn scrub_frame(
    graph: &MotionGraph,
    scrubs: &dyn ScrubSourceMap,
    store: &mut AccumulatorStore,
    id: Uuid,
) -> Result<f32, String> {
    let output = frame(graph, &Clock::deterministic(0.0), store, scrubs);
    scalar_of(&output, id)
}

/// A one-node scrub graph bound to `source`.
fn scrub_graph(id: Uuid, source: &str) -> Result<MotionGraph, String> {
    let component = AccumulatorComponent::new_scrub(String::from(source)).map_err(describe)?;
    Ok(single_node(id, NodeKind::Accumulator(component)))
}

/// A float patch on one tween field.
fn float_op(path: &str, value: f32) -> PatchOp {
    PatchOp::new(String::from(path), PatchValue::Float(value))
}

/// Bug caught: a refused patch that still touches the accumulator store, so
/// a failed request leaks a state slot or disturbs a neighbour's state.
#[test]
fn sim_motion_patch_failure_leaves_the_accumulator_store_untouched() -> Result<(), String> {
    for seed in 3_200..3_260_u64 {
        let mut random = SimRng::seed(seed);
        let id = node_id(&mut random);
        let graph = single_node(id, NodeKind::Tween(linear_tween(0.0, 10.0, 0.0, 500.0)?));
        let mut store = AccumulatorStore::new();
        let bystander = node_id(&mut random);
        let mut held = AccumulatorState::zero(bystander);
        held.set_position(0.25);
        store.set(bystander, held);
        let (bad, expected) = match random.below(5) {
            0 => (float_op("duration_ms", 0.0), MotionErrorCode::PatchInvalid),
            1 => (
                float_op("duration_ms", f32::NAN),
                MotionErrorCode::PatchInvalid,
            ),
            2 => (
                PatchOp::new(String::from("to"), PatchValue::Bool(true)),
                MotionErrorCode::PatchInvalid,
            ),
            3 => (float_op("bogus", 1.0), MotionErrorCode::PatchPathUnknown),
            _ => (float_op("kind", 1.0), MotionErrorCode::PatchPathImmutable),
        };
        let ops = vec![
            float_op("from", -3.0),
            PatchOp::new(
                String::from("easing"),
                PatchValue::Easing(Easing::named(NamedCurve::CubicOut)),
            ),
            bad,
        ];
        let patch = MotionPatch::new(node_id(&mut random), id, ops, true);
        match apply_patch(graph, &patch, &mut store) {
            Ok(_) => return Err(format!("seed {seed}: the failing patch was applied")),
            Err(error) => ensure(
                error.code() == expected,
                &format!("seed {seed}: wanted {expected}, got {error}"),
            )?,
        }
        ensure(
            store.get(&id).is_none()
                && state_of(&store, bystander)?.position().to_bits() == 0.25_f32.to_bits(),
            &format!("seed {seed}: a refused patch touched the store"),
        )?;
    }
    Ok(())
}

/// Bug caught: a patch engine that disagrees with the field model, e.g.
/// applying ops out of order, skipping the final validity rebuild, or
/// accepting a NaN or non-positive duration.
#[test]
fn sim_motion_patch_matches_the_field_model() -> Result<(), String> {
    const PATHS: [&str; 5] = ["from", "to", "start_ms", "duration_ms", "easing"];
    let pool = [
        f32::NAN,
        f32::INFINITY,
        -0.0,
        0.0,
        1.0e-30,
        -5.0,
        250.0,
        40.0,
    ];
    for seed in 3_300..3_400_u64 {
        let mut random = SimRng::seed(seed);
        let id = node_id(&mut random);
        let graph = single_node(id, NodeKind::Tween(linear_tween(2.0, 8.0, 10.0, 300.0)?));
        let mut fields = [2.0_f32, 8.0, 10.0, 300.0];
        let mut easing = Easing::named(NamedCurve::Linear);
        let mut ops = Vec::new();
        for _ in 0..random.below(5).saturating_add(1) {
            let slot = random.below_len(PATHS.len());
            if slot == 4 {
                easing = Easing::named(if random.below(2) == 0 {
                    NamedCurve::QuadIn
                } else {
                    NamedCurve::SineOut
                });
                ops.push(PatchOp::new(
                    String::from("easing"),
                    PatchValue::Easing(easing.clone()),
                ));
            } else {
                let value = if random.below(3) == 0 {
                    pool[random.below_len(pool.len())]
                } else {
                    f32::from(random.below_u16(900)) + 1.0
                };
                fields[slot] = value;
                ops.push(float_op(PATHS[slot], value));
            }
        }
        let valid = fields.iter().all(|field| field.is_finite()) && fields[3] > 0.0;
        let patch = MotionPatch::new(node_id(&mut random), id, ops, false);
        let mut store = AccumulatorStore::new();
        match apply_patch(graph.clone(), &patch, &mut store) {
            Ok(updated) => {
                let node = updated.get_node(&id).ok_or("patched node vanished")?;
                let NodeKind::Tween(ref tween) = *node.kind() else {
                    return Err(format!("seed {seed}: the patch changed the node kind"));
                };
                ensure(
                    valid
                        && updated.version() == graph.version().saturating_add(1)
                        && tween.from_value().to_bits() == fields[0].to_bits()
                        && tween.to_value().to_bits() == fields[1].to_bits()
                        && tween.start_ms().to_bits() == fields[2].to_bits()
                        && tween.duration_ms().to_bits() == fields[3].to_bits()
                        && *tween.easing() == easing,
                    &format!("seed {seed}: the patched tween differs from the model {fields:?}"),
                )?;
            }
            Err(error) => ensure(
                !valid && error.code() == MotionErrorCode::PatchInvalid,
                &format!("seed {seed}: refused a valid model {fields:?} with {error}"),
            )?,
        }
        ensure(
            store.get(&id).is_none(),
            &format!("seed {seed}: a patch without preserve created accumulator state"),
        )?;
    }
    Ok(())
}

/// Bug caught: refusals reported in the wrong order (immutable, then unknown
/// node, then unknown path, then bad value), so a caller is told the wrong
/// thing is wrong. The oracle is an ordered predicate written separately.
#[test]
fn sim_motion_patch_refusals_follow_the_documented_precedence() -> Result<(), String> {
    const PATHS: [&str; 10] = [
        "kind",
        "node_id",
        "accumulator_kind",
        "from",
        "to",
        "easing",
        "duration_ms",
        "bogus",
        "tension",
        "preserve_velocity",
    ];
    for seed in 3_400..3_560_u64 {
        let mut random = SimRng::seed(seed);
        let id = node_id(&mut random);
        let graph = single_node(id, NodeKind::Tween(linear_tween(0.0, 1.0, 0.0, 100.0)?));
        let mut ops = Vec::new();
        let mut wire = Vec::new();
        for _ in 0..random.below(3).saturating_add(1) {
            let path = PATHS[random.below_len(PATHS.len())];
            let kind = random.below(3);
            let value = match kind {
                0 => PatchValue::Float(1.0),
                1 => PatchValue::Bool(true),
                _ => PatchValue::Easing(Easing::named(NamedCurve::Linear)),
            };
            wire.push((path, kind));
            ops.push(PatchOp::new(String::from(path), value));
        }
        let known = random.below(4) != 0;
        let target = if known { id } else { node_id(&mut random) };
        let expected = if wire
            .iter()
            .any(|entry| ["kind", "node_id", "accumulator_kind"].contains(&entry.0))
        {
            Some(MotionErrorCode::PatchPathImmutable)
        } else if !known {
            Some(MotionErrorCode::NodeNotFound)
        } else if wire
            .iter()
            .any(|entry| !["from", "to", "easing", "duration_ms", "start_ms"].contains(&entry.0))
        {
            Some(MotionErrorCode::PatchPathUnknown)
        } else if wire.iter().any(|entry| {
            (entry.0 == "easing" && entry.1 != 2) || (entry.0 != "easing" && entry.1 != 0)
        }) {
            Some(MotionErrorCode::PatchInvalid)
        } else {
            None
        };
        let patch = MotionPatch::new(node_id(&mut random), target, ops, random.below(2) == 0);
        let outcome = apply_patch(graph, &patch, &mut AccumulatorStore::new());
        match (outcome, expected) {
            (Ok(_), None) => {}
            (Err(error), Some(code)) => ensure(
                error.code() == code,
                &format!("seed {seed}: ops {wire:?} gave {error}, wanted {code}"),
            )?,
            (Ok(_), Some(code)) => {
                return Err(format!("seed {seed}: ops {wire:?} applied, wanted {code}"));
            }
            (Err(error), None) => {
                return Err(format!("seed {seed}: ops {wire:?} refused: {error}"));
            }
        }
    }
    Ok(())
}

/// Bug caught: accumulator patches half implemented, where some path starts
/// to succeed or leaves state behind, or a tween path is wrongly accepted.
#[test]
fn sim_motion_patch_accumulator_targets_fail_without_side_effects() -> Result<(), String> {
    const ACCUMULATOR_PATHS: [&str; 5] = [
        "tension",
        "friction",
        "mass",
        "preserve_velocity",
        "scrub_source_id",
    ];
    for seed in 3_600..3_660_u64 {
        let mut random = SimRng::seed(seed);
        let id = node_id(&mut random);
        let component = match random.below(3) {
            0 => AccumulatorComponent::new_spring(spring_config(100.0, 10.0, 1.0)?),
            1 => AccumulatorComponent::new_drag(),
            _ => AccumulatorComponent::new_scrub(String::from("pointer.x")).map_err(describe)?,
        };
        let graph = single_node(id, NodeKind::Accumulator(component));
        let mut store = AccumulatorStore::new();
        let (path, expected) = if random.below(3) == 0 {
            let foreign = ["from", "to", "easing", "bogus"];
            (
                foreign[random.below_len(foreign.len())],
                MotionErrorCode::PatchPathUnknown,
            )
        } else {
            (
                ACCUMULATOR_PATHS[random.below_len(ACCUMULATOR_PATHS.len())],
                MotionErrorCode::PatchInvalid,
            )
        };
        let patch = MotionPatch::new(
            node_id(&mut random),
            id,
            vec![float_op(path, 1.0)],
            random.below(2) == 0,
        );
        match apply_patch(graph, &patch, &mut store) {
            Ok(_) => return Err(format!("seed {seed}: accumulator path {path} was applied")),
            Err(error) => ensure(
                error.code() == expected,
                &format!("seed {seed}: path {path} gave {error}, wanted {expected}"),
            )?,
        }
        ensure(
            store.get(&id).is_none(),
            &format!("seed {seed}: a failed accumulator patch created state for {path}"),
        )?;
    }
    Ok(())
}

/// Bug caught: a scrub binding outside [0, 1] (NaN included) because the
/// source returned garbage, which the clamp must contain.
#[test]
fn sim_motion_scrub_binding_is_clamped_for_hostile_source_values() -> Result<(), String> {
    let mut random = SimRng::seed(3_700);
    let mut values = vec![
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        -0.0,
        1.0e30,
        -1.0e30,
        1.000_000_1,
        0.5,
        1.0e-45,
    ];
    for _ in 0..200 {
        values.push(f32::from_bits(random.below_u32(u32::MAX)));
    }
    for value in values {
        let id = node_id(&mut random);
        let graph = scrub_graph(id, "src")?;
        let mut scrubs = HashScrubMap::new();
        scrubs.insert(String::from("src"), Box::new(ConstantScrub::new(value)));
        let got = scrub_frame(&graph, &scrubs, &mut AccumulatorStore::new(), id)?;
        let want = if value.is_nan() {
            0.0
        } else {
            value.clamp(0.0, 1.0)
        };
        ensure(
            (0.0..=1.0).contains(&got) && within(got, f64::from(want), 0.0),
            &format!("source {value:e} bound {got}, wanted {want}"),
        )?;
    }
    Ok(())
}

/// Bug caught: a scrub node caching its source, so a source wired late,
/// swapped, or removed is not followed on the very next frame.
#[test]
fn sim_motion_scrub_source_wired_late_swapped_and_removed() -> Result<(), String> {
    for seed in 3_800..3_860_u64 {
        let mut random = SimRng::seed(seed);
        let id = node_id(&mut random);
        let graph = scrub_graph(id, "scroll.y")?;
        let mut store = AccumulatorStore::new();
        let first = f32::from(random.below_u16(101)) / 100.0;
        let second = f32::from(random.below_u16(101)) / 100.0;
        let before = scrub_frame(&graph, &HashScrubMap::new(), &mut store, id)?;
        let mut wired = HashScrubMap::new();
        wired.insert(
            String::from("scroll.y"),
            Box::new(ConstantScrub::new(first)),
        );
        let after_wire = scrub_frame(&graph, &wired, &mut store, id)?;
        wired.insert(
            String::from("scroll.y"),
            Box::new(ConstantScrub::new(second)),
        );
        let after_swap = scrub_frame(&graph, &wired, &mut store, id)?;
        let after_loss = scrub_frame(&graph, &HashScrubMap::new(), &mut store, id)?;
        ensure(
            before.to_bits() == 0.0_f32.to_bits()
                && after_wire.to_bits() == first.to_bits()
                && after_swap.to_bits() == second.to_bits()
                && after_loss.to_bits() == 0.0_f32.to_bits(),
            &format!("seed {seed}: scrub did not follow wire {first}, swap {second}, loss"),
        )?;
        ensure(
            state_of(&store, id)?.last_scrub_value() == Some(0.0),
            &format!("seed {seed}: the lost source left a stale last value"),
        )?;
    }
    Ok(())
}

/// Bug caught: source ids normalised, trimmed or case-folded, so two
/// spellings of one name collide or one scrub reads another's value.
#[test]
fn sim_motion_scrub_ids_are_byte_exact_not_normalised() -> Result<(), String> {
    let long = "p".repeat(10_000);
    let ids = [
        "pointer.x",
        "pointer.x\u{200b}",
        "pointer.x\0",
        "POINTER.X",
        " pointer.x",
        "pointer.x\u{202e}",
        "pointer.x ",
        "poi\u{f1}ter.x",
        long.as_str(),
    ];
    let mut random = SimRng::seed(3_900);
    let mut scrubs = HashScrubMap::new();
    let mut builder = drishti::motion::MotionGraphBuilder::new();
    let mut expected = BTreeMap::new();
    for (slot, source) in ids.iter().enumerate() {
        let value = f32::from(u16::try_from(slot).map_err(describe)?) / 10.0;
        scrubs.insert(String::from(*source), Box::new(ConstantScrub::new(value)));
        let id = node_id(&mut random);
        let component = AccumulatorComponent::new_scrub(String::from(*source)).map_err(describe)?;
        builder = builder.add_node(
            id,
            String::from(".ids"),
            String::from("p"),
            NodeKind::Accumulator(component),
        );
        expected.insert(id, value);
    }
    let graph = builder.build();
    let output = frame(
        &graph,
        &Clock::deterministic(0.0),
        &mut AccumulatorStore::new(),
        &scrubs,
    );
    ensure(
        output.bindings().len() == ids.len(),
        "every spelling must bind its own node",
    )?;
    for write in output.bindings() {
        let want = expected.get(&write.node_id()).ok_or("unexpected binding")?;
        let got = write.value().as_scalar().ok_or("scalar expected")?;
        ensure(
            got.to_bits() == want.to_bits(),
            &format!("a spelling read {got} instead of its own {want}"),
        )?;
    }
    Ok(())
}

/// Bug caught: the scrub constructor refusing a non-empty id (whitespace,
/// NUL) or accepting the empty one, or dropping the id it was given.
#[test]
fn sim_motion_scrub_constructor_refuses_only_the_empty_id() -> Result<(), String> {
    let mut random = SimRng::seed(4_000);
    for _ in 0..200 {
        let length = random.below_len(4);
        let source = match random.below(3) {
            0 => random.text(length),
            1 => " ".repeat(length),
            _ => "\0".repeat(length),
        };
        match AccumulatorComponent::new_scrub(source.clone()) {
            Ok(component) => ensure(
                !source.is_empty()
                    && component.scrub_source_id() == Some(source.as_str())
                    && !component.preserve_velocity()
                    && component.spring_config().is_none(),
                &format!("accepted or altered scrub id {source:?}"),
            )?,
            Err(error) => ensure(
                source.is_empty() && error.code() == MotionErrorCode::PatchInvalid,
                &format!("refused scrub id {source:?} with {error}"),
            )?,
        }
    }
    Ok(())
}

/// What the monkey bound to a node, for the model.
enum Bound {
    /// Linear tween: from, to, start, duration.
    Tween([f32; 4]),
    /// Spring toward one.
    Spring,
    /// Scrub of source number `usize`.
    Scrub(usize),
    /// Drag at rest.
    Drag,
}

/// Names of the three monkey scrub sources.
const SOURCES: [&str; 3] = ["pointer.x", "scroll.y", "audio.t"];

/// Build a node of the kind the monkey drew.
fn monkey_node(id: Uuid, bound: &Bound) -> Result<MotionNode, String> {
    let kind = match *bound {
        Bound::Tween(spec) => NodeKind::Tween(linear_tween(spec[0], spec[1], spec[2], spec[3])?),
        Bound::Spring => NodeKind::Accumulator(AccumulatorComponent::new_spring(spring_config(
            120.0, 14.0, 1.0,
        )?)),
        Bound::Scrub(slot) => NodeKind::Accumulator(
            AccumulatorComponent::new_scrub(String::from(SOURCES[slot])).map_err(describe)?,
        ),
        Bound::Drag => NodeKind::Accumulator(AccumulatorComponent::new_drag()),
    };
    Ok(MotionNode::new(
        id,
        drishti::motion::BindingTarget::new(String::from(".monkey"), String::from("p")),
        kind,
    ))
}

/// Check one monkey frame against the model and mix it into the trace.
fn check_monkey_frame(
    output: &drishti::motion::Frame,
    model: &BTreeMap<Uuid, Bound>,
    sources: &[f32; 3],
    wall: f64,
    trace: &mut TraceHash,
) -> Result<(), String> {
    ensure(
        output.bindings().len() == model.len(),
        "the frame must bind exactly the model's nodes",
    )?;
    for (write, (id, bound)) in output.bindings().iter().zip(model.iter()) {
        let got = write.value().as_scalar().ok_or("scalar expected")?;
        let sound = match *bound {
            Bound::Tween(spec) => within(
                got,
                reference_linear(spec[0], spec[1], spec[2], spec[3], wall),
                3e-3,
            ),
            Bound::Spring => got.is_finite() && (-0.5..=2.5).contains(&got),
            Bound::Scrub(slot) => within(got, f64::from(sources[slot].clamp(0.0, 1.0)), 0.0),
            Bound::Drag => got.is_finite(),
        };
        ensure(
            write.node_id() == *id && sound,
            &format!("node {id:?} bound {got} against the model at {wall}"),
        )?;
        trace.mix_u64(u64::from(got.to_bits()));
    }
    Ok(())
}

/// Walk the whole motion API on one seed and hash what the frames wrote.
fn run_monkey(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut pool = Vec::new();
    for _ in 0..16 {
        pool.push(node_id(&mut random));
    }
    let mut graph = MotionGraph::default();
    let mut store = AccumulatorStore::new();
    let mut model: BTreeMap<Uuid, Bound> = BTreeMap::new();
    let mut scrubs = HashScrubMap::new();
    let mut sources = [0.0_f32; 3];
    for source in SOURCES {
        scrubs.insert(String::from(source), Box::new(ConstantScrub::new(0.0)));
    }
    let mut wall = 1_000.0_f64;
    for _ in 0..400 {
        let id = pool[random.below_len(pool.len())];
        match random.below(9) {
            0 | 1 => {
                let bound = match random.below(4) {
                    0 => Bound::Tween([
                        f32::from(random.below_u16(100)),
                        f32::from(random.below_u16(100)) + 100.0,
                        f32::from(random.below_u16(500)),
                        100.0 + f32::from(random.below_u16(900)),
                    ]),
                    1 => Bound::Spring,
                    2 => Bound::Scrub(random.below_len(SOURCES.len())),
                    _ => Bound::Drag,
                };
                graph = graph.add_node(monkey_node(id, &bound)?);
                model.insert(id, bound);
            }
            2 => {
                graph = graph.remove_node(&id);
                model.remove(&id);
                if random.below(2) == 0 {
                    store.remove(&id);
                }
            }
            3 => {
                if let Some(entry) = model.get_mut(&id)
                    && let Bound::Tween(ref mut spec) = *entry
                {
                    let to = f32::from(random.below_u16(300));
                    let patch = MotionPatch::new(
                        node_id(&mut random),
                        id,
                        vec![float_op("to", to)],
                        random.below(2) == 0,
                    );
                    graph = apply_patch(graph, &patch, &mut store).map_err(describe)?;
                    spec[1] = to;
                }
            }
            4 => store.rotate_and_reap(),
            5 => {
                let slot = random.below_len(SOURCES.len());
                let value = f32::from(random.below_u16(121)) / 100.0 - 0.1;
                scrubs.insert(
                    String::from(SOURCES[slot]),
                    Box::new(ConstantScrub::new(value)),
                );
                sources[slot] = value;
            }
            _ => {
                let step = random.below(10);
                wall += if step == 0 {
                    -500.0
                } else {
                    f64::from(random.below_u16(100)) + 1.0
                };
                let output = frame(&graph, &Clock::deterministic(wall), &mut store, &scrubs);
                check_monkey_frame(&output, &model, &sources, wall, &mut trace)?;
            }
        }
    }
    Ok(trace.finish())
}

/// Bug caught: a state nobody designed for, reachable by calling the motion
/// API in nonsense order (patch, reap, remove, rewind, re-add), where the
/// frame stops matching what the model says is bound.
#[test]
fn sim_motion_monkey_walks_every_call_and_replays() -> Result<(), String> {
    for seed in 4_100..4_130_u64 {
        let first = run_monkey(seed).map_err(|error| format!("seed {seed}: {error}"))?;
        let second = run_monkey(seed)?;
        ensure(
            first == second,
            &format!("seed {seed}: replay hash diverged"),
        )?;
    }
    ensure(
        run_monkey(4_100)? != run_monkey(4_101)?,
        "distinct monkey seeds must diverge",
    )?;
    Ok(())
}

/// One tenant of the neighbour sim: its own graph, store, clock and trace.
struct Tenant {
    /// Graph with a spring, a tween and a drag on shared node identities.
    graph: MotionGraph,
    /// This tenant's accumulator store.
    store: AccumulatorStore,
    /// This tenant's wall clock.
    wall: f64,
    /// This tenant's own randomness.
    random: SimRng,
    /// Hash of everything this tenant's frames wrote.
    trace: TraceHash,
}

impl Tenant {
    /// Build a tenant whose node identities come from `shared`, so tenants
    /// with the same `shared` value use the very same ids.
    fn new(seed: u64, shared: u64) -> Result<Self, String> {
        let mut ids = SimRng::seed(shared);
        let spring_id = node_id(&mut ids);
        let tween_id = node_id(&mut ids);
        let spring = AccumulatorComponent::new_spring(spring_config(120.0, 14.0, 1.0)?);
        let graph =
            single_node(spring_id, NodeKind::Accumulator(spring)).add_node(MotionNode::new(
                tween_id,
                drishti::motion::BindingTarget::new(String::from(".t"), String::from("p")),
                NodeKind::Tween(linear_tween(0.0, 100.0, 0.0, 500.0)?),
            ));
        Ok(Self {
            graph,
            store: AccumulatorStore::new(),
            wall: 0.0,
            random: SimRng::seed(seed),
            trace: TraceHash::new(),
        })
    }

    /// Advance this tenant's clock and hash one frame.
    fn advance(&mut self) {
        self.wall += f64::from(self.random.below_u16(40)) + 1.0;
        let output = eval_at(&self.graph, self.wall, &mut self.store);
        for write in output.bindings() {
            if let Some(value) = write.value().as_scalar() {
                self.trace.mix_u64(u64::from(value.to_bits()));
            }
        }
    }
}

/// Bug caught: state shared across tenants that use the same node ids, so
/// interleaving 100 to 1,000 tenants changes any one tenant's frames.
#[test]
fn sim_motion_neighbour_tenants_with_shared_ids_match_solo_runs() -> Result<(), String> {
    for (tenants, steps) in [(100_u64, 40_u32), (1_000, 12)] {
        let mut crowd = Vec::new();
        for seed in 0..tenants {
            crowd.push(Tenant::new(seed, seed.wrapping_rem(4))?);
        }
        let mut remaining = vec![steps; crowd.len()];
        let mut scheduler = SimRng::seed(tenants);
        let mut left = u64::from(steps).saturating_mul(tenants);
        while left > 0 {
            let pick = scheduler.below_len(crowd.len());
            if remaining[pick] > 0 {
                crowd[pick].advance();
                remaining[pick] = remaining[pick].saturating_sub(1);
                left = left.saturating_sub(1);
            }
        }
        for (seed, tenant) in (0..tenants).zip(crowd.iter()) {
            let mut solo = Tenant::new(seed, seed.wrapping_rem(4))?;
            for _ in 0..steps {
                solo.advance();
            }
            ensure(
                solo.trace.finish() == tenant.trace.finish(),
                &format!("{tenants} tenants: tenant {seed} was disturbed by its neighbours"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a replay trace that ignores part of the clock input, so two
/// timelines that differ by one frame time hash alike.
#[test]
fn sim_motion_replay_diverges_when_one_frame_time_shifts() -> Result<(), String> {
    for seed in 4_200..4_240_u64 {
        let mut random = SimRng::seed(seed);
        let id = node_id(&mut random);
        let component = AccumulatorComponent::new_spring(spring_config(120.0, 14.0, 1.0)?);
        let graph = single_node(id, NodeKind::Accumulator(component));
        let mut times = Vec::new();
        let mut wall = 100.0_f64;
        for _ in 0..60 {
            wall += f64::from(random.below_u16(20)) + 10.0;
            times.push(wall);
        }
        let hash_of = |script: &[f64]| -> Result<u64, String> {
            let mut store = AccumulatorStore::new();
            let mut trace = TraceHash::new();
            for time in script {
                trace.mix_bits(f64::from(scalar_of(
                    &eval_at(&graph, *time, &mut store),
                    id,
                )?));
            }
            Ok(trace.finish())
        };
        let mut shifted = times.clone();
        let victim = random.below_len(shifted.len().saturating_sub(1));
        shifted[victim] += 3.0;
        ensure(
            hash_of(&times)? == hash_of(&times)? && hash_of(&times)? != hash_of(&shifted)?,
            &format!("seed {seed}: shifting frame {victim} by 3 ms left the trace unchanged"),
        )?;
    }
    Ok(())
}
