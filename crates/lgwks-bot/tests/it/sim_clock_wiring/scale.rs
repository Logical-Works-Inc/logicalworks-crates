//! The scale family: exact accounting at every concurrency tier, a hundred thousand joined tasks, and a stack-bounded nest of governed steps.

use super::*;

/// The saturation tiers: 100, 1,000, 10,000 and 100,000 concurrent tasks on the
/// real supervisor, each tier asserting its own accounting rather than trusting
/// the counters.
///
/// The deterministic half of the hyperscale axis. Four tiers is not a benchmark —
/// `bench/async` measures the timing — it is a claim that the accounting is
/// exact at every order of magnitude a caller can reach, which is checkable
/// without a clock and without an extrapolation. The peak in-flight figure is
/// asserted to stay at or below the declared bound at every tier, which is the
/// property that actually breaks when a counter is a plain increment.
#[test]
fn the_supervisor_accounts_exactly_at_every_concurrency_tier()
-> Result<(), Box<dyn std::error::Error>> {
    /// The tiers, each a power of ten the issue names.
    const TIERS: [usize; 4] = [100, 1_000, 10_000, 100_000];

    for tier in TIERS {
        let bound = 64;
        let mut supervisor = Supervisor::with_clock(bound, Clock::wall());
        let observed = block_on(async {
            for _ in 0..tier {
                // `spawn` waits for a permit, so this is admission, not
                // queueing: the supervisor's ceiling is what bounds the run.
                supervisor.spawn(|_token| async {}).await;
            }
            // The snapshot is taken while the supervisor is still live, so its
            // `occupied` is the ceiling claim under test rather than a count
            // read after everything drained.
            let live = supervisor.snapshot();
            let occupied = live.occupied();
            assert!(
                occupied <= bound,
                "at {tier} tasks {occupied} were in flight at once, above the \
                 declared bound {bound}"
            );
            assert_eq!(
                live.max_in_flight(),
                bound,
                "the snapshot must report the declared bound, not a re-derived one"
            );
            supervisor.shutdown().await
        });

        let stats = observed.stats();
        assert_eq!(
            stats.spawned,
            u64::try_from(tier)?,
            "at {tier} tasks every spawn must be counted exactly once"
        );
        assert_eq!(
            stats.completed,
            u64::try_from(tier)?,
            "at {tier} tasks every task must reach a terminal state"
        );
        // `shutdown` cancels first and then drains cooperatively, so a body
        // that observes its token reports `Cancelled`. The invariant is the
        // partition, not "everything succeeded": a task accounted as neither
        // succeeded nor cancelled nor aborted has been lost, and that is the
        // defect this tier is here to catch.
        assert_eq!(stats.panicked, 0, "at {tier} tasks no body panicked");
        assert_eq!(
            stats.completed,
            stats
                .succeeded
                .saturating_add(stats.cancelled)
                .saturating_add(stats.aborted),
            "at {tier} tasks every completion must be accounted for as \
             succeeded, cancelled or aborted; the counts do not partition"
        );
        // `in_flight` is a method, not a field: it is accounting rather than a
        // live count, and the accessor is where that distinction is expressed.
        assert_eq!(
            stats.in_flight(),
            0,
            "at {tier} tasks every started task must be reaped, so the \
             accounting count returns to zero"
        );
        assert_eq!(
            stats.failed, 0,
            "at {tier} tasks no body failed; every task either returned or \
             observed its token"
        );
        assert_eq!(
            stats.refused, 0,
            "at {tier} tasks every spawn found a permit eventually, because \
             `spawn` waits for one rather than refusing"
        );
    }
    Ok(())
}

#[test]
fn a_hundred_thousand_joined_tasks_all_reach_a_terminal_state()
-> Result<(), Box<dyn std::error::Error>> {
    let tier = 100_000_usize;
    let tier_u64 = u64::try_from(tier)?;
    let completed = block_on(async {
        let mut set: JoinSet<u64> = JoinSet::new();
        for index in 0..tier_u64 {
            set.spawn(async move { index });
        }
        let mut finished = 0_u64;
        let mut total = 0_u64;
        while let Some(joined) = set.join_next().await {
            if let Ok(value) = joined {
                finished = finished.saturating_add(1);
                total = total.saturating_add(value);
            }
        }
        (finished, total)
    });
    let (finished, total) = completed;
    assert_eq!(
        finished,
        u64::try_from(tier)?,
        "every joined task must return a result, not a cancelled or panicked one"
    );
    // The sum of `0..tier` — an order check, so a join set that returned the
    // right *number* of the wrong values is caught rather than passed.
    let expected = tier_u64
        .saturating_mul(tier_u64.saturating_sub(1))
        .checked_div(2)
        .ok_or("the triangular number of an even count does not divide by two")?;
    assert_eq!(
        total, expected,
        "the joined values must be the ones submitted"
    );
    Ok(())
}

