//! Fools that break things on purpose: the pulled plug, the saboteur, the monkey
//! and the fat finger. Each drives the benchmark's own fleet, flow, stores and
//! command line on one seed; each oracle is an invariant or a reference model
//! written beside the code and never read from it.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;

use lgwks_bot::rt::runtime::block_on;
use lgwks_bot::effect::RunId;
use lgwks_bot::script::ApprovalDecision;

use crate::args::{self, DEFAULT_ROUNDS, DEFAULT_RUNS, MAX_RECOVER_RUNS, MAX_ROUNDS, MAX_RUNS};
use crate::fleet::{Parking, Rig, parking_of};
use crate::measure::measure;
use crate::sim_kit::{
    Bench, Dice, TestResult, Trace, finish, line, parked, replays, sim_clock, text, value_is_right,
};
use crate::workdir::Workdir;
use crate::world::Latencies;

/// Resume `run`; if it parks again, decide whatever the host lists and resume
/// once more. A value that is not the model's is the failure; a typed refusal
/// is an answer of `None`.
fn drive(rig: &Rig, ordinal: u32, run: RunId) -> Result<Option<u64>, String> {
    let first = block_on(rig.host.resume(run, &rig.task, ordinal));
    if value_is_right(first.output(), ordinal)? {
        return Ok(first.output().copied());
    }
    let mut decided = false;
    if let Ok(listed) = rig.host.pending(100) {
        for item in &listed {
            let verdict = rig.host.decide(
                item.id(),
                ApprovalDecision::approve("drive", *item.digest()),
            );
            decided |= verdict.is_ok();
        }
    }
    if !decided {
        return Ok(None);
    }
    let second = block_on(rig.host.resume(run, &rig.task, ordinal));
    value_is_right(second.output(), ordinal)?;
    Ok(second.output().copied())
}

/// A finished run's store cut at every tenth of its length: each cut is a plug
/// pulled after that many bytes reached the disk. Resuming from any cut gives
/// the model's value or a typed refusal, no step body runs twice, and the more
/// bytes survive the fewer bodies run again.
fn sweep(seed: u64) -> Result<u64, String> {
    let mut dice = Dice::new(seed);
    let ordinal = 2 * dice.small(3)?;
    let source = Bench::new(seed, 6, 0)?;
    let run = {
        let fleet = source.open()?;
        let rig = fleet.rig(ordinal);
        let parking = parked(rig, ordinal).map_err(line)?;
        finish(rig, ordinal, &parking).map_err(line)?;
        parking.run
    };
    let whole = std::fs::read(source.store("alpha")).map_err(text)?;
    let mut trace = Trace::new();
    let mut previous = usize::MAX;
    for cut in 0..=10_usize {
        let kept = whole
            .get(..whole.len() * cut / 10)
            .ok_or("a cut past the end")?;
        let replay = Bench::new(seed, 6, 0)?;
        std::fs::write(replay.store("alpha"), kept).map_err(text)?;
        let Ok(fleet) = replay.open() else {
            trace.note(&format!("{cut} refused"));
            continue;
        };
        let value = drive(fleet.rig(ordinal), ordinal, run)?;
        replay.steps_at_most(1)?;
        let again = replay.step_counts()?.len();
        if value.is_some() {
            if again > previous {
                lgwks_std::trace::debug!(
                    cut,
                    again,
                    previous,
                    "more bytes survived, more bodies ran again"
                );
                return Err(format!(
                    "cut {cut}: more bytes survived, more bodies ran again"
                ));
            }
            previous = again;
        }
        if cut == 10 && (value.is_none() || again != 0) {
            lgwks_std::trace::debug!(
                again,
                resumed = value.is_some(),
                "the whole file did not resume cleanly"
            );
            return Err("the whole file did not resume without rerunning a body".to_owned());
        }
        trace.note(&format!("{cut} {again} {}", value.is_some()));
    }
    Ok(trace.hash())
}

