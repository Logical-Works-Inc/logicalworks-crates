//! Simulation family: request-keyed durable submission under a seeded sweep.
//!
//! Every scenario drives the real `Host`, the real `RunStore` over a real file,
//! the real `submit`, and the real `remember`. Nothing here reimplements a
//! receipt, a run identity or a resume. What the seed controls is the shape of
//! the request space: how many keys, what payload each carries, and how the two
//! tenants share one store file.
//!
//! | Family | Property it pins |
//! |---|---|
//! | `collisions_across_two_tenants` | one key + same input reattaches, one key + different input conflicts naming both digests, and every key derives a distinct run |
//! | `drop_and_reattach` | a waiter dropped mid-effect leaves an in-flight request a later client reattaches to, with the receipt and the first record intact |
//! | `tier_submissions` | a drawn tier of distinct keys over two tenants derives as many distinct runs, and the requested/reached/ceiling levels are recorded together |
//! | `host_stops_never_poison_a_key` | a drawn stop point and disposition across two tenants: no key ever reattaches to a host stop, and every request reaches exactly one recorded terminal once driven on a healthy host, with each recorded step's body run once |
//! | `expired_deadlines_are_recorded` | a drawn declared budget across two tenants: the deadline *is* the request's own verdict, so `@terminal` is recorded, a repeat reattaches carrying `DeadlineExceeded`, and the recorded step's body never runs twice |
//! | `settle_refusals_are_reported_and_leave_the_request_unsettled_seed_a..p` | a store that refuses the `@terminal` write: the resume reports `Failed` naming the refusal and nothing is recorded, so a reopen still reads the request in flight |
//! | `same_seed_replays` | the same seed produces the same trace hash, twice |
//!
//! # What is real and what is seeded
//!
//! The store, the file, the `sync_all`, the derived run identity and the
//! receipt are real. The seed decides only the *inputs*: key count, payload and
//! tenant interleaving. No clock reading enters the trace, which is what makes
//! the hash a receipt: two runs of one seed must agree whatever the machine is
//! doing. The high-volume tiers (1,000 and 10,000 submissions) are a separate,
//! opt-in measurement (`concurrent_submissions_across_tiers`) because they are
//! minutes of real `fsync`s; the swept family stays at the level a routine run
//! can afford.

#![cfg(all(feature = "script", feature = "ephemeral"))]

use crate::scratch::Scratch;

use crate::sim;

// The band-declaration macro, defined once for the whole layer.
use crate::band_family;

// The request fixtures themselves.
use crate::request_fixtures as request;

use std::cell::Cell;
use std::error::Error;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use lgwks_bot::script::{FlowError, Scope};
use lgwks_bot::task::{
    Disposition, Host, InputDigest, RequestError, RequestKey, RunStore, Submission, Task, task,
};

use request::{
    BodyFuture, FirstStepRuns, counting_task, distinct_runs, drop_after_first_step, host_on,
    host_with_deadline_on, hosts_over, overrunning_task_on, parking_task, stop_mid_run,
    stored_host, two_step_task,
};
use sim::Band;

type TestResult = Result<(), Box<dyn Error>>;

/// The two tenants every scenario shares one store file between.
const TENANTS: [&str; 2] = ["alpha", "beta"];

/// The most keys one collision scenario draws.
const MAX_KEYS: u32 = 3;

/// The most submissions one swept tier draws.
///
/// Small on purpose: each submission commits three `fsync`-ed records, so this
/// family sweeps the *shape* of a tier cheaply while the high-volume tiers are
/// the opt-in measurement below.
const MAX_TIER: u32 = 12;

/// The ceiling this family declares for its tier, so the requested, reached and
/// ceiling levels can be read together (the INV-BOT-16 rule).
const DECLARED_TIER_CEILING: u64 = 10_000;

/// The tenant a drawn request index belongs to, alternating over [`TENANTS`].
fn tenant_for(index: u32) -> Result<&'static str, Box<dyn Error>> {
    let tenants = u32::try_from(TENANTS.len())?;
    let which = usize::try_from(index.checked_rem(tenants).ok_or("a nonzero tenant count")?)?;
    let tenant = TENANTS.get(which).ok_or("the tenant index is in range")?;
    Ok(tenant)
}

/// The host a request goes to and the key it is filed under, alternating
/// between the two tenants' hosts as `parity` flips.
fn alternating_target<'h>(
    hosts: &'h [Host],
    parity: bool,
    name: &str,
) -> Result<(&'h Host, RequestKey), Box<dyn Error>> {
    let host = hosts
        .get(usize::from(parity))
        .ok_or("the tenant index is in range")?;
    Ok((host, RequestKey::new(name)?))
}