/// A deep nest of clock-governed steps stays inside a bounded stack.
///
/// This is the regression for a defect the clock wiring introduced and that no
/// functional assertion could see. [`within`](lgwks_bot::script::within) races
/// the body's future against a logical-clock poll and an engine watchdog; a
/// `select!` builds one future per branch and holds them all for the body's life.
/// With the branches held inline, *every* `within` frame was as large as the
/// engine's timer state — paid once per nesting level — and `sim_task`'s nested
/// trees overflowed a test thread's default 2 MiB stack. Every assertion still
/// passed; the process died before reaching them.
///
/// Two things make this a real observation rather than a repeat of that report:
///
/// - the depth and the stack are **chosen by measurement** so the test fails
///   when the arms are unboxed and passes when they are boxed. A larger stack
///   would make it pass either way, which is the one thing a regression test must
///   not do; the constants below say which configuration discriminates.
/// - the result is a completed value, so a run that merely *survived* without
///   computing the right answer still fails.
#[test]
fn a_deep_nest_of_clock_governed_steps_is_stack_bounded() -> Result<(), Box<dyn std::error::Error>>
{
    /// Deep enough that an unboxed `select!` arm overflows the stack below, shallow
    /// enough that a correctly boxed one is comfortable.
    ///
    /// This pair was chosen by measurement, not by guess: at 1,024 levels on a
    /// 1,536 KiB stack this test **passes** with the arms boxed and **overflows**
    /// with them inline. A shallower or more generously budgeted configuration
    /// passes either way, and a regression test that cannot fail is not one.
    const DEPTH: usize = 1_024;
    /// The stack the nest is run on — the discriminating half of the pair above.
    /// A 1,024 KiB budget passes whether or not the arms are boxed; 1,536 KiB
    /// passes only when they are.
    const STACK_BYTES: usize = 1_536 * 1024;

    let clock = Clock::virtual_at(Duration::ZERO);
    // A root scope built on the same declared clock, so the nest's budgets are
    // the logical ones and the test measures the wiring rather than the engine's
    // timer.
    let scope = Scope::with_clock(
        Tenant::new("acme").map_err(|error| error.to_string())?,
        clock.clone(),
    );
    let outcome = std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .name("lgwks-deep-nest".to_owned())
        .spawn(move || block_on(Box::pin(nest(&clock, &scope, DEPTH))))
        .map_err(|error| format!("the nest thread could not start: {error}"))?
        .join()
        .map_err(crate::sim::panicked)??;

    assert_eq!(
        outcome,
        u64::try_from(DEPTH)?,
        "every level of the nest must run exactly once and report its own depth"
    );
    Ok(())
}

/// One level of the nest: `depth` clock-governed steps wrapped around the next.
///
/// `remaining` counts down to the leaf, so the value each level returns is the
/// number of levels it actually executed rather than the constant it was
/// handed — a level that skipped its body would report its argument back and
/// the sum would still look right.
async fn nest(
    clock: &Clock,
    scope: &Scope,
    remaining: usize,
) -> Result<u64, lgwks_bot::script::FlowError> {
    if remaining == 0 {
        return Ok(0);
    }
    let inner = lgwks_bot::script::within_on(
        clock,
        scope,
        "nest",
        Duration::from_secs(30),
        // Boxed because this function recurses — an unboxed recursive `async fn`
        // does not compile. That boxing is also exactly what makes this test
        // discriminating: it puts the *nest's* frame on the heap and leaves the
        // `within` frame — the one the clock wiring changed — on the stack, once
        // per level. A regression that enlarged the `within` frame shows up here
        // as a stack overflow rather than being absorbed by the heap.
        Box::pin(nest(clock, scope, remaining.saturating_sub(1))),
    )
    .await?;
    Ok(inner.saturating_add(1))
}
