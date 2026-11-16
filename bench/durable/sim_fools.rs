//! Fools against the real workload: the double-clicker, the wrong-order caller,
//! the giant and the empty, the weird bytes, the liar upstream, the clock fool and
//! the neighbour. Each test is one seeded scenario run through the benchmark's own
//! flow, fleet, measurement and command line; each oracle is an invariant or a
//! reference model that uses none of the code under test, and every scenario that
//! returns a trace hash is run twice on one seed to prove the replay is the same.

use std::ffi::OsString;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use lgwks_bot::rt::runtime::block_on;
use lgwks_bot::effect::RunId;
use lgwks_bot::script::ApprovalDecision;
use lgwks_bot::task::Host;

use crate::args::{self, MAX_RECOVER_RUNS, MAX_ROUNDS, MAX_RUNS};
use crate::error::BenchError;
use crate::fleet::{Finish, Rig, grant_and_resume, tenant_of};
use crate::flow::flow_task;
use crate::measure::measure;
use crate::sim::reference;
use crate::sim_kit::{
    Bench, Dice, TestResult, Trace, driven, finish, line, parked, replays, sim_clock, text,
    value_is_right, zero_digest,
};
use crate::workdir::Workdir;
use crate::world::{ATTEMPTS, Call, LOGGED_STEPS, Latencies, World, expected};

/// The same approval decided twice, once more with the wrong digest: the run
/// still ends once, with the model's value, and no step body runs twice.
fn click_decide(seed: u64) -> Result<u64, String> {
    let mut dice = Dice::new(seed);
    let ordinal = dice.small(6)?;
    let bench = Bench::new(seed, 6, 0)?;
    let fleet = bench.open()?;
    let rig = fleet.rig(ordinal);
    let parking = parked(rig, ordinal).map_err(line)?;
    let mut trace = Trace::new();
    let first = rig.host.decide(
        &parking.id,
        ApprovalDecision::approve("clicker", parking.digest),
    );
    if first.is_err() {
        lgwks_std::trace::debug!(ordinal, "the first decision was refused");
        return Err("the first decision was refused".to_owned());
    }
    let again = rig.host.decide(
        &parking.id,
        ApprovalDecision::approve("clicker", parking.digest),
    );
    trace.note(&format!("again {}", again.is_ok()));
    let wrong = rig.host.decide(
        &parking.id,
        ApprovalDecision::approve("liar", zero_digest()?),
    );
    trace.note(&format!("wrong {}", wrong.is_ok()));
    let report = block_on(rig.host.resume(parking.run, &rig.task, ordinal));
    if !value_is_right(report.output(), ordinal)? {
        lgwks_std::trace::debug!(
            ordinal,
            disposition = report.disposition().label(),
            "the run did not finish"
        );
        return Err(format!(
            "the run did not finish: {:?}",
            report.disposition()
        ));
    }
    trace.note(report.disposition().label());
    bench.steps_at_most(1)?;
    Ok(trace.hash())
}

#[test]
fn sim_fleet_double_clicker_decides_one_approval_twice() -> TestResult {
    replays(click_decide, 3)?;
    replays(click_decide, 77)
}

/// The same run resumed again after it finished: any answer it gives carries the
/// recorded value, and nothing ran twice.
fn click_resume(seed: u64) -> Result<u64, String> {
    let mut dice = Dice::new(seed);
    let ordinal = dice.small(4)?;
    let again = 1 + dice.small(3)?;
    let bench = Bench::new(seed, 4, 0)?;
    let fleet = bench.open()?;
    let rig = fleet.rig(ordinal);
    let parking = parked(rig, ordinal).map_err(line)?;
    finish(rig, ordinal, &parking).map_err(line)?;
    let mut trace = Trace::new();
    for _ in 0..again {
        let report = block_on(rig.host.resume(parking.run, &rig.task, ordinal));
        let right = value_is_right(report.output(), ordinal)?;
        trace.note(&format!("{} {right}", report.disposition().label()));
        if !right && report.error().is_none() {
            lgwks_std::trace::debug!(ordinal, "a resume without a value named no error");
            return Err("a resume without a value named no error".to_owned());
        }
    }
    bench.steps_at_most(1)?;
    Ok(trace.hash())
}

