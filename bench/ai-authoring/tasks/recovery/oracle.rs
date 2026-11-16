//! Oracle for the `recovery` task. One test per contract clause.
//!
//! | Test | Clause | What it observes |
//! |---|---|---|
//! | `a_first_attempt_completes` | 1 | after enough calls every unit ran once and the total is the sum |
//! | `a_resume_does_not_rerun_a_completed_unit` | 2 | a second call leaves every completed unit's body count at 1 |
//! | `the_effect_is_applied_exactly_once` | 3 | `applies() == 1` across two calls |
//! | `the_overall_deadline_is_honoured` | 4 | a deadline overrun is `Deadline` and leaves no unit live |
//! | `dropping_the_future_leaves_no_unit_live` | 5 | after `SETTLE`, `at_rest` holds |
//!
//! The harness owns the interruption. `interrupt_after` drops the first attempt's
//! future the moment the requested number of unit bodies has recorded, which is
//! the observable equivalent of the harness killing that attempt: the run ends
//! with some records committed and none of them replayed, and the solution is
//! never told it happened.
//!
//! Deterministic in the sense the other two oracles are: no counter is asserted
//! on a wall-clock sleep. The waits are the seeded per-unit delays plus the
//! [`SETTLE`](ai_task_support::recovery::SETTLE) the drop clause allows a
//! cancelled body to be counted out.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ai_task_support::recovery::{self, SETTLE, World};
use ai_trial::{RecoveryError, recover};
use lgwks_bot::rt::time;

/// The units every world in this file has.
const WIDTH: u32 = 4;

/// A deadline long enough that no success path reaches it.
const LONG: Duration = Duration::from_secs(60);

/// How long each unit takes when the clause needs it still running.
///
/// Long enough that a 50–80 ms interruption lands *inside* a unit rather than
/// between two, and short enough that the whole file finishes in seconds. A
/// clause that interrupts a unit does not need the unit to be slow for thirty
/// seconds; it needs it to outlive the interruption, and this is three times
/// the longest budget used below.
const SLOW: Duration = Duration::from_millis(250);

/// The sum of `index * 3` over `0..width`.
fn expected_total(width: u32) -> u64 {
    (0..width).map(|index| u64::from(index) * 3).sum()
}

/// A world of [`WIDTH`] units, seeded by `seed`, each unit taking [`SLOW`].
///
/// Every clause that interrupts wants the interruption to land between two units,
/// so a uniform slow world is the honest fixture: a per-unit delay would let one
/// clause's interruption point depend on which unit the seed happened to make
/// slow.
fn world(seed: u64) -> World {
    let base = World::new(seed, WIDTH);
    (0..WIDTH).fold(base, |world, index| world.delay(index, SLOW))
}

/// Why a clause failed, one variant per way it can.
///
/// `RecoveryError` is the *solution's* error type and only `Debug` is promised for it, so the
/// oracle carries it as a value rather than asking it to be an `Error`: the oracle must not
/// require an implementation the task prompt never asked for. A test reports through `Debug`.
#[derive(Debug)]
enum OracleError {
    /// The solution refused where the clause needed it to finish.
    Refused(RecoveryError),
    /// The run ended in another way than the clause names.
    Unexpected(Result<u64, RecoveryError>),
    /// The scratch directory could not be made.
    Scratch(std::io::Error),
}

/// A clause's result.
type ClauseResult = Result<(), OracleError>;

/// The total a successful attempt reports, or the refusal it ended with.
fn finished<F: std::future::Future<Output = Result<u64, RecoveryError>>>(
    attempt: F,
) -> Result<u64, OracleError> {
    lgwks_bot::rt::runtime::block_on(attempt).map_err(OracleError::Refused)
}

/// Scratch directories made by this process so far, so two clauses never share one.
static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A scratch directory under the system temp root, named so two runs of this
/// file can never collide: the store a run leaves behind would otherwise be a
/// second ledger this file does not own, and a later test would reopen it. The
/// process id separates runs and a per-process counter separates clauses, so no
/// clock is read to name it.
fn scratch(tag: &str) -> Result<PathBuf, OracleError> {
    let ordinal = MADE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let unique = format!("lgwks-recovery-{tag}-{}-{ordinal}", std::process::id());
    let dir = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&dir).map_err(OracleError::Scratch)?;
    Ok(dir)
}

/// Remove `dir` and everything under it.
///
/// The directory is this test's own and nothing else writes to it, so the
/// cleanup cannot destroy evidence: the run's own output is the counters the
/// tests read, and those are gone by the time this runs.
fn discard(dir: &Path) {
    if dir.is_dir() {
        if let Err(error) = std::fs::remove_dir_all(dir) {
            ai_task_support::diagnostic(format_args!("could not discard {}: {error}", dir.display()));
        }
    }
}

/// Poll `future` until `ready` says the attempt has recorded `after` unit bodies,
/// then drop it — the harness's interruption.
///
/// Dropping the future is the interruption. A killed *process* would leave the
/// same durable evidence and would additionally have to be arranged across a
/// process boundary, which changes what is being measured from "does a resume
/// work" to "does this runner manage a subprocess". The dropped-attempt form is
/// what a client that walked away mid-run leaves, and it is the same state the
/// crate's own `tests/request_key.rs` observes.
fn interrupt_after<F: std::future::Future>(
    mut future: std::pin::Pin<Box<F>>,
    world: &World,
    after: u32,
) {
    lgwks_bot::rt::runtime::block_on(std::future::poll_fn(|context| {
        // One snapshot per poll rather than one per index: this loop runs on
        // every wake of the attempt it is interrupting, and each snapshot copies
        // the whole counter vector.
        let recorded = world.stats().total_runs();
        if recorded >= after {
            return std::task::Poll::Ready(());
        }
        // A future that finished before it recorded `after` units is never polled
        // again: re-polling a completed future is a panic, and either way the
        // drop below still leaves the state a walked-away client leaves.
        if future.as_mut().poll(context).is_ready() {
            return std::task::Poll::Ready(());
        }
        context.waker().wake_by_ref();
        std::task::Poll::Pending
    }));
}

