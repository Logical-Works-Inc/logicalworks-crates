//! Seeded simulations of the whole measurement in one process: the engine, the
//! failure plan, the run stores and the report, against reference models that
//! use none of the engine.
//!
//! The upstreams wait nothing here (`Latencies::ZERO`), so what runs is the
//! engine's logic and its disk, not the clock. The one run with the workload's
//! latencies waits on a virtual clock that the test moves, never on real time.

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use lgwks_bot::block_on;
use lgwks_bot::script::ApprovalDecision;
use lgwks_std::json::Value;
use lgwks_std::seeded::Seeded;

use crate::args;
use crate::fleet::{Finish, Fleet, grant_and_resume, tenant_of};
use crate::measure::{flow, measure};
use crate::recover::check_steps;
use crate::report::{flow_text, json};
use crate::sim_kit::{TestResult, driven, line, parked, sim_clock};
use crate::workdir::Workdir;
use crate::world::{ATTEMPTS, AppendLog, FAILURE_PERCENT, Faults, Latencies, World};

/// What the plan alone says one run does: failures injected, retries taken, and
/// whether the run is given up on the exhausted call.
fn per_run(plan: Faults) -> (u64, u64, bool) {
    let top = u64::from(ATTEMPTS.get());
    let (mut injected, mut retries) = (0, 0);
    for failing in [plan.outside, plan.model] {
        let failing = u64::from(failing);
        injected += failing;
        retries += failing.min(top - 1);
        if failing >= top {
            return (injected, retries, true);
        }
    }
    (injected, retries, false)
}

/// The reference for a whole plan: injected, retries, exhausted runs, and the
/// retries each tenant needs.
pub(crate) fn reference(plans: &[Faults]) -> (u64, u64, u64, [u64; 2]) {
    let (mut injected, mut retries, mut exhausted) = (0, 0, 0);
    let mut by_tenant = [0_u64; 2];
    for (at, plan) in plans.iter().enumerate() {
        let (shot, taken, gave_up) = per_run(*plan);
        injected += shot;
        retries += taken;
        exhausted += u64::from(gave_up);
        let slot = usize::from(at % 2 == 1);
        if let Some(sum) = by_tenant.get_mut(slot) {
            *sum += taken;
        }
    }
    (injected, retries, exhausted, by_tenant)
}

/// The model gives a run up exactly when one call fails every attempt, and
/// counts no retry past the last attempt or any call after the one given up.
#[test]
fn the_model_gives_up_exactly_at_the_attempt_count() {
    let top = ATTEMPTS.get();
    let wide = u64::from(top);
    let cases = [
        ((top - 1, 0), (wide - 1, wide - 1, false)),
        ((top, 0), (wide, wide - 1, true)),
        ((top, top), (wide, wide - 1, true)),
        ((0, top), (wide, wide - 1, true)),
        ((2, top - 2), (wide, wide, false)),
        ((0, 0), (0, 0, false)),
    ];
    for ((outside, model), want) in cases {
        assert_eq!(
            per_run(Faults { outside, model }),
            want,
            "{outside} {model}"
        );
    }
}

