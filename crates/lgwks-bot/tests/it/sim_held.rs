//! Neighbour simulations for the admitted plan store (#889).
//!
//! The store holds plans for every tenant under one lock. Before #889 it
//! checked its total ceiling before the tenant's own, so sixteen tenants at
//! their limit filled it and every later tenant's first plan was refused
//! `BoundExceeded` while that tenant held none. The store now reserves a floor
//! for each tenant, shares the rest through one pool and caps the tenants it
//! holds. The oracle in every test is a reference model written from the
//! rules, never the store's own answer: 256 plans a tenant, a floor of 4,
//! 1,024 tenants, a shared pool of 4,096 and 8,192 plans in all. One seed
//! draws every choice.

#![cfg(all(feature = "edge", feature = "script"))]

use std::collections::{HashMap, HashSet};
use std::error::Error;

use lgwks_bot::proposal::Surface;
use lgwks_bot::rt::apps::{AppStore, OpRegistry, Outcome, PlanLimits, RefusalKind};

use crate::apps_harness::demo_registry;
use crate::sim;
use sim::Rng;

/// A test's result: an error fails it with the error's text.
type TestResult = Result<(), Box<dyn Error>>;

/// The plans one tenant may hold.
const CEILING: usize = 256;
/// The plans each tenant can always hold.
const FLOOR: usize = 4;
/// The tenants the store holds plans for.
const TENANTS: usize = 1_024;
/// The plans past every tenant's floor, shared.
const POOL: usize = 4_096;
/// The plans the store holds in all.
const TOTAL: usize = 8_192;

/// A failure naming its seed.
fn seeded(seed: u64, what: impl std::fmt::Display) -> Box<dyn Error> {
    format!("seed {seed}: {what}").into()
}

/// The text plan number `n`: the same operation with a label that makes its
/// bytes, and so its id, differ from every other number's.
fn plan_body(n: u64) -> Vec<u8> {
    format!("flow app():\n    let out = graph:\n        node fetch:\n            \"p{n}\"\n")
        .into_bytes()
}

/// Which bound a refusal named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// The tenant's own ceiling.
    Tenant,
    /// The ceiling on tenants holding plans.
    Tenants,
    /// The shared pool past the floors.
    Pool,
}

impl Reason {
    /// The bound a refusal's message names.
    fn of(message: &str) -> Option<Self> {
        if message.contains("shared pool") {
            Some(Self::Pool)
        } else if message.contains("tenants, past the ceiling") {
            Some(Self::Tenants)
        } else if message.starts_with("tenant ") {
            Some(Self::Tenant)
        } else {
            None
        }
    }
}

/// What the store answered one admission.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// Admitted under this id.
    Admitted(String),
    /// Refused `BoundExceeded` for this bound.
    Bound(Reason),
}

/// The verdict of one outcome: a refusal that is not a store bound is an error.
fn verdict_of(outcome: &Outcome) -> Result<Verdict, String> {
    if let Some(admitted) = outcome.admitted() {
        return Ok(Verdict::Admitted(admitted.id().to_owned()));
    }
    let refusals = outcome.refusals();
    match (refusals.first(), refusals.len()) {
        (Some(only), 1) if only.kind() == RefusalKind::BoundExceeded => Reason::of(only.message())
            .map(Verdict::Bound)
            .ok_or_else(|| format!("a bound no rule names: {}", only.message())),
        _ => Err(format!("refused for another reason: {refusals:?}")),
    }
}

/// The registry and the surfaces built from it, one per tenant.
struct Rig {
    /// The operations every tenant admits against.
    registry: OpRegistry,
    /// Each tenant's surface, built once.
    surfaces: HashMap<String, Surface>,
}

impl Rig {
    /// A rig over the demo registry.
    fn new() -> Result<Self, String> {
        Ok(Self {
            registry: demo_registry()?,
            surfaces: HashMap::new(),
        })
    }