#[test]
fn sim_store_pulled_plug_a_cut_at_every_tenth_resumes_without_rerunning() -> TestResult {
    replays(sweep, 14)?;
    replays(sweep, 15)
}

/// Runs parked, the process gone without a word: the reopened stores list exactly
/// the runs that parked, tenant by tenant, and every one finishes with its value.
fn gone(seed: u64) -> Result<u64, String> {
    let mut dice = Dice::new(seed);
    let total = 3 + dice.small(8)?;
    let bench = Bench::new(seed, total, 0)?;
    let mut known: HashMap<RunId, u32> = HashMap::new();
    {
        let fleet = bench.open()?;
        for ordinal in 0..total {
            let parking = parked(fleet.rig(ordinal), ordinal).map_err(line)?;
            known.insert(parking.run, ordinal);
        }
    }
    let fleet = bench.open()?;
    let mut listed_in_all = 0_u32;
    for (slot, rig) in fleet.rigs().iter().enumerate() {
        for item in &rig.host.pending(1_000).map_err(text)? {
            let ordinal = *known
                .get(&item.run())
                .ok_or("the store lists a run nobody parked")?;
            if usize::from(!ordinal.is_multiple_of(2)) != slot {
                lgwks_std::trace::debug!(ordinal, slot, "a run came back under the other tenant");
                return Err(format!("run {ordinal} came back under the other tenant"));
            }
            finish(rig, ordinal, &parking_of(item)).map_err(line)?;
            listed_in_all += 1;
        }
    }
    if listed_in_all != total {
        lgwks_std::trace::debug!(total, listed_in_all, "a parked run did not come back");
        return Err(format!("{total} runs parked, {listed_in_all} came back"));
    }
    bench.steps_at_most(1)?;
    let mut trace = Trace::new();
    trace.note(&format!("{total} {listed_in_all}"));
    Ok(trace.hash())
}

#[test]
fn sim_store_pulled_plug_a_dropped_fleet_loses_no_parked_run() -> TestResult {
    replays(gone, 6)?;
    replays(gone, 7)?;
    replays(gone, 8)
}

/// One of five wounds to a store file, chosen by the dice: a flipped bit, a
/// truncation, a zeroed tail, junk bytes appended, a repeated segment.
fn wound(original: &[u8], dice: &mut Dice) -> Result<Vec<u8>, String> {
    let mut bytes = original.to_vec();
    let from = dice.pick(original.len())?;
    match dice.pick(5)? {
        0 => {
            let bit = dice.pick(8)?;
            if let Some(byte) = bytes.get_mut(from) {
                *byte ^= 1_u8 << bit;
            }
        }
        1 => bytes.truncate(from),
        2 => bytes.iter_mut().skip(from).for_each(|byte| *byte = 0),
        3 => {
            for _ in 0..=dice.pick(64)? {
                bytes.push(u8::try_from(dice.roll(256)?).map_err(text)?);
            }
        }
        _ => {
            let repeated = original.get(from..).ok_or("a segment past the end")?;
            bytes.extend_from_slice(repeated);
        }
    }
    Ok(bytes)
}

/// A parked run's store wounded between the kill and the resume, twelve times:
/// the reopen refuses typed or the resume answers with the model's value; a
/// wrong value, or a step body run twice, is the failure.
fn wounded(seed: u64) -> Result<u64, String> {
    let mut dice = Dice::new(seed);
    let ordinal = 2 * dice.small(2)?;
    let source = Bench::new(seed, 4, 0)?;
    let run = {
        let fleet = source.open()?;
        parked(fleet.rig(ordinal), ordinal).map_err(line)?.run
    };
    let original = std::fs::read(source.store("alpha")).map_err(text)?;
    let mut trace = Trace::new();
    for round in 0..12 {
        let replay = Bench::new(seed, 4, 0)?;
        std::fs::write(replay.store("alpha"), wound(&original, &mut dice)?).map_err(text)?;
        match replay.open() {
            Ok(fleet) => {
                let value = drive(fleet.rig(ordinal), ordinal, run)?;
                trace.note(&format!("{round} resumed {}", value.is_some()));
            }
            Err(refusal) => trace.note(&format!("{round} refused {refusal}")),
        }
        replay.steps_at_most(1)?;
    }
    Ok(trace.hash())
}

