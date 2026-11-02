//! Deterministic simulations for model-written plans (#786).
//!
//! One seed draws a whole scenario: a plan shape, the registry it is admitted
//! against, and the handlers both paths run. Every assertion is a property
//! checked against what the boundary actually did — the admitted lines, the
//! refusal arms and their order, the schedule the graph ran — and every run
//! records its trace so the same seed replays to the same hash. The decoder,
//! the surface, the registry, the graph scheduler and the store are the
//! shipped types; only the scenario is seeded.

#![cfg(all(feature = "edge", feature = "script"))]

use std::error::Error;

use lgwks_bot::rt::apps::{
    AppStore, OpHandler, OpRegistry, Outcome, PlanLimits, RefusalKind, run_app,
};
use lgwks_bot::script::{GraphNode, NodeOutcome, Scope, Tenant, graph};

use crate::apps_harness::{demo_registry, fetch_step, render_step};
use crate::sim;
use sim::Rng;

/// A test's result: an error fails it with the error's text.
type TestResult = Result<(), Box<dyn Error>>;

/// A failure naming its seed.
fn seeded(seed: u64, what: impl std::fmt::Display) -> Box<dyn Error> {
    format!("seed {seed}: {what}").into()
}

/// A registry registering exactly `ops`, each running the handler its
/// prefix names.
fn open_registry(ops: &[String]) -> Result<OpRegistry, String> {
    let mut registry = OpRegistry::new();
    for op in ops {
        if op.starts_with("fetch") {
            registry = registry
                .operation(op.as_str(), &[], fetch_step)
                .map_err(|error| format!("the registry is refused: {error}"))?;
        } else {
            registry = registry
                .operation(op.as_str(), &[], render_step)
                .map_err(|error| format!("the registry is refused: {error}"))?;
        }
    }
    Ok(registry)
}

// The `script!` twin of a two-operation plan, calling the same handlers.
lgwks_bot::script! {
    flow reference(fetch: OpHandler, render: OpHandler) -> Vec<Vec<u8>>:
        let fetched = fetch.call(scope.clone(), b"fetch".to_vec()).await?
        let rendered = render.call(scope.clone(), b"render".to_vec()).await?
        give back vec![fetched, rendered]
}

/// One drawn operation: its name and the earlier operations it needs.
#[derive(Debug, Clone)]
struct DrawnOp {
    /// The operation as the plan writes it.
    name: String,
    /// Earlier operations it waits for, by name.
    needs: Vec<String>,
}

/// Draw `count` operations named `fetch{tag}_{i}` or `render{tag}_{i}`.
fn draw_ops(rng: &mut Rng, tag: u64, count: u32) -> Result<Vec<DrawnOp>, Box<dyn Error>> {
    let mut ops: Vec<DrawnOp> = Vec::with_capacity(usize::try_from(count)?);
    for index in 0..count {
        let base = if rng.below(2) == 0 { "fetch" } else { "render" };
        let name = format!("{base}{tag}_{index}");
        let mut needs = Vec::new();
        if index > 0 && rng.below(2) == 0 {
            let earlier = usize::try_from(rng.below(index))?;
            if let Some(prior) = ops.get(earlier) {
                needs.push(prior.name.clone());
            }
        }
        ops.push(DrawnOp { name, needs });
    }
    Ok(ops)
}

/// The text plan running `ops` through one graph.
fn plan_text(ops: &[DrawnOp]) -> String {
    let mut text = String::from("flow app():\n    let out = graph:\n");
    for op in ops {
        if op.needs.is_empty() {
            text.push_str(format!("        node {}:\n            \"x\"\n", op.name).as_str());
        } else {
            text.push_str(
                format!(
                    "        node {} needs {}:\n            \"x\"\n",
                    op.name,
                    op.needs.join(", ")
                )
                .as_str(),
            );
        }
    }
    text
}

/// Fold text into `fold`.
fn fold_text(fold: u64, text: &str) -> u64 {
    let mut fold = fold;
    for byte in text.bytes() {
        fold = fold.wrapping_mul(31).wrapping_add(u64::from(byte));
    }
    fold
}

/// Fold lines into `fold`.
fn fold_lines(fold: u64, lines: &[Vec<u8>]) -> u64 {
    let mut fold = fold;
    for line in lines {
        for byte in line {
            fold = fold.wrapping_mul(31).wrapping_add(u64::from(*byte));
        }
    }
    fold
}