/// One iteration of the loop it was lifted from.
fn collide_key<F: Fn(Scope, u32) -> BodyFuture>(
    sim_run: &mut sim::Sim,
    host: &Host,
    work: &Task<F>,
    tenant: &str,
    index: u32,
    payload: u32,
    runs: &mut Vec<String>,
) -> TestResult {
    let key = RequestKey::new(&format!("{tenant}-{index}"))?;

    let first = lgwks_bot::script::block_on(host.submit(&key, work, payload))?;
    assert!(
        matches!(first, Submission::Executed(_)),
        "a first submission executes"
    );
    let run = first.run_id().ok_or("a submission names a run")?;
    assert_eq!(
        first.report().and_then(|report| report.output().copied()),
        Some(payload),
        "the body's value is the recorded value"
    );
    let hex = run.id().to_hex();
    assert!(
        !runs.contains(&hex),
        "every key derives a distinct run; {tenant}/{index} collided with {hex}"
    );
    sim_run.trace.record(&format!("run {tenant} {index} {hex}"));
    runs.push(hex);

    let duplicate = lgwks_bot::script::block_on(host.submit(&key, work, payload))?;
    assert!(
        matches!(duplicate, Submission::Reattached(_)),
        "the same key and input reattaches"
    );
    assert_eq!(
        duplicate.run_id(),
        Some(run),
        "a reattach names the same derived run"
    );

    let other = payload.saturating_add(1);
    let conflict = lgwks_bot::script::block_on(host.submit(&key, work, other))
        .err()
        .ok_or("a different payload under one key must conflict")?;
    match conflict {
        RequestError::Conflict(named) => {
            assert_eq!(
                named.existing(),
                InputDigest::of(&payload),
                "the conflict names the bound input's digest"
            );
            assert_eq!(
                named.requested(),
                InputDigest::of(&other),
                "the conflict names the refused input's digest"
            );
        }
        other => {
            let refusal = Err(format!("expected a conflict, got {other:?}").into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "collision_scenario: returning an error to the caller");
            return refusal;
        }
    }
    Ok(())
}

/// Two tenants, one store file: reattach, conflict and distinct runs.
fn collision_scenario(sim_run: &mut sim::Sim) -> TestResult {
    let keys = sim_run.rng().between(1, MAX_KEYS);
    let payload = sim_run.rng().between(0, 1000);
    let scratch = Scratch::new("sim-req")?;
    // One store handle, cloned into both tenants: two handles over one file
    // would be two writers, which the store's own length fence refuses.
    let store = RunStore::open(scratch.path().join("shared.runstore"))?;
    let counter = Rc::new(Cell::new(0));
    let work = counting_task(Rc::clone(&counter))?;

    let mut runs: Vec<String> = Vec::new();
    for tenant in TENANTS {
        let host = host_on(tenant, store.clone())?;
        for index in 0..keys {
            collide_key(sim_run, &host, &work, tenant, index, payload, &mut runs)?;
        }
    }
    assert_eq!(
        counter.get(),
        u32::try_from(TENANTS.len())?.saturating_mul(keys),
        "only the first submission of each key ran the body"
    );
    sim_run
        .trace
        .record_number("bodies", u64::from(counter.get()));
    Ok(())
}

/// Drop a waiter mid-effect and reattach to the in-flight request.
fn drop_scenario(sim_run: &mut sim::Sim) -> TestResult {
    let payload = sim_run.rng().between(0, 1000);
    let scratch = Scratch::new("sim-req-drop")?;
    let host = stored_host("sim", scratch.path())?;
    let recorded = Arc::new(AtomicBool::new(false));
    let work = parking_task(Arc::clone(&recorded))?;
    let key = RequestKey::new(&format!("key-{payload}"))?;

    drop_after_first_step(Box::pin(host.submit(&key, &work, payload)), &recorded);

    let later = lgwks_bot::script::block_on(host.submit(&key, &work, payload))?;
    match later {
        Submission::InFlight(in_flight) => {
            assert!(
                in_flight.records() >= 2,
                "the receipt and the first step's record survive the drop"
            );
            sim_run
                .trace
                .record(&format!("inflight {}", in_flight.run().id().to_hex()));
            sim_run
                .trace
                .record_number("records", u64::try_from(in_flight.records())?);
        }
        other => {
            let refusal = Err(format!("expected an in-flight report, got {other:?}").into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "drop_scenario: returning an error to the caller");
            return refusal;
        }
    }
    Ok(())
}