#[test]
fn sim_store_saboteur_a_wounded_file_never_resumes_to_a_wrong_value() -> TestResult {
    replays(wounded, 33)?;
    replays(wounded, 34)
}

/// Alpha's file laid under beta's name: beta lists none of alpha's runs and
/// cannot finish one.
#[test]
fn sim_store_saboteur_one_tenants_file_under_the_others_name_is_not_theirs() -> TestResult {
    let mut dice = Dice::new(52);
    let ordinal = 2 * dice.small(3)?;
    let source = Bench::new(52, 6, 0)?;
    let run = {
        let fleet = source.open()?;
        parked(fleet.rig(ordinal), ordinal).map_err(line)?.run
    };
    let swapped = Bench::new(52, 6, 0)?;
    std::fs::copy(source.store("alpha"), swapped.store("beta")).map_err(text)?;
    if let Ok(fleet) = swapped.open() {
        let [_, beta] = fleet.rigs();
        if let Ok(listed) = beta.host.pending(100) {
            assert!(
                listed.iter().all(|item| item.run() != run),
                "beta lists alpha's run"
            );
        }
        let report = block_on(beta.host.resume(run, &beta.task, ordinal));
        assert!(report.output().is_none(), "beta finished alpha's run");
    }
    swapped.steps_at_most(1)
}

/// A measurement leaves nothing behind: the scratch directory, with its stores,
/// is gone when its owner drops, and a second run under the same name works.
#[test]
fn sim_workdir_saboteur_removes_everything_a_run_wrote() -> TestResult {
    let mut dice = Dice::new(4);
    let runs = 1 + dice.small(6)?;
    let path = {
        let scratch = Workdir::create(None).map_err(line)?;
        measure(
            scratch.path(),
            runs,
            4,
            0,
            Latencies::ZERO,
            sim_clock(),
            None,
        )
        .map_err(line)?;
        let written = std::fs::read_dir(scratch.path()).map_err(text)?.count();
        assert!(written >= 2, "no store files were written: {written}");
        scratch.path().to_path_buf()
    };
    let gone = std::fs::metadata(&path);
    assert!(
        matches!(gone, Err(ref error) if error.kind() == std::io::ErrorKind::NotFound),
        "{path:?} is still there"
    );
    Ok(())
}

/// What a run of the monkey's does to the model's idea of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Parked, no decision yet.
    Parked,
    /// Decided, not yet resumed.
    Decided,
    /// Resumed to its value.
    Done,
}

/// A run the monkey started, with the stage the model says it is in.
struct Known {
    /// The run's number.
    ordinal: u32,
    /// What a decision and a resume need.
    parking: Parking,
    /// The model's stage.
    stage: Stage,
}

