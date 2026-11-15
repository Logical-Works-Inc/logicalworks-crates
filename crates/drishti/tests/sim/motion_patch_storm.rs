//! Seeded patch storm: valid and invalid patch sequences must stay atomic —
//! versions bump only on success and the graph never half-applies.
//!
//! Same seed replays the same trace hash over versions and error codes.

use drishti::motion::patch::{MotionPatch, PatchOp, PatchValue, apply_patch};
use drishti::motion::{AccumulatorStore, Easing, MotionGraph, NamedCurve};
use lgwks_std::id::Uuid;

use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// Build one storm tween node and return the graph with its id.
fn tween_graph(random: &mut SimRng) -> Result<(MotionGraph, Uuid), String> {
    let node_id = Uuid::from((u128::from(random.next_u64()) ^ 0x9e37_79b9_7f4a_7c15).to_le_bytes());
    crate::fixtures::tween_graph(node_id, ".storm", 600.0)
}

/// Seeded patch op: valid tween paths, immutable paths, and unknown paths.
fn patch_op(random: &mut SimRng) -> PatchOp {
    const PATHS: &[&str] = &[
        "from",
        "to",
        "duration_ms",
        "start_ms",
        "easing",
        "kind",
        "node_id",
        "bogus",
    ];
    let path = if let Some(path) = random.pick_str(PATHS) {
        String::from(path)
    } else {
        String::from("bogus")
    };
    let value = if path == "easing" {
        PatchValue::Easing(Easing::named(NamedCurve::CubicOut))
    } else if random.below(5) == 0 {
        PatchValue::Bool(true)
    } else {
        PatchValue::Float(f32::from(random.below_u16(2000)) - 500.0)
    };
    PatchOp::new(path, value)
}

/// Start a refusal campaign: fresh graph, accumulators, trace, and baseline.
fn refusal_prelude(
    seed: u64,
    random: &mut SimRng,
) -> Result<(MotionGraph, Uuid, AccumulatorStore, TraceHash, u64), String> {
    let (graph, node_id) = tween_graph(random)?;
    let accumulators = AccumulatorStore::new();
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let before = graph.version();
    Ok((graph, node_id, accumulators, trace, before))
}

/// Record one refusal: hash the error and count it.
fn record_refusal(trace: &mut TraceHash, refused: usize, error: &impl core::fmt::Debug) -> usize {
    trace.mix_str(&format!("{error:?}"));
    refused.saturating_add(1)
}

/// Require a full-refusal campaign: every patch failed and the version held.
fn require_full_refusal(
    refused: usize,
    graph: &MotionGraph,
    before: u64,
    what: &str,
    trace: &mut TraceHash,
) -> Result<(), String> {
    ensure(refused == 200, || String::from(what))?;
    ensure(graph.version() == before, || {
        String::from("refused patches must not bump the version")
    })?;
    trace.mix_u64(graph.version());
    Ok(())
}

/// Run one storm from `seed` and hash versions plus outcomes.
fn run_storm(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let (mut graph, node_id) = tween_graph(&mut random)?;
    let mut accumulators = AccumulatorStore::new();
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    trace.mix_u64(graph.version());
    for _ in 0..200 {
        let op_count = random.below_len(3).saturating_add(1);
        let mut ops = Vec::new();
        for _ in 0..op_count {
            ops.push(patch_op(&mut random));
        }
        // Sometimes retarget a forged node id.
        let target = if random.below(10) == 0 {
            Uuid::from(u128::from(random.next_u64()).to_le_bytes())
        } else {
            node_id
        };
        let patch = MotionPatch::new(
            Uuid::from(u128::from(random.next_u64()).to_le_bytes()),
            target,
            ops,
            random.below(2) == 1,
        );
        let before = graph.version();
        match apply_patch(graph.clone(), &patch, &mut accumulators) {
            Ok(updated) => {
                ensure(updated.version() == before.saturating_add(1), || {
                    String::from("success must bump exactly once")
                })?;
                trace.mix_u64(updated.version());
                graph = updated;
            }
            Err(error) => {
                trace.mix_str(&format!("{error:?}"));
            }
        }
    }
    Ok(trace.finish())
}

/// Same seed replays the same hash with identical version tracks.
#[test]
fn sim_patch_storm_replays_identically() -> Result<(), String> {
    for seed in [61_u64, 67, 808] {
        if run_storm(seed)? != run_storm(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_storm(61)? == run_storm(62)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}

/// Run one immutable-path campaign from `seed`: single-op patches to the
/// immutable `kind` and `node_id` paths always fail and the graph version
/// never moves.
fn run_immutable(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let (graph, node_id, mut accumulators, mut trace, before) = refusal_prelude(seed, &mut random)?;
    let mut refused = 0_usize;
    for _ in 0..200 {
        let path = if random.below(2) == 0 {
            "kind"
        } else {
            "node_id"
        };
        let patch = MotionPatch::new(
            Uuid::from(u128::from(random.next_u64()).to_le_bytes()),
            node_id,
            vec![PatchOp::new(
                String::from(path),
                PatchValue::Float(f32::from(random.below_u16(2000)) - 500.0),
            )],
            random.below(2) == 1,
        );
        let error = apply_patch(graph.clone(), &patch, &mut accumulators)
            .err()
            .ok_or_else(|| format!("immutable path {path} must fail"))?;
        refused = record_refusal(&mut trace, refused, &error);
    }
    require_full_refusal(
        refused,
        &graph,
        before,
        "every immutable patch must fail",
        &mut trace,
    )?;
    Ok(trace.finish())
}

/// Same seed replays the same immutable-path hash with identical refusals.
#[test]
fn sim_patch_immutable_path_replays_identically() -> Result<(), String> {
    for seed in [71_u64, 73, 909] {
        if run_immutable(seed)? != run_immutable(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_immutable(71)? == run_immutable(72)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}

/// Run one forged-target campaign from `seed`: valid single-op patches aimed
/// at unknown node ids always fail and the graph version never moves.
fn run_forged(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let (graph, _, mut accumulators, mut trace, before) = refusal_prelude(seed, &mut random)?;
    let mut refused = 0_usize;
    for _ in 0..200 {
        let patch = MotionPatch::new(
            Uuid::from(u128::from(random.next_u64()).to_le_bytes()),
            Uuid::from(u128::from(random.next_u64()).to_le_bytes()),
            vec![PatchOp::new(
                String::from("from"),
                PatchValue::Float(f32::from(random.below_u16(2000)) - 500.0),
            )],
            random.below(2) == 1,
        );
        let error = apply_patch(graph.clone(), &patch, &mut accumulators)
            .err()
            .ok_or_else(|| String::from("forged target must fail"))?;
        refused = record_refusal(&mut trace, refused, &error);
    }
    require_full_refusal(
        refused,
        &graph,
        before,
        "every forged patch must fail",
        &mut trace,
    )?;
    Ok(trace.finish())
}

/// Same seed replays the same forged-target hash with identical refusals.
#[test]
fn sim_patch_forged_target_replays_identically() -> Result<(), String> {
    for seed in [79_u64, 83, 1_003] {
        if run_forged(seed)? != run_forged(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_forged(79)? == run_forged(80)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}