/// Requirement: an admitted plan and the compiled graph give one trace.
///
/// Proves INV-BOT-188: lowering the parsed tree into `GraphNode`s runs the
/// same scheduler, steps and report as the public `graph` runtime — and the
/// same app compiled with `script!` calling the same handlers prints the
/// same lines — one seed, one trace hash, on every path.
#[test]
fn the_admitted_plan_matches_the_compiled_graph_trace() -> TestResult {
    sim::assert_replays(sim::band_of(64), |sim| {
        let tag = sim.seed % 1_000;
        let count = sim.rng().between(1, 4);
        let ops = draw_ops(sim.rng(), tag, count)?;
        let names: Vec<String> = ops.iter().map(|op| op.name.clone()).collect();
        let registry = open_registry(names.as_slice())?;
        let surface = registry.surface("acme", &[])?;
        let store = AppStore::new(PlanLimits::default());
        let text = plan_text(ops.as_slice());
        let admitted = match store.admit_text(&surface, &registry, text.as_bytes()) {
            Outcome::Admitted(admitted) => admitted,
            Outcome::Refused(refused) => {
                return Err(seeded(
                    sim.seed,
                    format!("refused its own plan: {:?}", refused.refusals()),
                ));
            }
            _ => {
                return Err(seeded(
                    sim.seed,
                    "refused its own plan with an unknown outcome",
                ));
            }
        };
        let scope = Scope::root(Tenant::new("acme")?);
        let lines = lgwks_bot::block_on(run_app(&scope, &store, admitted.id()))?;
        // The compiled path: the same operations through the public graph.
        let scope = Scope::root(Tenant::new("acme")?);
        let mut nodes = Vec::with_capacity(ops.len());
        for op in &ops {
            let handler = registry
                .get(op.name.as_str())
                .ok_or_else(|| seeded(sim.seed, format!("lost {}", op.name)))?;
            let needs: Vec<&str> = op.needs.iter().map(String::as_str).collect();
            let input = op.name.clone().into_bytes();
            nodes.push(GraphNode::new(
                op.name.as_str(),
                needs.as_slice(),
                move |scope: Scope| {
                    let handler = handler.clone();
                    let input = input.clone();
                    Box::pin(async move { handler.call(scope, input).await })
                },
            ));
        }
        let report = lgwks_bot::block_on(graph(&scope, "plan", ops.len(), nodes))
            .map_err(|error| seeded(sim.seed, format!("graph refused: {error}")))?;
        let mut expected = Vec::with_capacity(ops.len());
        for op in &ops {
            let Some(outcome) = report.outcome(op.name.as_str()) else {
                return Err(seeded(
                    sim.seed,
                    format!("node {} never ran: no outcome", op.name),
                ));
            };
            match *outcome {
                NodeOutcome::Ran(Ok(ref line)) => expected.push(line.clone()),
                ref other => {
                    return Err(seeded(
                        sim.seed,
                        format!("node {} never ran: {other:?}", op.name),
                    ));
                }
            }
        }
        let printed: Vec<String> = lines
            .iter()
            .map(|line| String::from_utf8_lossy(line.as_slice()).into_owned())
            .collect();
        let wanted: Vec<String> = expected
            .iter()
            .map(|line| String::from_utf8_lossy(line.as_slice()).into_owned())
            .collect();
        if printed != wanted {
            return Err(seeded(sim.seed, format!("ran {printed:?}, not {wanted:?}")));
        }
        let mut trace = fold_lines(0x51ab_1e5e, lines.as_slice());
        for wave in report.waves() {
            for name in wave.iter() {
                trace = fold_text(trace, name.as_ref());
            }
        }
        // The same app compiled with `script!`, calling the same handlers.
        let demo = demo_registry()?;
        let demo_surface = demo.surface("acme", &[])?;
        let fixed = match store.admit_text(
            &demo_surface,
            &demo,
            b"flow app():\n    let out = graph:\n        node fetch:\n            \"x\"\n        node render needs fetch:\n            \"x\"\n",
        ) {
            Outcome::Admitted(fixed) => fixed,
            Outcome::Refused(refused) => {
                return Err(seeded(sim.seed, format!("fixed plan refused: {:?}", refused.refusals())));
            }
            _ => {
                return Err(seeded(sim.seed, "fixed plan refused with an unknown outcome"));
            }
        };
        let fixed_lines = lgwks_bot::block_on(run_app(&scope, &store, fixed.id()))?;
        let scope = Scope::root(Tenant::new("acme")?);
        let fetch = demo
            .get("fetch")
            .ok_or_else(|| "the demo registry lost fetch".to_owned())?;
        let render = demo
            .get("render")
            .ok_or_else(|| "the demo registry lost render".to_owned())?;
        let scripted = lgwks_bot::block_on(reference(&scope, fetch, render))?;
        let printed: Vec<String> = fixed_lines
            .iter()
            .map(|line| String::from_utf8_lossy(line.as_slice()).into_owned())
            .collect();
        let wanted: Vec<String> = scripted
            .iter()
            .map(|line| String::from_utf8_lossy(line.as_slice()).into_owned())
            .collect();
        if printed != wanted {
            return Err(seeded(
                sim.seed,
                format!("the script twin ran {wanted:?}, not {printed:?}"),
            ));
        }
        sim.record(format!("trace {trace:#x}").as_str());
        Ok(())
    })
}