    /// Admit plan `n` for `tenant`.
    fn admit(&mut self, store: &AppStore, tenant: &str, n: u64) -> Result<Verdict, String> {
        if !self.surfaces.contains_key(tenant) {
            let surface = self
                .registry
                .surface(tenant, &[])
                .map_err(|error| format!("the surface for {tenant} is refused: {error}"))?;
            self.surfaces.insert(tenant.to_owned(), surface);
        }
        let surface = self
            .surfaces
            .get(tenant)
            .ok_or_else(|| format!("lost the surface for {tenant}"))?;
        let body = plan_body(n);
        verdict_of(&store.admit_text(surface, &self.registry, body.as_slice()))
    }

    /// Admit distinct plans for `tenant`, numbering from `*next`, until the
    /// store refuses; how many it admitted.
    fn greedy(&mut self, store: &AppStore, tenant: &str, next: &mut u64) -> Result<usize, String> {
        let mut admitted = 0_usize;
        for _ in 0..=CEILING.saturating_add(1) {
            let n = *next;
            *next = next.saturating_add(1);
            match self.admit(store, tenant, n)? {
                Verdict::Admitted(_) => admitted = admitted.saturating_add(1),
                Verdict::Bound(_) => return Ok(admitted),
            }
        }
        Err(format!("{tenant} was never refused"))
    }
}

/// What the reference model predicts for a plan not yet held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Predict {
    /// Admitted.
    Admit,
    /// Refused for this bound.
    Deny(Reason),
}

/// The four admission rules, written from the spec and nothing else.
#[derive(Default)]
struct Model {
    /// Each tenant's held plans: number and id.
    held: HashMap<String, Vec<(u64, String)>>,
    /// The plans held past each tenant's floor.
    shared: usize,
    /// The plans held in all.
    total: usize,
}

impl Model {
    /// How many plans `tenant` holds.
    fn count(&self, tenant: &str) -> usize {
        match self.held.get(tenant) {
            Some(plans) => plans.len(),
            None => 0,
        }
    }

    /// The verdict the rules give a plan `tenant` does not hold.
    fn predict(&self, tenant: &str) -> Predict {
        let held = self.count(tenant);
        if held >= CEILING {
            Predict::Deny(Reason::Tenant)
        } else if held == 0 && self.held.len() >= TENANTS {
            Predict::Deny(Reason::Tenants)
        } else if held < FLOOR {
            Predict::Admit
        } else if self.shared >= POOL {
            Predict::Deny(Reason::Pool)
        } else {
            Predict::Admit
        }
    }

    /// Record plan `n` as held by `tenant`.
    fn commit(&mut self, tenant: &str, n: u64, id: String) {
        if self.count(tenant) >= FLOOR {
            self.shared = self.shared.saturating_add(1);
        }
        self.total = self.total.saturating_add(1);
        self.held
            .entry(tenant.to_owned())
            .or_default()
            .push((n, id));
    }
}

/// Which bounds a sweep reached, so a bound that never bound is caught.
#[derive(Default)]
struct Reached {
    /// Refusals for the tenant's ceiling.
    tenant: usize,
    /// Refusals for the ceiling on tenants.
    tenants: usize,
    /// Refusals for the shared pool.
    pool: usize,
    /// Repeats of a held plan.
    repeats: usize,
}