/// The counters of a measurement equal what the plan alone predicts, and a seed
/// run twice gives the same counters: the retry budget cannot refuse a run in
/// the seeds picked here, so scheduling cannot change them. At least one seed
/// sits exactly at the reserve, so the reserve itself is proven, not a margin.
#[test]
fn sim_counters_match_the_plan_and_repeat() -> TestResult {
    const RUNS: u32 = 40;
    /// Retries per tenant the host's budget always grants a 40-run measurement.
    const RESERVE: u64 = 8;
    /// Seeds to check.
    const WANT: u32 = 4;
    let (mut checked, mut at_reserve, mut seed) = (0, false, 0_u64);
    while (checked < WANT || !at_reserve) && seed < 400 {
        seed += 1;
        let world =
            World::plan(seed, RUNS, 10, Latencies::ZERO, sim_clock(), None).map_err(line)?;
        let (injected, retries, exhausted, by_tenant) = reference(world.plans());
        if by_tenant.iter().any(|need| *need > RESERVE) {
            continue;
        }
        at_reserve |= by_tenant.contains(&RESERVE);
        for _ in 0..2 {
            let dir = Workdir::create(None).map_err(line)?;
            let tally = measure(
                dir.path(),
                RUNS,
                seed,
                10,
                Latencies::ZERO,
                sim_clock(),
                None,
            )
            .map_err(line)?;
            assert_eq!(tally.injected, injected, "injected, seed {seed}");
            assert_eq!(tally.retries, retries, "retries, seed {seed}");
            assert_eq!(tally.exhausted, exhausted, "exhausted, seed {seed}");
            assert_eq!(tally.throttled, 0, "throttled, seed {seed}");
            assert_eq!(
                u64::from(tally.finished) + tally.exhausted,
                u64::from(RUNS),
                "every run is counted, seed {seed}"
            );
            assert_eq!(tally.latencies_us.len(), tally.finished as usize);
        }
        checked += 1;
    }
    if checked >= WANT && at_reserve {
        Ok(())
    } else {
        Err(format!(
            "{checked} seeds of {seed} fit the reserve of {RESERVE}, at it: {at_reserve}"
        ))
    }
}

/// With nothing failing every run finishes with the reference value (the
/// measurement itself refuses a wrong one), nobody retries, and the disk was
/// written by group commit: no more syncs than records.
#[test]
fn sim_a_clean_plan_finishes_every_run_with_no_retry() -> TestResult {
    for seed in 1..=4_u64 {
        let dir = Workdir::create(None).map_err(line)?;
        let tally =
            measure(dir.path(), 30, seed, 0, Latencies::ZERO, sim_clock(), None).map_err(line)?;
        assert_eq!(tally.finished, 30, "seed {seed}");
        assert_eq!((tally.retries, tally.injected), (0, 0), "seed {seed}");
        assert_eq!((tally.throttled, tally.exhausted), (0, 0), "seed {seed}");
        assert!(tally.staged > 0 && tally.fsyncs > 0, "seed {seed}");
        assert!(tally.fsyncs <= tally.staged, "seed {seed}");
    }
    Ok(())
}

/// Past what the host's retry budget allows, runs are given up and counted, never
/// lost and never timed as finished: the four counts always add up to the runs.
#[test]
fn sim_saturation_is_counted_and_never_lost() -> TestResult {
    const RUNS: u32 = 60;
    let mut stream = Seeded::from_seed(11);
    let mut given_up = 0;
    for _ in 0..4 {
        let seed = stream.below(1_000).map_err(|e| e.to_string())?;
        let percent = 60 + stream.below(35).map_err(|e| e.to_string())?;
        let dir = Workdir::create(None).map_err(line)?;
        let tally = measure(
            dir.path(),
            RUNS,
            seed,
            percent,
            Latencies::ZERO,
            sim_clock(),
            None,
        )
        .map_err(line)?;
        let all = u64::from(tally.finished) + tally.throttled + tally.exhausted;
        assert_eq!(all, u64::from(RUNS), "seed {seed} percent {percent}");
        assert_eq!(tally.latencies_us.len(), tally.finished as usize);
        given_up += tally.throttled + tally.exhausted;
    }
    assert!(given_up > 0, "a 60% or worse failure rate gave nothing up");
    Ok(())
}