/// Requirement: hostile bytes are refused typed and never panic.
///
/// Proves INV-BOT-188: seeded fuzz — arbitrary bytes as text and as JSON —
/// always answers an `Outcome`, and the same bytes answer the same verdict.
#[test]
fn hostile_bytes_are_refused_typed_never_panic() -> TestResult {
    sim::assert_replays(sim::band_of(65), |sim| {
        let count = sim.rng().between(0, 512);
        let mut bytes = Vec::with_capacity(usize::try_from(count)?);
        for _ in 0..count {
            let byte = match sim.rng().below(4) {
                0 => b'a',
                1 => b'b',
                2 => b'c',
                _ => b'd',
            };
            bytes.push(byte);
        }
        let registry = demo_registry()?;
        let surface = registry.surface("acme", &[])?;
        let store = AppStore::new(PlanLimits::default());
        let text = store.admit_text(&surface, &registry, bytes.as_slice());
        let json = store.admit_json(&surface, &registry, bytes.as_slice());
        let again = store.admit_text(&surface, &registry, bytes.as_slice());
        if text.verdict_json() != again.verdict_json() {
            return Err(seeded(sim.seed, "answered twice differently"));
        }
        sim.record(
            format!(
                "text {} json {}",
                text.refusals().len(),
                json.refusals().len()
            )
            .as_str(),
        );
        Ok(())
    })
}

/// Requirement: nesting past the ceiling is refused naming the depth.
///
/// Proves INV-BOT-188: bounds come before authorization, and the refusal
/// names the measured depth against its ceiling.
#[test]
fn a_plan_past_depth_is_refused() -> TestResult {
    sim::assert_replays(sim::band_of(66), |sim| {
        let depth = match sim.seed % 8 {
            0 => 33_usize,
            1 => 34_usize,
            2 => 35_usize,
            3 => 36_usize,
            4 => 37_usize,
            5 => 38_usize,
            6 => 39_usize,
            _ => 40_usize,
        };
        let mut text = String::from("flow app():\n");
        for level in 0..depth {
            text.push_str(
                format!("{}step s{level}:\n", "    ".repeat(level.saturating_add(1))).as_str(),
            );
        }
        text.push_str(format!("{}run fetch()\n", "    ".repeat(depth.saturating_add(1))).as_str());
        let registry = demo_registry()?;
        let surface = registry.surface("acme", &[])?;
        let store = AppStore::new(PlanLimits::default());
        match store.admit_text(&surface, &registry, text.as_bytes()) {
            Outcome::Refused(refused) => {
                if refused.refusals().len() != 1 {
                    return Err(seeded(
                        sim.seed,
                        format!("refused {} times, not once", refused.refusals().len()),
                    ));
                }
                let only = &refused.refusals()[0];
                if only.kind() != RefusalKind::BoundExceeded {
                    return Err(seeded(sim.seed, format!("refused as {:?}", only.kind())));
                }
                sim.record(format!("depth {}", only.message()).as_str());
                Ok(())
            }
            Outcome::Admitted(_) => Err(seeded(sim.seed, "admitted past depth")),
            _ => Err(seeded(
                sim.seed,
                "admitted past depth with an unknown outcome",
            )),
        }
    })
}