/// Requirement: a new tenant's floor survives full neighbours (#889).
///
/// Catches the old order of checks: the total ceiling came before the tenant's
/// own, so sixteen greedy tenants filled the store and every later tenant's
/// first plan was refused. Each greedy tenant here must also land exactly where
/// the model puts it: floor, then whatever the pool still holds, to 256.
#[test]
fn sim_apps_store_neighbour_full_tenants_never_refuse_a_new_tenants_floor() -> TestResult {
    sim::assert_replays(sim::band_of(72), |sim| {
        let seed = sim.seed;
        let mut rng = sim.rng().clone();
        let store = AppStore::new(PlanLimits::default());
        let mut rig = Rig::new()?;
        let greedy = rng.between(16, 40);
        let mut next = u64::from(rng.below(100_000));
        let mut pool_left = POOL;
        let mut taken = 0_usize;
        for index in 0..greedy {
            let tenant = format!("greedy-{seed}-{index}");
            let admitted = rig.greedy(&store, &tenant, &mut next)?;
            let wanted = CEILING.min(FLOOR.saturating_add(pool_left));
            if admitted != wanted {
                return Err(seeded(
                    seed,
                    format!("{tenant} held {admitted} plans, the model says {wanted}"),
                ));
            }
            pool_left = pool_left.saturating_sub(wanted.saturating_sub(FLOOR));
            taken = taken.saturating_add(admitted);
        }
        for index in 0..50_u32 {
            let tenant = format!("fresh-{seed}-{index}");
            for _ in 0..FLOOR {
                let n = next;
                next = next.saturating_add(1);
                match rig.admit(&store, &tenant, n)? {
                    Verdict::Admitted(_) => taken = taken.saturating_add(1),
                    Verdict::Bound(reason) => {
                        return Err(seeded(
                            seed,
                            format!("{tenant} was refused inside its floor: {reason:?}"),
                        ));
                    }
                }
            }
        }
        sim.record(format!("greedy {greedy} taken {taken} pool_left {pool_left}").as_str());
        Ok(())
    })
}

/// Requirement: every verdict is the model's, across every bound (#889).
///
/// Catches a count that drifts from the plans held, a floor a neighbour can
/// take, a pool that eats a floor, a tenant ceiling or tenant cap that never
/// binds, and a repeat of a held plan refused at a limit. The moves are skewed:
/// a few hot tenants hammer their ceiling or the pool, and a wide tail of cold
/// tenants pushes the store past its tenant cap.
#[test]
fn sim_apps_store_saboteur_every_verdict_matches_the_reference_model() -> TestResult {
    let mut reached = Reached::default();
    sim::assert_replays(sim::band_of(73), |sim| {
        let seed = sim.seed;
        let mut rng = sim.rng().clone();
        // Even seeds draw enough hot tenants to spend the pool; odd seeds too
        // few to, so their ceilings bind instead.
        let hot = if seed % 2 == 0 {
            rng.between(17, 24)
        } else {
            rng.between(6, 14)
        };
        let names = rng.between(1_080, 1_100);
        let store = AppStore::new(PlanLimits::default());
        let mut rig = Rig::new()?;
        let mut model = Model::default();
        let mut fresh = u64::from(rng.below(100_000));
        for step in 0..14_000_u32 {
            let index = if rng.below(100) < 60 {
                rng.below(hot)
            } else {
                hot.saturating_add(rng.below(names.saturating_sub(hot)))
            };
            let tenant = format!("sab-{index}");
            let repeat = if rng.below(100) < 15 {
                model.held.get(&tenant).filter(|plans| !plans.is_empty())
            } else {
                None
            };
            if let Some(plans) = repeat {
                let pick = usize::try_from(rng.below(u32::try_from(plans.len())?))?;
                let Some((n, id)) = plans.get(pick).cloned() else {
                    return Err(seeded(seed, "picked a plan the model does not hold"));
                };
                match rig.admit(&store, &tenant, n)? {
                    Verdict::Admitted(again) if again == id => {
                        reached.repeats = reached.repeats.saturating_add(1);
                    }
                    other => {
                        return Err(seeded(
                            seed,
                            format!("step {step}: a repeat of a held plan answered {other:?}"),
                        ));
                    }
                }
                continue;
            }
            fresh = fresh.saturating_add(1);
            let predicted = model.predict(&tenant);
            match (predicted, rig.admit(&store, &tenant, fresh)?) {
                (Predict::Admit, Verdict::Admitted(id)) => model.commit(&tenant, fresh, id),
                (Predict::Deny(want), Verdict::Bound(have)) if want == have => match want {
                    Reason::Tenant => reached.tenant = reached.tenant.saturating_add(1),
                    Reason::Tenants => reached.tenants = reached.tenants.saturating_add(1),
                    Reason::Pool => reached.pool = reached.pool.saturating_add(1),
                },
                (want, have) => {
                    return Err(seeded(
                        seed,
                        format!(
                            "step {step} {tenant}: the model says {want:?}, the store {have:?}"
                        ),
                    ));
                }
            }
            if model.count(&tenant) > CEILING || model.total > TOTAL {
                return Err(seeded(seed, "the model passed a bound the rules forbid"));
            }
        }
        if model.held.len() > TENANTS || model.held.values().any(|plans| plans.len() > CEILING) {
            return Err(seeded(seed, "a tenant cap or ceiling was passed"));
        }
        sim.record(
            format!(
                "total {} shared {} tenants {}",
                model.total,
                model.shared,
                model.held.len()
            )
            .as_str(),
        );
        Ok(())
    })?;
    if reached.tenant == 0 || reached.tenants == 0 || reached.pool == 0 || reached.repeats == 0 {
        return Err(format!(
            "a bound never bound: tenant {} tenants {} pool {} repeats {}",
            reached.tenant, reached.tenants, reached.pool, reached.repeats
        )
        .into());
    }
    Ok(())
}