#[test]
fn sim_fleet_double_clicker_resumes_one_finished_run_again_and_again() -> TestResult {
    replays(click_resume, 5)?;
    replays(click_resume, 19)
}

/// Eight threads resume one decided run at once: whoever gets a value gets the
/// model's, at least one gets it, and each step body ran once in all.
#[test]
fn sim_fleet_double_clicker_races_eight_resumes_of_one_run() -> TestResult {
    const RACERS: usize = 8;
    let mut dice = Dice::new(41);
    let ordinal = dice.small(6)?;
    let bench = Bench::new(41, 6, 0)?;
    let fleet = bench.open()?;
    let rig = fleet.rig(ordinal);
    let parking = parked(rig, ordinal).map_err(line)?;
    rig.host
        .decide(
            &parking.id,
            ApprovalDecision::approve("racer", parking.digest),
        )
        .map_err(text)?;
    let seen: Vec<Option<u64>> = std::thread::scope(|scope| {
        let racers: Vec<_> = (0..RACERS)
            .map(|_| {
                scope.spawn(|| {
                    let report = block_on(rig.host.resume(parking.run, &rig.task, ordinal));
                    report.output().copied()
                })
            })
            .collect();
        racers
            .into_iter()
            .map(|racer| racer.join().map_err(|_| "a racer panicked".to_owned()))
            .collect::<Result<Vec<_>, _>>()
    })?;
    let valued: Vec<u64> = seen.iter().flatten().copied().collect();
    assert!(!valued.is_empty(), "no racer finished the run");
    assert!(
        valued.iter().all(|got| *got == expected(ordinal)),
        "{valued:?}"
    );
    // Steps 1 to 7 were recorded before the park; 9 and 10 were not. Since #1055
    // the engine holds one resumer per run, so each body runs at most once.
    for (row, seen) in bench.step_counts()? {
        assert!(
            seen <= 1,
            "{row} ran {seen} times for {RACERS} racers, most is 1"
        );
    }
    Ok(())
}

/// A decision for an approval that was never raised is refused, whatever the id
/// looks like, and it leaves nothing behind.
fn decide_early(seed: u64) -> Result<u64, String> {
    let mut dice = Dice::new(seed);
    let bench = Bench::new(seed, 4, 0)?;
    let fleet = bench.open()?;
    let mut trace = Trace::new();
    for rig in fleet.rigs() {
        for _ in 0..5 {
            let id = format!("approval-{:x}", dice.roll(u64::MAX)?);
            let verdict = rig
                .host
                .decide(&id, ApprovalDecision::approve("early", zero_digest()?));
            if verdict.is_ok() {
                lgwks_std::trace::debug!(
                    id = id.as_str(),
                    "an approval was decided before it parked"
                );
                return Err(format!("{id} was decided before it parked"));
            }
            trace.note(&id);
        }
        let listed = rig.host.pending(50).map_err(text)?;
        if !listed.is_empty() {
            lgwks_std::trace::debug!(
                listed = listed.len(),
                "an approval is listed that nobody raised"
            );
            return Err("an approval is listed that nobody raised".to_owned());
        }
    }
    if !bench.step_counts()?.is_empty() {
        lgwks_std::trace::debug!("a step body ran with no run");
        return Err("a step body ran with no run".to_owned());
    }
    Ok(trace.hash())
}

#[test]
fn sim_fleet_wrong_order_decides_before_anything_parks() -> TestResult {
    replays(decide_early, 8)?;
    replays(decide_early, 9)
}