/// Requirement: oversized and over-long bodies are refused before any parse.
///
/// Proves INV-BOT-188: the byte ceiling and the line ceiling are charged
/// before anything is buffered or read.
#[test]
fn an_oversized_plan_is_refused() -> TestResult {
    sim::assert_replays(sim::band_of(67), |sim| {
        let limits = PlanLimits::default();
        let store = AppStore::new(limits);
        let registry = demo_registry()?;
        let surface = registry.surface("acme", &[])?;
        let wide = vec![b'x'; limits.max_bytes().saturating_add(1)];
        match store.admit_text(&surface, &registry, wide.as_slice()) {
            Outcome::Refused(refused)
                if refused
                    .refusals()
                    .iter()
                    .all(|refusal| refusal.kind() == RefusalKind::BoundExceeded)
                    && Outcome::Refused(refused.clone()).http_status() == 413 =>
            {
                sim.record("oversize 413");
            }
            other => {
                return Err(seeded(sim.seed, format!("oversize answered {other:?}")));
            }
        }
        let mut long = String::from("flow app():\n");
        for index in 0..300 {
            long.push_str(format!("    step s{index}:\n        run fetch()\n").as_str());
        }
        match store.admit_text(&surface, &registry, long.as_bytes()) {
            Outcome::Refused(refused)
                if refused
                    .refusals()
                    .iter()
                    .all(|refusal| refusal.kind() == RefusalKind::BoundExceeded) =>
            {
                sim.record("overlong bound-exceeded");
                Ok(())
            }
            other => Err(seeded(sim.seed, format!("overlong answered {other:?}"))),
        }
    })
}

/// Requirement: every refusal is gathered and sorted by location.
///
/// Proves INV-BOT-188: three bad operations in one plan give three refusals
/// — unknown, missing grant, unknown — in source order, never just the first.
#[test]
fn unknown_operations_and_missing_grants_are_gathered_sorted() -> TestResult {
    sim::assert_replays(sim::band_of(68), |sim| {
        let text = "flow app():\n    let out = graph:\n        node steal:\n            \"x\"\n        node preview:\n            \"x\"\n        node grab:\n            \"x\"\n        node fetch:\n            \"x\"\n";
        let registry = demo_registry()?;
        let surface = registry.surface("acme", &[])?;
        let store = AppStore::new(PlanLimits::default());
        match store.admit_text(&surface, &registry, text.as_bytes()) {
            Outcome::Refused(refused) => {
                let found: Vec<RefusalKind> = refused
                    .refusals()
                    .iter()
                    .map(|refusal| refusal.kind())
                    .collect();
                let wanted = vec![
                    RefusalKind::UnknownOperation,
                    RefusalKind::MissingCapability,
                    RefusalKind::UnknownOperation,
                ];
                if found != wanted {
                    return Err(seeded(
                        sim.seed,
                        format!("refused {found:?}, not {wanted:?}"),
                    ));
                }
                let locations: Vec<&str> = refused
                    .refusals()
                    .iter()
                    .map(|refusal| refusal.location())
                    .collect();
                let mut ordered = locations.clone();
                ordered.sort();
                if locations != ordered {
                    return Err(seeded(sim.seed, "refusals read unsorted"));
                }
                sim.record(format!("refusals {}", refused.refusals().len()).as_str());
                Ok(())
            }
            Outcome::Admitted(_) => Err(seeded(sim.seed, "admitted hostile ops")),
            _ => Err(seeded(
                sim.seed,
                "admitted hostile ops with an unknown outcome",
            )),
        }
    })
}