/// Requirement: a plan held at the ceiling is admitted again (#889).
///
/// Catches a ceiling check that runs before the held-key check, so a client
/// that double-clicks a plan it already holds is refused at 256 plans, and a
/// resubmission that takes a pool slot a second time.
#[test]
fn sim_apps_store_double_clicker_a_held_plan_at_the_ceiling_is_admitted_again() -> TestResult {
    sim::assert_replays(sim::band_of(74), |sim| {
        let seed = sim.seed;
        let mut rng = sim.rng().clone();
        let store = AppStore::new(PlanLimits::default());
        let mut rig = Rig::new()?;
        let tenant = format!("click-{}", rng.below(1_000));
        let base = u64::from(rng.below(100_000));
        let mut held: Vec<(u64, String)> = Vec::new();
        for offset in 0..CEILING {
            let n = base.saturating_add(u64::try_from(offset)?);
            match rig.admit(&store, &tenant, n)? {
                Verdict::Admitted(id) => held.push((n, id)),
                Verdict::Bound(reason) => {
                    return Err(seeded(seed, format!("plan {offset} refused: {reason:?}")));
                }
            }
        }
        let distinct: HashSet<&str> = held.iter().map(|plan| plan.1.as_str()).collect();
        if distinct.len() != CEILING {
            return Err(seeded(seed, "the held plans did not get distinct ids"));
        }
        // Fisher-Yates, so the resubmission order is the seed's.
        for at in (1..held.len()).rev() {
            let other = usize::try_from(rng.below(u32::try_from(at.saturating_add(1))?))?;
            held.swap(at, other);
        }
        let rounds = rng.between(1, 3);
        for _ in 0..rounds {
            for &(n, ref id) in &held {
                match rig.admit(&store, &tenant, n)? {
                    Verdict::Admitted(again) if again == *id => {}
                    other => {
                        return Err(seeded(seed, format!("a held plan answered {other:?}")));
                    }
                }
            }
        }
        let extra = base.saturating_add(u64::try_from(CEILING)?);
        for _ in 0..rounds {
            match rig.admit(&store, &tenant, extra)? {
                Verdict::Bound(Reason::Tenant) => {}
                other => {
                    return Err(seeded(seed, format!("a 257th plan answered {other:?}")));
                }
            }
        }
        let neighbour = format!("quiet-{}", rng.below(1_000));
        for offset in 0..FLOOR {
            let n = extra
                .saturating_add(1)
                .saturating_add(u64::try_from(offset)?);
            if !matches!(rig.admit(&store, &neighbour, n)?, Verdict::Admitted(_)) {
                return Err(seeded(seed, "a neighbour was refused inside its floor"));
            }
        }
        sim.record(format!("held {} rounds {rounds}", held.len()).as_str());
        Ok(())
    })
}