/// A run id nobody started: resuming it never reports a value without all nine
/// logged steps having run once.
#[test]
fn sim_fleet_wrong_order_resumes_a_run_nobody_started() -> TestResult {
    let mut dice = Dice::new(13);
    for round in 0..3 {
        let ordinal = dice.small(4)?;
        let bench = Bench::new(13 + round, 4, 0)?;
        let fleet = bench.open()?;
        let rig = fleet.rig(ordinal);
        let ghost = RunId::mint().map_err(|error| format!("{error:?}"))?;
        let report = block_on(rig.host.resume(ghost, &rig.task, ordinal));
        let counts = bench.step_counts()?;
        if value_is_right(report.output(), ordinal)? {
            let ran = LOGGED_STEPS
                .iter()
                .filter(|step| {
                    counts.contains_key(&format!("{} {ordinal} {step}", tenant_of(ordinal)))
                })
                .count();
            assert_eq!(
                ran,
                LOGGED_STEPS.len(),
                "a value arrived over skipped steps"
            );
        }
        bench.steps_at_most(1)?;
    }
    Ok(())
}

/// Resuming before the decision any number of times leaves the run parked and
/// reruns nothing; the decision and one more resume then finish it.
fn resume_early(seed: u64) -> Result<u64, String> {
    let mut dice = Dice::new(seed);
    let ordinal = dice.small(5)?;
    let eager = 1 + dice.small(4)?;
    let bench = Bench::new(seed, 5, 0)?;
    let fleet = bench.open()?;
    let rig = fleet.rig(ordinal);
    let parking = parked(rig, ordinal).map_err(line)?;
    let before = bench.step_counts()?.len();
    let mut trace = Trace::new();
    for _ in 0..eager {
        let report = block_on(rig.host.resume(parking.run, &rig.task, ordinal));
        if report.output().is_some() {
            lgwks_std::trace::debug!(ordinal, "a run finished without its approval");
            return Err("a run finished without its approval".to_owned());
        }
        trace.note(report.disposition().label());
    }
    if bench.step_counts()?.len() != before {
        lgwks_std::trace::debug!(ordinal, before, "an early resume ran step bodies again");
        return Err("an early resume ran step bodies again".to_owned());
    }
    finish(rig, ordinal, &parking).map_err(line)?;
    bench.steps_at_most(1)?;
    Ok(trace.hash())
}

#[test]
fn sim_fleet_wrong_order_resumes_before_the_decision() -> TestResult {
    replays(resume_early, 21)?;
    replays(resume_early, 22)
}

/// Zero runs and one run: the empty measurement is empty and says so, the single
/// run is counted once. The empty summary is driven in `sim_edges`.
fn tiny_measures(seed: u64) -> Result<u64, String> {
    let mut trace = Trace::new();
    let empty_dir = Workdir::create(None).map_err(line)?;
    let empty = measure(
        empty_dir.path(),
        0,
        seed,
        20,
        Latencies::ZERO,
        sim_clock(),
        None,
    )
    .map_err(line)?;
    if empty.finished != 0 || !empty.latencies_us.is_empty() || empty.retries != 0 {
        lgwks_std::trace::debug!(
            finished = empty.finished,
            retries = empty.retries,
            "a measurement of nothing reported something"
        );
        return Err("a measurement of nothing reported something".to_owned());
    }
    let one_dir = Workdir::create(None).map_err(line)?;
    let one = measure(
        one_dir.path(),
        1,
        seed,
        0,
        Latencies::ZERO,
        sim_clock(),
        None,
    )
    .map_err(line)?;
    trace.note(&format!("{} {}", one.finished, one.latencies_us.len()));
    if one.finished != 1 || one.latencies_us.len() != 1 {
        lgwks_std::trace::debug!(
            finished = one.finished,
            "one clean run was not counted once"
        );
        return Err(format!("one clean run was counted {} times", one.finished));
    }
    Ok(trace.hash())
}

#[test]
fn sim_measure_giant_and_empty_zero_runs_and_one_run() -> TestResult {
    replays(tiny_measures, 2)?;
    replays(tiny_measures, 3)
}

/// The accepted range of a count flag, as a model: whole ASCII digits, inside the bound.
fn model_accepts(spelled: &str, low: u64, high: u64) -> bool {
    if spelled.is_empty() || !spelled.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if let Ok(found) = spelled.parse::<u64>() {
        (low..=high).contains(&found)
    } else {
        false
    }
}

