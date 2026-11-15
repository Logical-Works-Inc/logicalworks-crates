//! The fools that walk the layers: wrong-order reads `source()` and the route
//! table in the wrong order, neighbour shares one process between two tenants,
//! and the monkey applies random layer operations to one error and checks a
//! hand-kept model after each.

use std::io;

use lgwks_bot::BotError;
use lgwks_bot::error::{DispatchCertainty, RetryClass};
use lgwks_bot::script::FlowError;
use lgwks_bot::task::StoreError;

use super::{DRAWS, Fields, Hash, Kind, Route, Spec, TestResult, arms, hostile, layer_of, path};
use super::{replays, walk};
use crate::sim::{Rng, Trace};

/// A flow nested `depth` retries deep over a bottom drawn from `rng`; returns
/// the flow and the links its walk must end in once the flow layers are passed.
fn nested(rng: &mut Rng, depth: u32) -> (FlowError, Vec<&'static str>) {
    let text = hostile(rng);
    let (mut flow, tail) = match rng.below(4) {
        0 => (
            FlowError::from(Kind::draw(rng).build(&Fields::draw(rng))),
            vec!["world"],
        ),
        1 => (
            FlowError::from(StoreError::Storage {
                cause: io::Error::other(text),
            }),
            vec!["store", "io"],
        ),
        2 => (
            FlowError::from(StoreError::Corrupt { at: rng.next_u64() }),
            vec!["store"],
        ),
        _ => (FlowError::failed(text), Vec::new()),
    };
    for level in 0..depth {
        let at = path("retry");
        let attempts = level.saturating_add(1);
        let last = Box::new(flow);
        flow = if rng.chance(500) {
            FlowError::Exhausted { at, attempts, last }
        } else {
            FlowError::Throttled { at, attempts, last }
        };
    }
    (flow, tail)
}

/// `source()` walks the layers in the order they were wrapped, once each.
pub(super) fn sim_wrong_order_source_walk(seed: u64) -> Hash {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    for draw in 0..DRAWS {
        let depth = rng.below(40);
        let (flow, tail) = nested(&mut rng, depth);
        let mut expected = vec!["flow"; usize::try_from(depth)?];
        expected.extend(tail);
        let seen = walk(&BotError::from(flow));
        assert_eq!(seen, expected, "seed {seed:#x} draw {draw}: depth {depth}");
        // A kernel failure has no deeper cause, so a transparent wrapper has no hop.
        let spec = Spec::draw(&mut rng);
        let want: &[&str] = if spec.route == Route::Flow {
            &["world"]
        } else {
            &[]
        };
        assert_eq!(
            walk(&spec.raise()),
            want,
            "seed {seed:#x} draw {draw}: {:?}",
            spec.route
        );
        trace.record(&seen.join(">"));
    }
    Ok(trace.hash())
}

#[test]
/// `source()` visits each layer once, in wrapping order, however deep the retries nest.
fn sim_error_layers_wrong_order_source_walk_visits_each_layer_once_in_order() -> TestResult {
    replays(sim_wrong_order_source_walk)
}

/// The four routes in a shuffled order each land in their own arm.
pub(super) fn sim_wrong_order_every_route(seed: u64) -> Hash {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    for draw in 0..DRAWS {
        let mut order = Route::ALL;
        for index in (1..order.len()).rev() {
            let bound = u32::try_from(index)?.saturating_add(1);
            order.swap(index, usize::try_from(rng.below(bound))?);
        }
        let fields = Fields::draw(&mut rng);
        let kind = Kind::draw(&mut rng);
        let mut landed: Vec<&str> = Vec::new();
        for route in order {
            let error = route.wrap(kind.build(&fields));
            let at = format!("seed {seed:#x} draw {draw}: {route:?}");
            assert_eq!(arms(&error), 1, "{at}");
            assert_eq!(layer_of(&error), route.tag(), "{at}");
            landed.push(layer_of(&error));
        }
        landed.sort_unstable();
        landed.dedup();
        assert_eq!(
            landed.len(),
            4,
            "seed {seed:#x} draw {draw}: two routes shared an arm"
        );
        trace.record(&format!("{order:?}"));
    }
    Ok(trace.hash())
}

#[test]
/// Whatever order the routes run in, each ends in its own arm and no arm goes unused.
fn sim_error_layers_wrong_order_every_route_lands_in_its_own_arm() -> TestResult {
    replays(sim_wrong_order_every_route)
}

/// The two names a store error carries: the other party, then the raiser.
fn names_of(error: &BotError) -> Option<(&str, &str)> {
    match *error {
        BotError::Script(FlowError::Store { ref source, .. }) => match **source {
            StoreError::ForeignTenant {
                ref owner,
                ref asked,
            } => Some((owner, asked)),
            StoreError::UnknownRun {
                ref run,
                ref tenant,
            } => Some((run, tenant)),
            _ => None,
        },
        _ => None,
    }
}

/// One failure a tenant raised, and which tenant that was.
struct Event {
    /// Index into the two tenant names.
    raiser: usize,
    /// The failure the caller received.
    error: BotError,
}