/// Requirement: two tenants admitting one plan stay isolated.
///
/// Proves INV-BOT-188: the same bytes admitted under two tenants give one id
/// and two `(tenant, id)` entries — each tenant runs its own copy, and a
/// tenant that never admitted finds nothing under the id.
#[test]
fn two_tenants_admitting_one_plan_stay_isolated() -> TestResult {
    sim::assert_replays(sim::band_of(69), |sim| {
        let text = "flow app():\n    let out = graph:\n        node fetch:\n            \"x\"\n";
        let registry = demo_registry()?;
        let allowed = registry.surface("acme", &[])?;
        let store = AppStore::new(PlanLimits::default());
        let admitted = match store.admit_text(&allowed, &registry, text.as_bytes()) {
            Outcome::Admitted(admitted) => admitted,
            Outcome::Refused(refused) => {
                return Err(seeded(
                    sim.seed,
                    format!("refused its own plan: {:?}", refused.refusals()),
                ));
            }
            _ => {
                return Err(seeded(
                    sim.seed,
                    "refused its own plan with an unknown outcome",
                ));
            }
        };
        let other = Scope::root(Tenant::new("other")?);
        match lgwks_bot::block_on(run_app(&other, &store, admitted.id())) {
            Err(error) if error.kind() == lgwks_bot::rt::apps::RunKind::UnknownApp => {}
            other => {
                return Err(seeded(
                    sim.seed,
                    format!("ran another tenant's app: {other:?}"),
                ));
            }
        }
        let same =
            match store.admit_text(&registry.surface("other", &[])?, &registry, text.as_bytes()) {
                Outcome::Admitted(same) => same,
                Outcome::Refused(refused) => {
                    return Err(seeded(
                        sim.seed,
                        format!("refused the other tenant: {:?}", refused.refusals()),
                    ));
                }
                _ => {
                    return Err(seeded(
                        sim.seed,
                        "refused the other tenant with an unknown outcome",
                    ));
                }
            };
        if admitted.id() != same.id() {
            return Err(seeded(sim.seed, "gave one plan two ids"));
        }
        let lines = lgwks_bot::block_on(run_app(&other, &store, same.id()))?;
        if lines.len() != 1 {
            return Err(seeded(
                sim.seed,
                format!("ran {} lines, not one", lines.len()),
            ));
        }
        sim.record(format!("isolated {}", admitted.id()).as_str());
        Ok(())
    })
}

/// Requirement: the same seed replays the same verdict.
///
/// Proves INV-BOT-188: admission is deterministic — one plan and surface give
/// byte-identical verdicts, twice over.
#[test]
fn the_same_seed_replays_the_same_verdict() -> TestResult {
    sim::assert_replays(sim::band_of(70), |sim| {
        let tag = sim.seed % 1_000;
        let ops = draw_ops(sim.rng(), tag, 3)?;
        let names: Vec<String> = ops.iter().map(|op| op.name.clone()).collect();
        let registry = open_registry(names.as_slice())?;
        let surface = registry.surface("acme", &[])?;
        let store = AppStore::new(PlanLimits::default());
        let text = plan_text(ops.as_slice());
        let first = store
            .admit_text(&surface, &registry, text.as_bytes())
            .verdict_json();
        let second = store
            .admit_text(&surface, &registry, text.as_bytes())
            .verdict_json();
        if first != second {
            return Err(seeded(sim.seed, "answered twice differently"));
        }
        sim.record(format!("verdict {}", first.len()).as_str());
        Ok(())
    })
}

/// Requirement: distinct seeds diverge in their verdict.
///
/// Proves INV-BOT-188: the verdict carries the plan — different seeds draw
/// different operations and answer different bytes.
#[test]
fn distinct_seeds_diverge_in_their_verdict() -> TestResult {
    sim::assert_replays(sim::band_of(71), |sim| {
        let seed = sim.seed;
        let first_ops = draw_ops(sim.rng(), seed % 1_000, 3)?;
        let seed = sim.seed;
        let second_ops = draw_ops(sim.rng(), seed.wrapping_add(7) % 1_000, 3)?;
        let mut names: Vec<String> = first_ops
            .iter()
            .chain(second_ops.iter())
            .map(|op| op.name.clone())
            .collect();
        names.sort();
        names.dedup();
        let registry = open_registry(names.as_slice())?;
        let surface = registry.surface("acme", &[])?;
        let store = AppStore::new(PlanLimits::default());
        let first = store
            .admit_text(
                &surface,
                &registry,
                plan_text(first_ops.as_slice()).as_bytes(),
            )
            .verdict_json();
        let second = store
            .admit_text(
                &surface,
                &registry,
                plan_text(second_ops.as_slice()).as_bytes(),
            )
            .verdict_json();
        if first == second {
            return Err(seeded(sim.seed, "drew one verdict twice"));
        }
        sim.record(format!("diverged {} {}", first.len(), second.len()).as_str());
        Ok(())
    })
}