/// Every bound, one past it and ten times it, plus seeded numbers, against the model.
#[test]
fn sim_args_giant_and_empty_every_bound_and_one_past() -> TestResult {
    let mut dice = Dice::new(99);
    let cases: [(&[&str], &str, u64, u64); 3] = [
        (&[], "--runs", 1, u64::from(MAX_RUNS)),
        (&["--recover"], "--runs", 1, u64::from(MAX_RECOVER_RUNS)),
        (&[], "--recover-rounds", 1, u64::from(MAX_ROUNDS)),
    ];
    for (before, flag, low, high) in cases {
        let mut probes = vec![
            0,
            1,
            2,
            high - 1,
            high,
            high + 1,
            high * 10,
            u64::from(u32::MAX),
            u64::MAX,
        ];
        for _ in 0..200 {
            probes.push(dice.roll(high * 3)?);
        }
        for probe in probes {
            let mut words: Vec<OsString> = before.iter().map(OsString::from).collect();
            words.push(flag.into());
            words.push(probe.to_string().into());
            let parsed = args::parse(words);
            let wants = model_accepts(&probe.to_string(), low, high);
            assert_eq!(parsed.is_ok(), wants, "{flag} {probe}: {parsed:?}");
            if let Err(error) = parsed {
                assert!(matches!(error, BenchError::OutOfRange { .. }), "{error:?}");
            }
        }
    }
    let past = args::parse(["--runs", "18446744073709551616"].map(OsString::from));
    assert!(matches!(past, Err(BenchError::MalformedArgument { .. })));
    Ok(())
}

/// Listing pending approvals with every limit from nothing to everything returns
/// exactly the smaller of the limit and what the tenant holds.
#[test]
fn sim_fleet_giant_and_empty_pending_limits_from_zero_to_max() -> TestResult {
    let mut dice = Dice::new(31);
    let total = 4 + dice.small(5)?;
    let bench = Bench::new(31, total, 0)?;
    let fleet = bench.open()?;
    let mut held = [0_usize; 2];
    for ordinal in 0..total {
        parked(fleet.rig(ordinal), ordinal).map_err(line)?;
        if let Some(count) = held.get_mut(usize::from(!ordinal.is_multiple_of(2))) {
            *count += 1;
        }
    }
    for limit in [0, 1, 2, 3, 100, usize::MAX] {
        for (rig, mine) in fleet.rigs().iter().zip(held) {
            match rig.host.pending(limit) {
                Ok(listed) => assert_eq!(listed.len(), limit.min(mine), "limit {limit}"),
                Err(error) => assert_eq!(limit, 0, "limit {limit} refused: {error}"),
            }
        }
    }
    Ok(())
}

/// Weird bytes as flag values: NUL, invalid UTF-8, marks that read as digits,
/// path tricks, signs and floats. The model: a count flag takes whole ASCII digits
/// inside its bound, anything not UTF-8 is `NotUnicode`, `--work-dir` takes any text.
#[test]
fn sim_args_weird_bytes_are_refused_typed_and_never_panic() -> TestResult {
    let mut weird: Vec<OsString> = [
        "\0",
        "5\0",
        "\u{202e}5",
        "5\u{200b}",
        "../..",
        "%00",
        "NaN",
        "-0",
        "-9223372036854775808",
        "18446744073709551615",
        "\u{ff11}\u{ff12}",
        "\u{663}",
        "5 ",
        "1_000",
        "+7",
        "0x1f",
        "1e3",
    ]
    .map(OsString::from)
    .to_vec();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        weird.push(OsString::from_vec(vec![0xff, 0xfe, b'5']));
        weird.push(OsString::from_vec(vec![0xc0, 0xaf]));
    }
    for flag in ["--runs", "--seed", "--recover-rounds", "--work-dir"] {
        for value in &weird {
            let parsed = args::parse([OsString::from(flag), value.clone()]);
            let Some(spelled) = value.to_str() else {
                assert!(matches!(parsed, Err(BenchError::NotUnicode)), "{flag}");
                continue;
            };
            let wants = match flag {
                "--work-dir" => true,
                "--seed" => model_accepts(spelled, 0, u64::MAX),
                "--runs" => model_accepts(spelled, 1, u64::from(MAX_RUNS)),
                _ => model_accepts(spelled, 1, u64::from(MAX_ROUNDS)),
            };
            assert_eq!(parsed.is_ok(), wants, "{flag} {spelled:?}: {parsed:?}");
        }
    }
    Ok(())
}