/// Let every cancelled unit body be counted out.
///
/// The grace is the task's declared [`SETTLE`], spent on the injected clock: clause 5 asserts what
/// holds once that grace has passed, so the wait is the contract's own window, not a race.
fn settle() {
    let clock = time::Clock::wall();
    lgwks_bot::rt::runtime::block_on(clock.sleep(SETTLE));
}

/// Drive one `recover` attempt until it has recorded `after` unit bodies, drop
/// it, and let the cancelled bodies be counted out.
///
/// Sync rather than `async` for the same reason [`interrupt_after`] is: it owns
/// a `block_on` of its own, so an `async` caller would be nesting one runtime
/// inside another. The settle lives here because every caller wants it — a test
/// that dropped a run and read the counters immediately would be racing the drop
/// guards rather than observing them.
fn interrupt_recover(world: World, dir: PathBuf, after: u32) {
    let observed = world.clone();
    interrupt_after(Box::pin(recover(world, dir, LONG)), &observed, after);
    settle();
}

/// Clause 1: with enough attempts the work completes and the total is right.
#[test]
fn a_first_attempt_completes() -> ClauseResult {
    let dir = scratch("complete")?;
    let world = world(11);
    let total = finished(recover(world.clone(), dir.clone(), LONG))?;
    discard(&dir);

    assert_eq!(total, expected_total(WIDTH), "the sum of every unit's value");
    for index in 0..WIDTH {
        assert_eq!(
            world.stats().runs(index),
            Some(1),
            "unit {index} recorded exactly one body run"
        );
    }
    Ok(())
}

/// Clause 2: a second call finishes without re-running a completed unit.
///
/// The interruption is the harness's, not the solution's: the first attempt is
/// dropped after two unit bodies have recorded, and the resume must replay those
/// two records rather than run them again.
#[test]
fn a_resume_does_not_rerun_a_completed_unit() -> ClauseResult {
    let dir = scratch("rerun")?;
    let world = world(13);
    let interrupted = world.clone();

    interrupt_recover(interrupted, dir.clone(), 2);
    // At least the two the interruption waited for. More is legitimate: a
    // solution that fans the units out has several bodies in flight when the
    // drop lands, and the contract never asks for one unit at a time. Each body
    // counted here wrote its record (the storage owner finishes a write its
    // waiter walked away from), so every one of them must replay below.
    let recorded = world.stats().total_runs();
    assert!(
        recorded >= 2,
        "the interrupted attempt recorded at least two unit bodies, recorded {recorded}"
    );

    // The run the interrupted attempt left behind, over the same directory.
    let completed = (0..WIDTH)
        .filter(|index| world.stats().runs(*index).is_some_and(|count| count > 0))
        .collect::<Vec<u32>>();

    let total = finished(recover(world.clone(), dir.clone(), LONG))?;
    discard(&dir);

    assert_eq!(
        total,
        expected_total(WIDTH),
        "the finished run reports every unit's value"
    );
    for index in completed {
        assert_eq!(
            world.stats().runs(index),
            Some(1),
            "unit {index} had already recorded, so the second attempt must not run it again"
        );
    }
    Ok(())
}

/// Clause 3: across any number of attempts the effect is applied exactly once.
#[test]
fn the_effect_is_applied_exactly_once() -> ClauseResult {
    let dir = scratch("effect")?;
    let world = world(17);

    let first = finished(recover(world.clone(), dir.clone(), LONG))?;
    let second = finished(recover(world.clone(), dir.clone(), LONG))?;
    let applies = world.ledger().applies();
    discard(&dir);

    assert_eq!(
        (first, second),
        (expected_total(WIDTH), expected_total(WIDTH)),
        "both calls report the finished run's total"
    );
    assert_eq!(
        applies, 1,
        "the effect was applied once across two complete calls, not once per call"
    );
    Ok(())
}

/// Clause 4: the overall deadline is honoured and leaves no unit live.
#[test]
fn the_overall_deadline_is_honoured() -> ClauseResult {
    let dir = scratch("deadline")?;
    // Every unit takes far longer than the budget, so the deadline is what ends
    // the attempt rather than the work finishing.
    let world = world(19);

    let result = lgwks_bot::rt::runtime::block_on(recover(
        world.clone(),
        dir.clone(),
        Duration::from_millis(80),
    ));
    settle();
    discard(&dir);

    match result {
        Err(RecoveryError::Deadline) => {}
        other => return Err(OracleError::Unexpected(other)),
    }
    assert!(
        recovery::at_rest(&world),
        "no unit body is still live {SETTLE:?} after the deadline fired"
    );
    Ok(())
}

/// Clause 5: dropping the returned future mid-run leaves nothing live.
#[test]
fn dropping_the_future_leaves_no_unit_live() -> ClauseResult {
    let dir = scratch("drop")?;
    let world = world(23);
    let observed = world.clone();

    let attempt = Box::pin(recover(world, dir.clone(), LONG));
    interrupt_after(attempt, &observed, 1);
    settle();
    let at_rest = recovery::at_rest(&observed);
    discard(&dir);

    assert!(
        at_rest,
        "no unit body is still live {SETTLE:?} after the future was dropped"
    );
    Ok(())
}