/// A drawn tier of distinct keys over two tenants derives as many distinct runs.
fn tier_scenario(sim_run: &mut sim::Sim) -> TestResult {
    let requested = sim_run.rng().between(8, MAX_TIER);
    let scratch = Scratch::new("sim-req-tier")?;
    let store = RunStore::open(scratch.path().join("shared.runstore"))?;
    let counter = Rc::new(Cell::new(0));
    let work = counting_task(Rc::clone(&counter))?;
    let hosts = hosts_over(&TENANTS, &store)?;

    let mut runs: Vec<String> = Vec::new();
    let mut parity = false;
    for index in 0..requested {
        let (host, key) = alternating_target(&hosts, parity, &format!("tier-{index}"))?;
        let submission = lgwks_bot::script::block_on(host.submit(&key, &work, index))?;
        let run = submission.run_id().ok_or("a submission names a run")?;
        assert_eq!(
            submission
                .report()
                .map(|report| report.disposition().is_success()),
            Some(true),
            "every distinct key runs and succeeds"
        );
        runs.push(run.id().to_hex());
        parity = !parity;
    }

    assert_eq!(
        distinct_runs(&runs),
        runs.len(),
        "every distinct key derived a distinct run"
    );
    assert_eq!(
        counter.get(),
        requested,
        "every distinct key ran its body exactly once"
    );
    // Requested, reached and ceiling together, so no reader is told a number
    // nobody ran (INV-BOT-16).
    sim_run
        .trace
        .record_number("tier-requested", u64::from(requested));
    sim_run
        .trace
        .record_number("tier-reached", u64::from(requested));
    sim_run
        .trace
        .record_number("tier-ceiling", DECLARED_TIER_CEILING);
    Ok(())
}

/// A seeded sweep of the collision family.
fn collisions_across_two_tenants(band: Band) -> TestResult {
    sim::assert_replays(band, collision_scenario)?;
    Ok(())
}

/// A seeded sweep of the drop-and-reattach family.
fn drop_and_reattach(band: Band) -> TestResult {
    sim::assert_replays(band, drop_scenario)?;
    Ok(())
}

/// A seeded sweep of the tier family.
fn tier_submissions(band: Band) -> TestResult {
    sim::assert_replays(band, tier_scenario)?;
    Ok(())
}

/// The same seed produces the same trace hash, twice, over the collision and
/// drop bodies.
///
/// The tier family carries its own `assert_replays` sweep, so its determinism
/// is proven there; folding it in here as well would only pay its `fsync` cost
/// twice for a fact already established.
fn same_seed_replays(band: Band) -> TestResult {
    sim::assert_replays(band, |sim_run| {
        collision_scenario(sim_run)?;
        drop_scenario(sim_run)?;
        Ok(())
    })?;
    Ok(())
}

band_family::band_family! {
    collisions_across_two_tenants_band_00 => collisions_across_two_tenants, 0;
    collisions_across_two_tenants_band_01 => collisions_across_two_tenants, 1;
    collisions_across_two_tenants_band_02 => collisions_across_two_tenants, 2;
    collisions_across_two_tenants_band_03 => collisions_across_two_tenants, 3;
    collisions_across_two_tenants_band_04 => collisions_across_two_tenants, 4;
    collisions_across_two_tenants_band_05 => collisions_across_two_tenants, 5;
    collisions_across_two_tenants_band_06 => collisions_across_two_tenants, 6;
    collisions_across_two_tenants_band_07 => collisions_across_two_tenants, 7;
    drop_and_reattach_band_08 => drop_and_reattach, 8;
    drop_and_reattach_band_09 => drop_and_reattach, 9;
    drop_and_reattach_band_10 => drop_and_reattach, 10;
    drop_and_reattach_band_11 => drop_and_reattach, 11;
    tier_submissions_band_12 => tier_submissions, 12;
    tier_submissions_band_13 => tier_submissions, 13;
    tier_submissions_band_14 => tier_submissions, 14;
    tier_submissions_band_15 => tier_submissions, 15;
    same_seed_replays_band_00 => same_seed_replays, 0;
    same_seed_replays_band_01 => same_seed_replays, 1;
    same_seed_replays_band_02 => same_seed_replays, 2;
    same_seed_replays_band_03 => same_seed_replays, 3;
    same_seed_replays_band_04 => same_seed_replays, 4;
    same_seed_replays_band_05 => same_seed_replays, 5;
    same_seed_replays_band_06 => same_seed_replays, 6;
    same_seed_replays_band_07 => same_seed_replays, 7;
    same_seed_replays_band_08 => same_seed_replays, 8;
    same_seed_replays_band_09 => same_seed_replays, 9;
    same_seed_replays_band_10 => same_seed_replays, 10;
    same_seed_replays_band_11 => same_seed_replays, 11;
    same_seed_replays_band_12 => same_seed_replays, 12;
    same_seed_replays_band_13 => same_seed_replays, 13;
    same_seed_replays_band_14 => same_seed_replays, 14;
    same_seed_replays_band_15 => same_seed_replays, 15;
}

mod deadline;
mod scale;
mod settle;
mod stop;