/// A work directory named with NUL, a missing parent, `..` through nothing, or a
/// plain file is refused as unusable and nothing appears where it was aimed.
#[test]
fn sim_workdir_weird_bytes_parents_are_refused_and_leave_nothing() -> TestResult {
    let scratch = Workdir::create(None).map_err(line)?;
    let file = scratch.path().join("plain-file");
    std::fs::write(&file, b"x").map_err(text)?;
    let before = std::fs::read_dir(scratch.path()).map_err(text)?.count();
    let missing = scratch.path().join("no/such/parent");
    let through = scratch.path().join("..").join("..").join("nowhere-at-all");
    let aimed: [&Path; 4] = [Path::new("a\0b"), &missing, &through, &file];
    for parent in aimed {
        let made = Workdir::create(Some(parent));
        assert!(
            matches!(made, Err(BenchError::UnreadablePath { .. })),
            "{parent:?}"
        );
    }
    assert_eq!(
        std::fs::read_dir(scratch.path()).map_err(text)?.count(),
        before
    );
    Ok(())
}

/// Tenant names with NUL, slashes, marks and a thousand characters: the engine
/// refuses each typed or runs it, and a host it accepts lists its own run.
#[test]
fn sim_host_weird_bytes_tenant_names_refuse_typed_or_run() -> TestResult {
    let long = "t".repeat(1_000);
    let names = [
        "",
        "a\0b",
        "a/b",
        "../x",
        "\u{202e}rtl",
        "Alpha",
        "alpha",
        long.as_str(),
    ];
    let bench = Bench::new(61, 4, 0)?;
    let mut accepted = Vec::new();
    for (at, name) in names.iter().enumerate() {
        let built = Host::builder(name)
            .map_err(text)
            .and_then(|builder| builder.run_store(bench.dir.path()).map_err(text))
            .and_then(|builder| builder.build().map_err(text));
        let Ok(host) = built else { continue };
        let task = flow_task(std::sync::Arc::clone(&bench.world)).map_err(text)?;
        let rig = Rig {
            host,
            task,
            clock: bench.world.clock().clone(),
        };
        let ordinal = u32::try_from(at % 4).map_err(text)?;
        let parking = parked(&rig, ordinal).map_err(line)?;
        let listed = rig.host.pending(50).map_err(text)?;
        assert!(
            listed.iter().any(|found| found.run() == parking.run),
            "{name:?} lost its own run"
        );
        accepted.push(*name);
    }
    assert!(accepted.contains(&"alpha"), "a plain name was refused");
    Ok(())
}

/// Every attempt fails: the measurement ends, with every run counted as given
/// up, inside a wall-clock bound, instead of hanging.
#[test]
fn sim_world_liar_upstream_total_failure_ends_typed_and_counted() -> TestResult {
    const RUNS: u32 = 12;
    let (sender, receiver) = mpsc::sync_channel(1);
    let dir = Workdir::create(None).map_err(line)?;
    let worker = std::thread::Builder::new()
        .name("liar-watchdog".to_owned())
        .spawn(move || {
            let tally = measure(dir.path(), RUNS, 4, 100, Latencies::ZERO, sim_clock(), None);
            // The error crosses the channel as its one line, so the send result stays small.
            let counts = tally
                .map(|done| (done.finished, done.throttled, done.exhausted))
                .map_err(line);
            sender.send(counts)
        })
        .map_err(text)?;
    let tally = receiver
        .recv_timeout(Duration::from_secs(60))
        .map_err(|_| "the measurement hung".to_owned())??;
    worker
        .join()
        .map_err(|_| "the worker panicked".to_owned())?
        .map_err(text)?;
    assert_eq!(tally.0, 0, "a run finished though every attempt failed");
    assert_eq!(u64::from(RUNS), tally.1 + tally.2);
    Ok(())
}

