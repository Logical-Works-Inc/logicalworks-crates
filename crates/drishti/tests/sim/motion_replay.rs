//! Seeded motion replay: random tween graphs evaluated twice from fresh
//! stores must hash identically, with clock rewinds and teleport jumps as
//! injected faults.
//!
//! Same seed replays the same trace hash over frame bindings.

use drishti::motion::{
    AccumulatorComponent, AccumulatorStore, Clock, Easing, HashScrubMap, MotionGraph,
    MotionGraphBuilder, NamedCurve, NodeKind, TweenComponent, frame, scrub::ConstantScrub,
    scrub::EmptyScrubMap, scrub::ScrubSourceMap,
};
use lgwks_std::id::Uuid;

use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// Seeded easing.
fn easing(random: &mut SimRng) -> Easing {
    match random.below(4) {
        0 => Easing::named(NamedCurve::Linear),
        1 => Easing::named(NamedCurve::CubicInOut),
        2 => Easing::named(NamedCurve::ExpoOut),
        _ => Easing::named(NamedCurve::BackOut),
    }
}

/// Build one seeded tween graph over `.sim/opacity` nodes.
fn tween_graph(seed: u64, random: &mut SimRng) -> Result<MotionGraph, String> {
    let mut builder = MotionGraphBuilder::new();
    let node_count = random.below_len(8).saturating_add(1);
    for _ in 0..node_count {
        let from = f32::from(random.below_u16(200)) - 100.0;
        let span = f32::from(random.below_u16(200)) + 1.0;
        let duration = f32::from(random.below_u16(2000)) + 1.0;
        let tween = TweenComponent::new(from, from + span, 0.0, duration, easing(&mut *random))
            .map_err(|error| format!("seeded tween rejected: {error}"))?;
        builder = builder.add_node(
            Uuid::from((u128::from(random.next_u64()) ^ (u128::from(seed) << 64)).to_le_bytes()),
            String::from(".sim"),
            String::from("opacity"),
            NodeKind::Tween(tween),
        );
    }
    Ok(builder.build())
}

/// Require two scalar bindings to agree bit-exactly, returning the scalar.
fn agree_scalars(first: Option<f32>, second: Option<f32>) -> Result<f32, String> {
    let shape = || String::from("replay must agree on shape");
    let first_v = first.ok_or_else(shape)?;
    let second_v = second.ok_or_else(shape)?;
    crate::fixtures::require_same_bits(first_v, second_v, "replay must agree bit-exactly")?;
    Ok(first_v)
}

/// Evaluate one frame on both fresh stores and require bit-exact agreement,
/// mixing every scalar into the trace.
fn compare_frame(
    graph: &MotionGraph,
    clock: &Clock,
    first_store: &mut AccumulatorStore,
    second_store: &mut AccumulatorStore,
    scrubs: &dyn ScrubSourceMap,
    trace: &mut TraceHash,
) -> Result<(), String> {
    let first = frame(graph, clock, first_store, scrubs);
    let second = frame(graph, clock, second_store, scrubs);
    ensure(first.bindings().len() == second.bindings().len(), || {
        String::from("replay must agree on binding count")
    })?;
    let mut first_sorted = first.bindings().to_vec();
    let mut second_sorted = second.bindings().to_vec();
    first_sorted.sort_by_key(|write| write.node_id());
    second_sorted.sort_by_key(|write| write.node_id());
    for (first_write, second_write) in first_sorted.iter().zip(second_sorted.iter()) {
        ensure(first_write.node_id() == second_write.node_id(), || {
            String::from("replay must agree on identity")
        })?;
        let first_v = agree_scalars(
            first_write.value().as_scalar(),
            second_write.value().as_scalar(),
        )?;
        trace.mix_u64(u64::from(first_v.to_bits()));
    }
    Ok(())
}

/// Start a frame replay: random graph, empty scrubs, seeded trace, fresh
/// stores, and a zeroed wall clock.
fn replay_prelude(
    seed: u64,
    random: &mut SimRng,
) -> Result<
    (
        MotionGraph,
        EmptyScrubMap,
        TraceHash,
        AccumulatorStore,
        AccumulatorStore,
        f64,
    ),
    String,
> {
    let graph = tween_graph(seed, random)?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    Ok((
        graph,
        EmptyScrubMap::new(),
        trace,
        AccumulatorStore::new(),
        AccumulatorStore::new(),
        0.0,
    ))
}

/// Advance the wall clock one frame and require fresh stores to agree.
fn tick_frame(
    graph: &MotionGraph,
    wall_ms: &mut f64,
    first_store: &mut AccumulatorStore,
    second_store: &mut AccumulatorStore,
    scrubs: &EmptyScrubMap,
    trace: &mut TraceHash,
) -> Result<(), String> {
    *wall_ms += 16.6;
    let clock = Clock::deterministic(*wall_ms);
    compare_frame(graph, &clock, first_store, second_store, scrubs, trace)
}

