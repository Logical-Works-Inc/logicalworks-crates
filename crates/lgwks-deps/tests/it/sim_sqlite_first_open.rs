//! The first open of a store under fire (#872, #873): many openers of one new
//! file, in threads and in real processes, each carrying a step that cannot run
//! twice. SQLite's own documentation (`PRAGMA journal_mode`, "isolation") says a
//! mode change takes an exclusive lock, and a read-then-write of `user_version`
//! outside a transaction is a textbook lost update; each sim names the bug.

use std::path::Path;
use std::process::Command;
use std::sync::Barrier;
use std::thread;

use lgwks_deps::sqlite::{SqliteError, migrate, open};

use crate::lock_fixture::Scratch;
use crate::replay::{Failure, refuse, replay_sweep};

/// Steps a second run refuses or doubles: the second makes a row the oracle counts.
const ONCE_ONLY: [&str; 3] = [
    "CREATE TABLE once(id INTEGER);",
    "INSERT INTO once(id) VALUES (1);",
    "ALTER TABLE once ADD COLUMN later INTEGER;",
];

/// One opener's whole journey, as text so a thread can hand it back.
fn open_and_migrate(path: &Path, steps: &[&str]) -> Result<u32, String> {
    let connection = open(path).map_err(|error| error.to_string())?;
    let target = u32::try_from(steps.len()).map_err(|error| error.to_string())?;
    migrate(&connection, target, steps).map_err(|error| error.to_string())
}

/// `openers` threads released together onto one path.
fn race(path: &Path, steps: &[&str], openers: usize) -> Vec<Result<u32, String>> {
    let gate = Barrier::new(openers);
    thread::scope(|scope| {
        let runners: Vec<_> = (0..openers)
            .map(|_| {
                scope.spawn(|| {
                    gate.wait();
                    open_and_migrate(path, steps)
                })
            })
            .collect();
        runners
            .into_iter()
            .map(|runner| match runner.join() {
                Ok(outcome) => outcome,
                Err(_) => Err("an opener panicked".to_owned()),
            })
            .collect()
    })
}

/// The invariant: the row exists exactly once and the version is the last step.
fn assert_each_step_ran_once(path: &Path) -> Result<(), Failure> {
    let connection = open(path)?;
    let rows: i64 = connection.query_row("SELECT COUNT(*) FROM once", [], |row| row.get(0))?;
    if rows != 1 {
        return refuse(format!("the once-only insert ran {rows} times"));
    }
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 3 {
        return refuse(format!("the store stopped at version {version}"));
    }
    Ok(())
}

/// Catches: a journal-mode change refused with `database is locked` (#873), and
/// a version read outside the step's transaction running a step twice (#872).
#[test]
fn sim_sqlite_double_clicker_first_opens_run_each_step_once() -> Result<(), Failure> {
    replay_sweep("threads", 8, |rng, trace| {
        let scratch = Scratch::new("first-open")?;
        let path = scratch.path().join("store.sqlite");
        let openers = rng.between(2, 64);
        trace.record_number("openers", openers);
        for outcome in race(&path, &ONCE_ONLY, usize::try_from(openers)?) {
            match outcome {
                Ok(3) => trace.record("reached 3"),
                other => return refuse(format!("an opener saw {other:?}")),
            }
        }
        assert_each_step_ran_once(&path)
    })
}