/// Whatever the failure rate, no call is attempted more than the policy allows and
/// no retry happens without a failure before it.
#[test]
fn sim_world_liar_upstream_attempts_never_exceed_the_policy() -> TestResult {
    const RUNS: u32 = 24;
    let mut dice = Dice::new(57);
    for percent in [10, 50, 90] {
        let seed = dice.roll(10_000)?;
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
        let calls = u64::from(RUNS) * 2;
        let top = u64::from(ATTEMPTS.get());
        assert!(
            tally.retries <= calls * (top - 1),
            "{percent}%: {}",
            tally.retries
        );
        assert!(
            tally.injected <= calls * top,
            "{percent}%: {}",
            tally.injected
        );
        assert!(tally.retries <= tally.injected, "{percent}%");
        if tally.throttled == 0 {
            let world = World::plan(seed, RUNS, percent, Latencies::ZERO, sim_clock(), None)
                .map_err(line)?;
            let (injected, retries, _, _) = reference(world.plans());
            assert_eq!(
                (tally.injected, tally.retries),
                (injected, retries),
                "{percent}%"
            );
        }
    }
    Ok(())
}

/// The declared percent is the share that fails: over four thousand runs the
/// outside call's first attempt fails within five points of it, zero fails never
/// and a hundred fails always; a run outside the plan is a typed refusal.
#[test]
fn sim_world_liar_upstream_failure_rate_matches_the_declared_percent() -> TestResult {
    const RUNS: u32 = 4_000;
    let mut dice = Dice::new(123);
    let seed = dice.roll(1_000_000)?;
    let share = |percent| -> Result<(u64, World), String> {
        let world =
            World::plan(seed, RUNS, percent, Latencies::ZERO, sim_clock(), None).map_err(line)?;
        let failing = world.plans().iter().filter(|plan| plan.outside > 0).count();
        Ok((u64::try_from(failing).map_err(text)?, world))
    };
    let (some, world) = share(20)?;
    let wanted = u64::from(RUNS) * 20 / 100;
    assert!(
        some.abs_diff(wanted) <= u64::from(RUNS) * 5 / 100,
        "{some} of {RUNS}"
    );
    assert_eq!(share(0)?.0, 0);
    assert_eq!(share(100)?.0, u64::from(RUNS));
    let outside = block_on(world.call(Call::Outside, RUNS + 7, 1));
    assert!(outside.is_err(), "a run outside the plan was answered");
    assert_eq!(world.injected(), 0);
    Ok(())
}

/// A zero wait and a one-microsecond wait agree on every counter: the clock
/// changes how long runs take, not which attempts fail.
#[test]
fn sim_world_clock_fool_zero_and_tiny_latencies_agree_on_counters() -> TestResult {
    const RUNS: u32 = 30;
    let tiny = Latencies {
        outside: Duration::from_micros(1),
        model: Duration::from_micros(1),
        approval: Duration::from_micros(1),
    };
    let mut seeds = 0;
    for seed in 1..=300_u64 {
        let world =
            World::plan(seed, RUNS, 10, Latencies::ZERO, sim_clock(), None).map_err(line)?;
        let (_, _, _, by_tenant) = reference(world.plans());
        if by_tenant.iter().any(|need| *need > 6) {
            continue;
        }
        let a_dir = Workdir::create(None).map_err(line)?;
        let b_dir = Workdir::create(None).map_err(line)?;
        let a = measure(
            a_dir.path(),
            RUNS,
            seed,
            10,
            Latencies::ZERO,
            sim_clock(),
            None,
        )
        .map_err(line)?;
        let clock = sim_clock();
        let b = driven(&clock, || {
            measure(b_dir.path(), RUNS, seed, 10, tiny, clock.clone(), None)
        })
        .map_err(line)?;
        assert_eq!(
            (a.retries, a.injected, a.exhausted),
            (b.retries, b.injected, b.exhausted)
        );
        seeds += 1;
        if seeds == 3 {
            return Ok(());
        }
    }
    Err("fewer than three seeds fit the retry reserve".to_owned())
}