/// Run one replay from `seed` and hash the frames.
fn run_replay(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let (graph, scrubs, mut trace, mut first_store, mut second_store, mut wall_ms) =
        replay_prelude(seed, &mut random)?;
    for step in 0..120 {
        // Faults: rewind the clock, then jump past the teleport bound.
        if step == 40 {
            wall_ms -= 250.0;
        }
        if step == 80 {
            wall_ms += 400_000.0;
        }
        tick_frame(
            &graph,
            &mut wall_ms,
            &mut first_store,
            &mut second_store,
            &scrubs,
            &mut trace,
        )?;
    }
    Ok(trace.finish())
}

/// Same seed replays the same hash, faults included.
#[test]
fn sim_motion_replay_replays_identically() -> Result<(), String> {
    for seed in [9_u64, 13, 5150] {
        if run_replay(seed)? != run_replay(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_replay(9)? == run_replay(10)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}

/// Run one scrub-driven replay from `seed`: scrub accumulators sample seeded
/// constant sources while one source stays unwired; fresh stores always agree
/// and every binding matches its source value.
fn run_scrub(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    const SOURCES: &[&str] = &["pointer.x", "scroll.y", "clock.t"];
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut scrubs = HashScrubMap::new();
    let mut expected = [0.0_f32; 3];
    for (position, source) in SOURCES.iter().enumerate() {
        let slot = expected
            .get_mut(position)
            .ok_or("a source position is outside the expectations")?;
        if random.below(4) == 0 {
            // Fault: this source stays unwired and reads as zero.
            trace.mix_str("unwired");
            *slot = 0.0;
        } else {
            let value = f32::from(random.below_u16(101)) / 100.0;
            scrubs.insert(String::from(*source), Box::new(ConstantScrub::new(value)));
            *slot = value;
            trace.mix_bits(f64::from(value));
        }
    }
    let mut builder = MotionGraphBuilder::new();
    let mut nodes = Vec::new();
    let node_count = random.below_len(3).saturating_add(1);
    for _ in 0..node_count {
        let position = random.below_len(SOURCES.len());
        let source = SOURCES
            .get(position)
            .ok_or_else(|| String::from("source index out of range"))?;
        let node_id =
            Uuid::from((u128::from(random.next_u64()) ^ (u128::from(seed) << 64)).to_le_bytes());
        let component = AccumulatorComponent::new_scrub(String::from(*source))
            .map_err(|error| format!("seeded scrub rejected: {error}"))?;
        builder = builder.add_node(
            node_id,
            String::from(".sim"),
            String::from("scrub"),
            NodeKind::Accumulator(component),
        );
        nodes.push((node_id, position));
    }
    nodes.sort_by_key(|&(node_id, _)| node_id);
    let graph = builder.build();
    let mut first_store = AccumulatorStore::new();
    let mut second_store = AccumulatorStore::new();
    for _ in 0..120 {
        let clock = Clock::deterministic(1_000.0);
        let first = frame(&graph, &clock, &mut first_store, &scrubs);
        let second = frame(&graph, &clock, &mut second_store, &scrubs);
        ensure(
            first.bindings().len() == nodes.len() && second.bindings().len() == nodes.len(),
            || String::from("replay must bind every node"),
        )?;
        let mut first_sorted = first.bindings().to_vec();
        let mut second_sorted = second.bindings().to_vec();
        first_sorted.sort_by_key(|write| write.node_id());
        second_sorted.sort_by_key(|write| write.node_id());
        for ((&(node_id, position), first_write), second_write) in nodes
            .iter()
            .zip(first_sorted.iter())
            .zip(second_sorted.iter())
        {
            ensure(
                first_write.node_id() == node_id && second_write.node_id() == node_id,
                || String::from("replay must agree on identity"),
            )?;
            let first_v = agree_scalars(
                first_write.value().as_scalar(),
                second_write.value().as_scalar(),
            )?;
            let want = *expected
                .get(position)
                .ok_or("a source position is outside the expectations")?;
            ensure(first_v.to_bits() == want.to_bits(), || {
                format!("binding must match source {position}")
            })?;
            trace.mix_u64(u64::from(first_v.to_bits()));
        }
    }
    Ok(trace.finish())
}

/// Same seed replays the same scrub hash with identical source bindings.
#[test]
fn sim_motion_scrub_replays_identically() -> Result<(), String> {
    for seed in [21_u64, 59, 1_403] {
        if run_scrub(seed)? != run_scrub(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_scrub(21)? == run_scrub(22)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}

/// Run one rewind-heavy replay from `seed`: the clock only ever rewinds or
/// holds, teleport jumps never fire, and fresh stores still agree bit-exactly.
fn run_rewind(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let (graph, scrubs, mut trace, mut first_store, mut second_store, mut wall_ms) =
        replay_prelude(seed, &mut random)?;
    for step in 0..150 {
        // Faults: rewind the clock at three points; never teleport.
        if step == 30 || step == 60 || step == 90 {
            wall_ms -= 500.0;
        }
        tick_frame(
            &graph,
            &mut wall_ms,
            &mut first_store,
            &mut second_store,
            &scrubs,
            &mut trace,
        )?;
    }
    Ok(trace.finish())
}

/// Same seed replays the same rewind hash, rewinds included.
#[test]
fn sim_motion_clock_rewind_replays_identically() -> Result<(), String> {
    for seed in [33_u64, 77, 1_601] {
        if run_rewind(seed)? != run_rewind(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_rewind(33)? == run_rewind(34)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}