/// A thousand seeded moves over parked runs: start, decide, resume, list, and
/// restart the whole process. The model: a decided run finishes, an undecided one
/// never does, the listing holds exactly the undecided runs of its tenant, and no
/// step body runs twice.
fn monkey(seed: u64) -> Result<u64, String> {
    const LIMIT: u32 = 40;
    let mut dice = Dice::new(seed);
    let bench = Bench::new(seed, LIMIT, 0)?;
    let mut slot = Some(bench.open()?);
    let mut known: Vec<Known> = Vec::new();
    let mut trace = Trace::new();
    for _ in 0..1_000 {
        let op = dice.roll(100)?;
        if op >= 97 {
            drop(slot.take());
            slot = Some(bench.open()?);
            trace.note("restart");
            continue;
        }
        let fleet = slot.as_ref().ok_or("the fleet is gone")?;
        let at = if known.is_empty() {
            0
        } else {
            dice.pick(known.len())?
        };
        match op {
            0..=7 => {
                let next = u32::try_from(known.len()).map_err(text)?;
                if next < LIMIT {
                    let parking = parked(fleet.rig(next), next).map_err(line)?;
                    known.push(Known {
                        ordinal: next,
                        parking,
                        stage: Stage::Parked,
                    });
                    trace.note("start");
                }
            }
            8..=40 => {
                if let Some(run) = known.get_mut(at) {
                    let rig = fleet.rig(run.ordinal);
                    let approve = ApprovalDecision::approve("monkey", run.parking.digest);
                    let verdict = rig.host.decide(&run.parking.id, approve);
                    if run.stage == Stage::Parked {
                        verdict.map_err(text)?;
                        run.stage = Stage::Decided;
                    }
                    trace.note("decide");
                }
            }
            41..=70 => {
                if let Some(run) = known.get_mut(at) {
                    let rig = fleet.rig(run.ordinal);
                    let report = block_on(rig.host.resume(run.parking.run, &rig.task, run.ordinal));
                    let right = value_is_right(report.output(), run.ordinal)?;
                    match (run.stage, right) {
                        (Stage::Parked, true) => {
                            lgwks_std::trace::debug!(
                                ordinal = run.ordinal,
                                "a run finished without a decision"
                            );
                            return Err("finished without a decision".to_owned());
                        }
                        (Stage::Decided, false) => {
                            lgwks_std::trace::debug!(
                                ordinal = run.ordinal,
                                "a decided run did not finish"
                            );
                            return Err("a decided run did not finish".to_owned());
                        }
                        (Stage::Decided, true) => run.stage = Stage::Done,
                        (Stage::Parked | Stage::Done, _) => {}
                    }
                    trace.note("resume");
                }
            }
            _ => {
                for (index, rig) in fleet.rigs().iter().enumerate() {
                    let listed: HashSet<RunId> = rig
                        .host
                        .pending(1_000)
                        .map_err(text)?
                        .iter()
                        .map(|item| item.run())
                        .collect();
                    let model: HashSet<RunId> = known
                        .iter()
                        .filter(|run| run.stage == Stage::Parked)
                        .filter(|run| usize::from(!run.ordinal.is_multiple_of(2)) == index)
                        .map(|run| run.parking.run)
                        .collect();
                    if listed != model {
                        lgwks_std::trace::debug!(
                            index,
                            listed = listed.len(),
                            model = model.len(),
                            "a tenant's listing differs from the model"
                        );
                        return Err(format!(
                            "tenant {index}: listed {} runs, model {}",
                            listed.len(),
                            model.len()
                        ));
                    }
                }
                trace.note("list");
            }
        }
    }
    let fleet = slot.as_ref().ok_or("the fleet is gone")?;
    for run in known.iter_mut().filter(|run| run.stage == Stage::Decided) {
        let rig = fleet.rig(run.ordinal);
        let report = block_on(rig.host.resume(run.parking.run, &rig.task, run.ordinal));
        if !value_is_right(report.output(), run.ordinal)? {
            lgwks_std::trace::debug!(
                ordinal = run.ordinal,
                "a decided run did not finish at the end"
            );
            return Err("a decided run did not finish at the end".to_owned());
        }
        run.stage = Stage::Done;
    }
    let done = known.iter().filter(|run| run.stage == Stage::Done).count();
    let left = known
        .iter()
        .filter(|run| run.stage == Stage::Parked)
        .count();
    if done + left != known.len() {
        lgwks_std::trace::debug!(started = known.len(), done, left, "the model lost a run");
        return Err(format!(
            "{} started, {done} done, {left} parked",
            known.len()
        ));
    }
    bench.steps_at_most(1)?;
    trace.note(&format!("{done} {left}"));
    Ok(trace.hash())
}

#[test]
fn sim_fleet_monkey_a_thousand_moves_match_the_model() -> TestResult {
    replays(monkey, 2026)?;
    replays(monkey, 11)
}