/// A person who decides after a pause (zero to forty milliseconds, seeded) still
/// gets the run finished: a late decision is not a lost one.
#[test]
fn sim_fleet_clock_fool_a_slow_human_decides_after_a_pause() -> TestResult {
    let mut dice = Dice::new(17);
    for _ in 0..3 {
        let pause = Duration::from_millis(dice.roll(40)?);
        let ordinal = dice.small(4)?;
        let bench = Bench::new(17, 4, 0)?;
        let fleet = bench.open()?;
        let rig = fleet.rig(ordinal);
        let parking = parked(rig, ordinal).map_err(line)?;
        let done = driven(bench.world.clock(), || {
            block_on(grant_and_resume(rig, ordinal, &parking, pause))
        });
        assert!(matches!(done, Ok(Finish::Done)), "{done:?}");
        bench.steps_at_most(1)?;
    }
    Ok(())
}

/// A noisy tenant with twenty-odd parked runs does not change the quiet one:
/// the listings are exact per tenant and the quiet tenant's lone run finishes.
#[test]
fn sim_fleet_neighbour_a_noisy_tenant_leaves_the_quiet_one_exact() -> TestResult {
    let mut dice = Dice::new(73);
    let noisy = 20 + dice.small(10)?;
    let bench = Bench::new(73, 2 * noisy + 2, 0)?;
    let fleet = bench.open()?;
    for step in 0..noisy {
        parked(fleet.rig(2 * step), 2 * step).map_err(line)?;
    }
    let quiet = parked(fleet.rig(1), 1).map_err(line)?;
    let [alpha, beta] = fleet.rigs();
    let noisy_held = usize::try_from(noisy).map_err(text)?;
    assert_eq!(alpha.host.pending(1_000).map_err(text)?.len(), noisy_held);
    assert_eq!(beta.host.pending(1_000).map_err(text)?.len(), 1);
    finish(beta, 1, &quiet).map_err(line)?;
    assert_eq!(beta.host.pending(1_000).map_err(text)?.len(), 0);
    assert_eq!(alpha.host.pending(1_000).map_err(text)?.len(), noisy_held);
    let beta_rows = bench
        .step_counts()?
        .keys()
        .filter(|row| row.starts_with("beta "))
        .count();
    assert_eq!(beta_rows, LOGGED_STEPS.len());
    Ok(())
}

/// Alpha's run id offered to beta's host: alpha's listing, record and result
/// stay as they were, and nothing is logged for alpha that alpha did not run.
#[test]
fn sim_fleet_neighbour_a_foreign_run_id_never_resumes_under_the_other_tenant() -> TestResult {
    let mut dice = Dice::new(88);
    let ordinal = 2 * dice.small(3)?;
    let bench = Bench::new(88, 6, 0)?;
    let fleet = bench.open()?;
    let [alpha, beta] = fleet.rigs();
    let mine = parked(alpha, ordinal).map_err(line)?;
    let held = bench.step_counts()?;
    let stolen = block_on(beta.host.resume(mine.run, &beta.task, ordinal));
    if stolen.output().is_some() {
        return Err("beta finished a run alpha parked".to_owned());
    }
    let listed = alpha.host.pending(50).map_err(text)?;
    let runs: Vec<RunId> = listed.iter().map(|found| found.run()).collect();
    assert_eq!(runs, vec![mine.run]);
    let after = bench.step_counts()?;
    for (row, seen) in &held {
        assert_eq!(after.get(row), Some(seen), "{row}");
    }
    assert!(
        after
            .keys()
            .all(|row| !row.starts_with("alpha ") || held.contains_key(row))
    );
    finish(alpha, ordinal, &mine).map_err(line)?;
    Ok(())
}