/// Catches: an in-process mutex standing in for a lock processes share.
#[test]
fn sim_sqlite_double_clicker_across_processes_run_each_step_once() -> Result<(), Failure> {
    let binary = std::env::var_os("CARGO_BIN_EXE_lgwks-deps")
        .ok_or("Cargo did not provide the lgwks-deps test binary")?;
    replay_sweep("processes", 3, |rng, trace| {
        let scratch = Scratch::new("first-open-processes")?;
        let path = scratch.path().join("store.sqlite");
        let openers = rng.between(4, 16);
        trace.record_number("openers", openers);
        let mut running = Vec::new();
        for _ in 0..openers {
            let child = Command::new(&binary)
                .arg("sqlite")
                .arg(&path)
                .arg("3")
                .args(ONCE_ONLY)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;
            running.push(child);
        }
        for child in running {
            let output = child.wait_with_output()?;
            let printed = String::from_utf8_lossy(&output.stdout);
            if !output.status.success() || !printed.contains("user_version=3") {
                return refuse(format!(
                    "a process exited {:?}: {}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            trace.record("reached 3");
        }
        assert_each_step_ran_once(&path)
    })
}

/// Catches: a step's first statement surviving its second one's failure, and a
/// version recorded for a step that did not finish.
#[test]
fn sim_sqlite_pulled_plug_failed_step_leaves_no_version_behind() -> Result<(), Failure> {
    replay_sweep("failed-step", 16, |rng, trace| {
        let scratch = Scratch::new("failed-step")?;
        let path = scratch.path().join("store.sqlite");
        let good = rng.between(0, 5);
        let mut steps: Vec<String> = (0..good)
            .map(|step| format!("CREATE TABLE t{step}(x);"))
            .collect();
        // The half that succeeds, then the half that cannot.
        steps.push("CREATE TABLE half(x); CREATE TABLE half(x);".to_owned());
        steps.push("CREATE TABLE after(x);".to_owned());
        let borrowed: Vec<&str> = steps.iter().map(String::as_str).collect();
        let connection = open(&path)?;
        let target = u32::try_from(borrowed.len())?;
        if !matches!(
            migrate(&connection, target, &borrowed),
            Err(SqliteError::Engine { .. })
        ) {
            return refuse("a refused step must be an engine refusal");
        }
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != good {
            return refuse(format!("version {version} after {good} good steps"));
        }
        let held: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('half', 'after')",
            [],
            |row| row.get(0),
        )?;
        if held != 0 {
            return refuse("the refused step or a later one left a table behind");
        }
        trace.record_number("stopped at", version);
        // The repaired step replays the tail and only the tail.
        let mut repaired: Vec<&str> = (0..borrowed.len())
            .filter_map(|at| borrowed.get(at).copied())
            .collect();
        if let Some(slot) = repaired.get_mut(usize::try_from(good)?) {
            *slot = "CREATE TABLE half(x);";
        }
        if migrate(&connection, target, &repaired)? != target {
            return refuse("the repaired tail did not reach its target");
        }
        Ok(())
    })
}

/// Catches: a step's own transaction control committing the migration's
/// transaction out from under the version it must commit with.
#[test]
fn sim_sqlite_fat_finger_step_with_its_own_begin_is_refused() -> Result<(), Failure> {
    let scratch = Scratch::new("own-begin")?;
    let path = scratch.path().join("store.sqlite");
    let connection = open(&path)?;
    let steps = ["BEGIN; CREATE TABLE t(x); COMMIT;"];
    if !matches!(
        migrate(&connection, 1, &steps),
        Err(SqliteError::Engine { .. })
    ) {
        return refuse("a step carrying BEGIN must be refused");
    }
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 0 {
        return refuse(format!("a refused step recorded version {version}"));
    }
    Ok(())
}

/// Catches: a global first-open lock or version that one tenant's store shares
/// with another's.
#[test]
fn sim_sqlite_neighbour_two_stores_never_share_a_version() -> Result<(), Failure> {
    replay_sweep("neighbour", 8, |rng, trace| {
        let scratch = Scratch::new("neighbour")?;
        let left = scratch.path().join("left.sqlite");
        let right = scratch.path().join("right.sqlite");
        let openers = usize::try_from(rng.between(2, 16))?;
        let upto = usize::try_from(rng.between(1, 2))?;
        let short = ONCE_ONLY.get(..upto).ok_or("a prefix of the steps")?;
        let (first, second) = thread::scope(|scope| {
            let one = scope.spawn(|| race(&left, &ONCE_ONLY, openers));
            let two = scope.spawn(|| race(&right, short, openers));
            (one.join(), two.join())
        });
        let (Ok(first), Ok(second)) = (first, second) else {
            return refuse("a neighbour's race panicked");
        };
        if first.iter().chain(second.iter()).any(Result::is_err) {
            return refuse("a store refused because of its neighbour");
        }
        assert_each_step_ran_once(&left)?;
        let neighbour = open(&right)?;
        let version: u32 = neighbour.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if usize::try_from(version)? != upto {
            return refuse(format!("the right store moved to {version}"));
        }
        trace.record_number("right stopped at", version);
        Ok(())
    })
}

/// Runs after the others' shapes: a reopen creates nothing, and a store already
/// in WAL never waits on the sidecar a stuck first opener might hold.
/// Catches: every open paying for the first-open lock.
#[test]
fn sim_sqlite_saboteur_reopen_never_touches_the_sidecar() -> Result<(), Failure> {
    let scratch = Scratch::new("sidecar")?;
    let path = scratch.path().join("store.sqlite");
    drop(open(&path)?);
    let sidecar = scratch.path().join("store.sqlite.open-lock");
    if !sidecar.exists() {
        return refuse("the first open must have taken the sidecar");
    }
    std::fs::remove_file(&sidecar)?;
    drop(open(&path)?);
    if sidecar.exists() {
        return refuse("a reopen of a WAL store recreated the sidecar");
    }
    Ok(())
}