/// Two tenants run the same ordinal and share nothing: each lists only its own
/// parked run, a decision aimed at the other tenant's approval is refused, and
/// both finish with the reference value.
#[test]
fn sim_two_tenants_with_one_ordinal_stay_apart() -> TestResult {
    let mut stream = Seeded::from_seed(5);
    for _ in 0..3 {
        let ordinal = u32::try_from(stream.below(8).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let dir = Workdir::create(None).map_err(line)?;
        let world = World::plan(1, 8, 0, Latencies::ZERO, sim_clock(), None).map_err(line)?;
        let fleet = Fleet::open(dir.path(), Arc::new(world)).map_err(line)?;
        let [alpha, beta] = fleet.rigs();
        let first = parked(alpha, ordinal).map_err(line)?;
        let second = parked(beta, ordinal).map_err(line)?;
        assert_ne!(first.run, second.run);
        for (rig, mine) in [(alpha, &first), (beta, &second)] {
            let listed = rig.host.pending(100).map_err(|e| e.to_string())?;
            assert_eq!(listed.len(), 1);
            assert_eq!(listed.first().map(|found| found.run()), Some(mine.run));
        }
        let foreign = alpha.host.decide(
            &second.id,
            ApprovalDecision::approve("intruder", second.digest),
        );
        assert!(foreign.is_err(), "alpha accepted a decision for beta's run");
        for (rig, mine) in [(alpha, &first), (beta, &second)] {
            let done = block_on(grant_and_resume(rig, ordinal, mine, Duration::ZERO));
            assert!(matches!(done, Ok(Finish::Done)), "{done:?}");
        }
    }
    Ok(())
}

/// An ordinary measurement, no crash, runs every step body exactly once.
#[test]
fn sim_no_step_runs_twice_without_a_crash() -> TestResult {
    const RUNS: u32 = 30;
    for seed in 1..=3_u64 {
        let dir = Workdir::create(None).map_err(line)?;
        let log_path = dir.path().join("exec.log");
        let steps = AppendLog::open(&log_path, "opening the step log").map_err(line)?;
        measure(
            dir.path(),
            RUNS,
            seed,
            FAILURE_PERCENT,
            Latencies::ZERO,
            sim_clock(),
            Some(steps),
        )
        .map_err(line)?;
        let log = std::fs::read_to_string(&log_path).map_err(|e| e.to_string())?;
        check_steps(&log, &[]).map_err(line)?;
        assert!(
            log.lines().count() >= 30,
            "the log holds too little: seed {seed}"
        );
        assert_eq!(tenant_of(0), "alpha");
    }
    Ok(())
}

/// A real measurement's text and JSON carry the same numbers.
#[test]
fn sim_text_and_json_of_a_real_run_agree() -> TestResult {
    for seed in 1..=3_u64 {
        let words: Vec<OsString> = ["--runs", "40", "--seed", &seed.to_string()]
            .iter()
            .map(OsString::from)
            .collect();
        let parsed = args::parse(words).map_err(line)?;
        let dir = Workdir::create(None).map_err(line)?;
        let clock = sim_clock();
        let doc = driven(&clock, || flow(&parsed, dir.path(), clock.clone())).map_err(line)?;
        let value = lgwks_std::json::from_str::<Value>(&json(&doc).map_err(line)?)
            .map_err(|e| e.to_string())?;
        let text = flow_text(&doc);
        let find = |key: &str| {
            text.lines()
                .find_map(|row| row.strip_prefix(key))
                .map(str::to_owned)
        };
        for key in [
            "samples",
            "retries",
            "failures_injected",
            "throttled_runs",
            "exhausted_runs",
            "peak_rss_bytes",
        ] {
            let number = value[key].as_u64().map(|n| n.to_string());
            assert_eq!(find(&format!("{key}: ")), number, "{key} seed {seed}");
        }
        assert_eq!(value["samples"].as_u64(), u64::try_from(doc.samples).ok());
        assert!(doc.peak_rss_bytes > 0, "no resident set, seed {seed}");
        assert!(doc.latency_us.p50 > 0, "a zero median, seed {seed}");
        assert!(doc.latency_us.p50 <= doc.latency_us.p95);
        assert!(doc.latency_us.p95 <= doc.latency_us.p99);
        assert!(doc.latency_us.p99 <= doc.latency_us.max);
    }
    Ok(())
}