/// One quiet tenant among a neighbour a thousand times louder: each error
/// carries the name of the tenant that raised it, exactly, and never the other's.
pub(super) fn sim_neighbour(seed: u64) -> Hash {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    let base = hostile(&mut rng);
    let names = [format!("{base}-a"), format!("{base}-b")];
    let noisy = 1000_u32;
    let quiet_at = rng.below(noisy);
    let mut events = Vec::new();
    for index in 0..=noisy {
        let raiser = usize::from(index != quiet_at);
        let mine = names.get(raiser).ok_or("a tenant name")?.clone();
        let theirs = names
            .get(usize::from(raiser == 0))
            .ok_or("a tenant name")?
            .clone();
        let store = if rng.chance(500) {
            StoreError::ForeignTenant {
                owner: theirs,
                asked: mine,
            }
        } else {
            StoreError::UnknownRun {
                run: format!("run-{index}"),
                tenant: mine,
            }
        };
        events.push(Event {
            raiser,
            error: BotError::from(FlowError::from(store)),
        });
    }
    let mut quiet = 0_u32;
    for event in &events {
        let carried = names_of(&event.error).ok_or("a store error in the script arm")?;
        let mine = names.get(event.raiser).ok_or("a tenant name")?;
        let theirs = names
            .get(usize::from(event.raiser == 0))
            .ok_or("a tenant name")?;
        // The raiser's own name is the second field in both shapes.
        assert_eq!(
            carried.1, mine,
            "seed {seed:#x}: an error lost its raiser's name"
        );
        assert_ne!(
            carried.1, theirs,
            "seed {seed:#x}: a neighbour's name leaked in"
        );
        assert_ne!(
            carried.0, mine,
            "seed {seed:#x}: an owner is its own foreigner"
        );
        quiet = quiet.saturating_add(u32::from(event.raiser == 0));
        trace.record(carried.0);
    }
    assert_eq!(
        quiet, 1,
        "seed {seed:#x}: the quiet tenant raised exactly one error"
    );
    Ok(trace.hash())
}

#[test]
/// Two tenants' errors never share a tenant field, however loud one of them is.
fn sim_error_layers_neighbour_two_tenants_never_share_a_tenant_field() -> TestResult {
    replays(sim_neighbour)
}

/// What the walk expects of the error it holds, kept by hand beside it.
struct Model {
    /// The arm the error must be in.
    tag: &'static str,
    /// The certainty it must answer.
    certainty: DispatchCertainty,
    /// The class it must answer.
    class: RetryClass,
    /// Whether it is a flow wrapping a world failure, which can be taken apart.
    bot_flow: bool,
    /// The line it must render.
    line: String,
    /// The kernel line under the flow wrapper.
    bare: String,
}

/// Carry an error up its own layer's `From` again.
fn rewrap(error: BotError) -> BotError {
    match error {
        BotError::Kernel(source) => BotError::from(source),
        BotError::World(source) => BotError::from(source),
        BotError::Session(source) => BotError::from(source),
        BotError::Script(source) => BotError::from(source),
        other => other,
    }
}

/// Take the world failure out of a flow that wraps one, into the world arm.
fn demote(error: BotError) -> BotError {
    match error {
        BotError::Script(FlowError::Bot { source, .. }) => BotError::from(*source),
        other => other,
    }
}

/// Put a script-arm flow inside a spent retry.
fn exhaust(error: BotError, attempts: u32) -> BotError {
    match error {
        BotError::Script(last) => BotError::from(FlowError::Exhausted {
            at: path("retry"),
            attempts,
            last: Box::new(last),
        }),
        other => other,
    }
}

/// Random layer operations on one error, checked against the model after each.
fn monkey_walk(rng: &mut Rng, trace: &mut Trace, seed: u64) {
    let spec = Spec::draw(rng);
    let (certainty, class) = spec.kind.answer();
    let bare = spec.kind.build(&spec.fields).to_string();
    let mut model = Model {
        tag: spec.route.tag(),
        certainty,
        class,
        bot_flow: spec.route == Route::Flow,
        line: spec.line(),
        bare,
    };
    let mut error = spec.raise();
    for step in 0..20_u32 {
        match rng.below(4) {
            0 => error = rewrap(error),
            1 if model.bot_flow => {
                error = demote(error);
                model.tag = "world";
                model.bot_flow = false;
                model.line.clone_from(&model.bare);
            }
            2 if model.tag == "script" => {
                error = exhaust(error, step);
                model.certainty = DispatchCertainty::Refused;
                model.class = RetryClass::Never;
                model.bot_flow = false;
                model.line = format!("retry: gave up after {step} attempts; last: {}", model.line);
            }
            _ => {}
        }
        let at = format!("seed {seed:#x} step {step} {:?}", spec.route);
        assert_eq!(arms(&error), 1, "{at}: one arm");
        assert_eq!(layer_of(&error), model.tag, "{at}: arm");
        assert_eq!(
            error.dispatch_certainty(),
            model.certainty,
            "{at}: certainty"
        );
        assert_eq!(error.retry_class(), model.class, "{at}: class");
        assert_eq!(error.to_string(), model.line, "{at}: line");
    }
    trace.record(&model.line);
}

/// A thousand seeds, each its own walk.
pub(super) fn sim_monkey(seed: u64) -> Hash {
    let mut trace = Trace::new();
    for index in 0..1000_u64 {
        let inner = seed.wrapping_add(index.wrapping_mul(0x9e37_79b9));
        monkey_walk(&mut Rng::new(inner), &mut trace, inner);
    }
    Ok(trace.hash())
}

#[test]
/// A thousand random walks over layer operations keep the layer invariant.
fn sim_error_layers_monkey_1000_walks_over_layer_operations_keep_the_invariant() -> TestResult {
    replays(sim_monkey)
}
