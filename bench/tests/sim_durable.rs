//! Fools against the built `lgwks-bench-durable` binary, on seeds: the fat finger
//! at the command line, the weird bytes in an argument, the giant and the empty
//! count, the double-clicker who runs it twice, the pulled plug that is a real
//! `kill -9`, and the saboteur who checks it left nothing behind. The oracles are
//! the contract (exit 2, one typed stderr line, empty stdout) and counts a reader
//! can derive from the flow's own table (nine logged steps per run).

use std::ffi::OsString;
use std::process::{Command, Output};

use lgwks_std::json::Value;
use lgwks_std::seeded::Seeded;

#[path = "../../crates/lgwks-bot/tests/support/scratch.rs"]
mod scratch;

/// A test's result.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Logged step bodies per run in the flow: seven local steps and two calls.
const STEPS_PER_RUN: u64 = 9;

/// The binary with `args`.
fn run<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> Result<Output, std::io::Error> {
    Command::new(env!("CARGO_BIN_EXE_lgwks-bench-durable"))
        .args(args)
        .output()
}

/// The usage-refusal contract.
fn assert_usage_refusal(out: &Output, kind: &str) -> TestResult {
    let stderr = String::from_utf8(out.stderr.clone())?;
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(out.stdout.is_empty(), "stdout held {:?}", out.stdout);
    assert_eq!(stderr.lines().count(), 1, "{stderr:?}");
    let want = format!("lgwks-bench-durable: {kind}: ");
    assert!(stderr.starts_with(&want), "{stderr:?} vs {want:?}");
    Ok(())
}

/// A JSON document read off a successful run.
fn document(out: &Output) -> Result<Value, Box<dyn std::error::Error>> {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(lgwks_std::json::from_str::<Value>(&String::from_utf8(
        out.stdout.clone(),
    )?)?)
}

/// Every invocation cut from a pool of wrong flags and values is refused with the
/// type the model names: never a panic, never output, never a second line.
#[test]
fn sim_cli_fat_finger_every_bad_invocation_is_one_typed_stderr_line() -> TestResult {
    let bad: [(&[&str], &str); 8] = [
        (&["--runs", "-3"], "MalformedArgument"),
        (&["--runs", ""], "MalformedArgument"),
        (&["--runs", "+4"], "MalformedArgument"),
        (&["--runs"], "MissingValue"),
        (&["--runs=4"], "UnknownArgument"),
        (&["--RUNS", "4"], "UnknownArgument"),
        (&["--runs", "0"], "OutOfRange"),
        (&["--park-child"], "MissingValue"),
    ];
    let mut dice = Seeded::from_seed(8);
    for _ in 0..16 {
        let pick = usize::try_from(dice.below(8)?)?;
        let (args, kind) = bad.get(pick).ok_or("a case past the pool")?;
        assert_usage_refusal(&run(args)?, kind)?;
    }
    Ok(())
}

/// An argument that is not UTF-8 is refused as such, with the same contract.
#[cfg(unix)]
#[test]
fn sim_cli_weird_bytes_a_non_utf8_argument_is_refused_typed() -> TestResult {
    use std::os::unix::ffi::OsStringExt;
    let mut dice = Seeded::from_seed(0xff);
    for _ in 0..4 {
        let mut bytes = vec![b'-', b'-'];
        for _ in 0..=dice.below(5)? {
            bytes.push(0x80 + u8::try_from(dice.below(0x40)?)?);
        }
        let out = run(&[OsString::from_vec(bytes)])?;
        assert_usage_refusal(&out, "NotUnicode")?;
    }
    Ok(())
}

/// One past the least and the greatest count is refused, and the recovery mode
/// has the lower ceiling its own docs state.
#[test]
fn sim_cli_giant_and_empty_counts_one_past_each_bound_are_refused() -> TestResult {
    for words in [
        ["--runs", "0"],
        ["--runs", "100001"],
        ["--runs", "99999999999"],
        ["--recover-rounds", "0"],
        ["--recover-rounds", "101"],
    ] {
        assert_usage_refusal(&run(&words)?, "OutOfRange")?;
    }
    assert_usage_refusal(&run(&["--recover", "--runs", "10001"])?, "OutOfRange")?;
    Ok(())
}

/// The same seed, run twice, injects and retries the same: the plan is a
/// function of the seed alone, and a different seed is allowed to differ.
#[test]
fn sim_cli_double_clicker_one_seed_twice_gives_one_plan() -> TestResult {
    let mut dice = Seeded::from_seed(21);
    for _ in 0..2 {
        let seed = dice.below(1_000)?.to_string();
        let words = ["--runs", "10", "--seed", seed.as_str(), "--json"];
        let (first, second) = (document(&run(&words)?)?, document(&run(&words)?)?);
        for key in ["failures_injected", "retries", "samples", "throttled_runs"] {
            assert_eq!(
                first[key].as_u64(),
                second[key].as_u64(),
                "{key} seed {seed}"
            );
        }
    }
    Ok(())
}

/// A real `kill -9` of a process holding parked runs, twice, on a seeded count:
/// every parked run comes back and finishes, and the step log shows nine bodies
/// per resumed run and none twice.
#[test]
fn sim_cli_pulled_plug_a_real_kill_resumes_every_parked_run_once() -> TestResult {
    let mut dice = Seeded::from_seed(60);
    let runs = (10 + dice.below(21)?).to_string();
    let seed = dice.below(1_000)?.to_string();
    let out = run(&[
        "--recover",
        "--runs",
        runs.as_str(),
        "--seed",
        seed.as_str(),
        "--recover-rounds",
        "2",
        "--json",
    ])?;
    let doc = document(&out)?;
    let recovery = &doc["recovery"];
    let parked: Vec<u64> = recovery["parked_runs_per_round"]
        .as_array()
        .ok_or("no parked list")?
        .iter()
        .map(|each| each.as_u64().ok_or("a parked count is not a number"))
        .collect::<Result<_, _>>()?;
    assert_eq!(parked.len(), 2);
    let total: u64 = parked.iter().sum();
    let asked: u64 = runs.parse()?;
    assert!(
        parked.iter().all(|count| 0 < *count && *count <= asked),
        "{parked:?}"
    );
    assert_eq!(recovery["resumed_runs"].as_u64(), Some(total));
    assert_eq!(
        recovery["steps_checked"].as_u64(),
        Some(total * STEPS_PER_RUN)
    );
    assert_eq!(recovery["duplicate_step_executions"].as_u64(), Some(0));
    Ok(())
}

/// After a run the directory it was told to work under holds nothing: the
/// scratch directory it made inside is gone, on the success path and the
/// refusal path alike.
#[test]
fn sim_cli_saboteur_leaves_nothing_behind_in_the_work_directory() -> TestResult {
    let scratch = scratch::Scratch::new("durable-sim-litter")?;
    let dir = scratch.path().to_str().ok_or("the path is not UTF-8")?;
    let ok = run(&["--runs", "6", "--work-dir", dir])?;
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let refused = run(&["--runs", "0", "--work-dir", dir])?;
    assert_usage_refusal(&refused, "OutOfRange")?;
    let left = std::fs::read_dir(scratch.path())?.count();
    assert_eq!(left, 0, "the run left {left} entries under {dir}");
    Ok(())
}