/// What one worker thread left behind.
struct Worker {
    /// How many plans each of its greedy tenants held.
    greedy: Vec<usize>,
    /// How many plans its fresh tenant held.
    fresh: usize,
}

/// One thread: its own seeded greedy tenants admit until refused, then a fresh
/// tenant takes its floor.
fn worker(store: &AppStore, seed: u64, thread: u32) -> Result<Worker, String> {
    let mixed = seed ^ u64::from(thread.saturating_add(1)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut rng = Rng::new(mixed);
    let mut rig = Rig::new()?;
    let mut next = u64::from(rng.below(100_000));
    let mut greedy = Vec::new();
    for index in 0..rng.between(3, 6) {
        let tenant = format!("th{thread}-greedy-{index}");
        greedy.push(rig.greedy(store, &tenant, &mut next)?);
    }
    let fresh_tenant = format!("th{thread}-fresh");
    let mut fresh = 0_usize;
    for _ in 0..FLOOR {
        let n = next;
        next = next.saturating_add(1);
        match rig.admit(store, &fresh_tenant, n)? {
            Verdict::Admitted(_) => fresh = fresh.saturating_add(1),
            Verdict::Bound(reason) => {
                let error = format!("thread {thread}: the fresh tenant was refused: {reason:?}");
                lgwks_std::trace::warn!(error = error.as_str(), "sim: a floor plan was refused");
                return Err(error);
            }
        }
    }
    Ok(Worker { greedy, fresh })
}

/// Run eight workers against one store, in parallel.
fn eight_workers(store: &AppStore, seed: u64) -> Result<Vec<Worker>, String> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8_u32)
            .map(|thread| scope.spawn(move || worker(store, seed, thread)))
            .collect();
        let mut done = Vec::with_capacity(handles.len());
        for handle in handles {
            let error = match handle.join() {
                Ok(Ok(worker)) => {
                    done.push(worker);
                    continue;
                }
                Ok(Err(error)) => error,
                Err(_) => String::from("a worker thread panicked"),
            };
            lgwks_std::trace::warn!(error = error.as_str(), "sim: a worker failed");
            return Err(error);
        }
        Ok(done)
    })
}

/// Requirement: the bounds hold under eight threads at once (#889).
///
/// Catches a count updated outside the lock, a pool slot taken twice, and a
/// floor a racing neighbour can take. The total is order independent: every
/// tenant holds its floor, and the greedy ones spend the pool to its last slot
/// or stop at their ceilings, so the model names it exactly.
#[test]
fn sim_apps_store_neighbour_eight_threads_admitting_at_once_keep_every_bound() -> TestResult {
    sim::assert_replays(sim::band_of(75), |sim| {
        let seed = sim.seed;
        let store = AppStore::new(PlanLimits::default());
        let workers = eight_workers(&store, seed)?;
        let mut tenants = 0_usize;
        let mut greedy_tenants = 0_usize;
        let mut total = 0_usize;
        for worker in &workers {
            if worker.fresh != FLOOR {
                return Err(seeded(seed, "a fresh tenant lost its floor"));
            }
            tenants = tenants
                .saturating_add(1)
                .saturating_add(worker.greedy.len());
            greedy_tenants = greedy_tenants.saturating_add(worker.greedy.len());
            total = total.saturating_add(worker.fresh);
            for held in &worker.greedy {
                if *held > CEILING || *held < FLOOR {
                    return Err(seeded(seed, format!("a greedy tenant held {held}")));
                }
                total = total.saturating_add(*held);
            }
        }
        let demand = greedy_tenants.saturating_mul(CEILING.saturating_sub(FLOOR));
        let wanted = tenants
            .saturating_mul(FLOOR)
            .saturating_add(demand.min(POOL));
        if total != wanted || total > TOTAL {
            return Err(seeded(
                seed,
                format!("held {total} plans, the model says {wanted}"),
            ));
        }
        sim.record(format!("tenants {tenants} total {total}").as_str());
        Ok(())
    })
}