/// The command line as a model sees it: the tokens in order, a flag that takes a
/// value takes the next token whatever it is, numbers are whole ASCII digits.
fn model_parse(tokens: &[&str]) -> Result<(u64, bool), &'static str> {
    let (mut runs, mut rounds) = (u64::from(DEFAULT_RUNS), u64::from(DEFAULT_ROUNDS));
    let (mut json, mut recover, mut child, mut dir) = (false, false, false, false);
    let mut at = 0;
    while let Some(token) = tokens.get(at) {
        at += 1;
        match *token {
            "--json" => json = true,
            "--recover" => recover = true,
            "--park-child" => child = true,
            "--runs" | "--seed" | "--recover-rounds" | "--work-dir" => {
                let value = tokens.get(at).ok_or("MissingValue")?;
                at += 1;
                if *token == "--work-dir" {
                    dir = true;
                    continue;
                }
                let whole = value.bytes().all(|byte| byte.is_ascii_digit());
                let Ok(number) = value.parse::<u64>() else {
                    lgwks_std::trace::debug!(
                        value = *value,
                        "the model reads a value that is not a number"
                    );
                    return Err("MalformedArgument");
                };
                if !whole {
                    lgwks_std::trace::debug!(
                        value = *value,
                        "the model reads a number with a non-digit"
                    );
                    return Err("MalformedArgument");
                }
                match *token {
                    "--runs" => runs = number,
                    "--recover-rounds" => rounds = number,
                    _ => {}
                }
            }
            _ => {
                lgwks_std::trace::debug!(token = *token, "the model knows no flag of this name");
                return Err("UnknownArgument");
            }
        }
    }
    if child && !dir {
        lgwks_std::trace::debug!("the model's child has no work directory");
        return Err("MissingValue");
    }
    let most = if child || recover {
        u64::from(MAX_RECOVER_RUNS)
    } else {
        u64::from(MAX_RUNS)
    };
    if !(1..=most).contains(&runs) || !(1..=u64::from(MAX_ROUNDS)).contains(&rounds) {
        lgwks_std::trace::debug!(runs, rounds, most, "the model's count is outside its bound");
        return Err("OutOfRange");
    }
    Ok((runs, json))
}

/// Four hundred seeded command lines cut from a pool of flags, wrong values,
/// misspellings and conflicts, each answered the way the model answers.
#[test]
fn sim_args_fat_finger_random_command_lines_match_the_model() -> TestResult {
    let pool = [
        "--runs",
        "7",
        "--runs",
        "0",
        "--seed",
        "9",
        "--seed",
        "x",
        "--json",
        "--recover",
        "--park-child",
        "--bogus",
        "--work-dir",
        "/tmp/w",
        "--recover-rounds",
        "3",
        "--recover-rounds",
        "900",
        "-5",
        "--runs=3",
        "100001",
        "12",
        "",
        "+4",
        "--RUNS",
    ];
    let mut dice = Dice::new(404);
    for _ in 0..400 {
        let length = 1 + dice.pick(6)?;
        let mut tokens = Vec::new();
        for _ in 0..length {
            tokens.push(
                *pool
                    .get(dice.pick(pool.len())?)
                    .ok_or("a token past the pool")?,
            );
        }
        let words: Vec<OsString> = tokens.iter().map(OsString::from).collect();
        match (args::parse(words), model_parse(&tokens)) {
            (Ok(got), Ok((runs, json))) => {
                assert_eq!((u64::from(got.runs), got.json), (runs, json), "{tokens:?}");
            }
            (Err(got), Err(kind)) => assert_eq!(got.kind(), kind, "{tokens:?}"),
            (got, want) => {
                lgwks_std::trace::debug!(?tokens, "the command line and its model disagree");
                return Err(format!("{tokens:?}: code {got:?}, model {want:?}"));
            }
        }
    }
    Ok(())
}
