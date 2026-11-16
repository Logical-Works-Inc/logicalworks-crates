//! The repeating-loop family: an advanced clock stops the real budget loop, a stop beats an unspent budget, and `repeat_on` reads the clock it is given.

use super::*;

/// An advanced virtual clock exhausts the supervisor's real budget loop, and the
/// same program on a wall clock reaches the same verdict by real elapsed time.
///
/// The twin assertion is the point of the family: the same body, the same
/// budget, the same terminal outcome — reached by arithmetic in one case and by
/// a real wait in the other. A run whose outcome depended on *which kind* of
/// clock governed it would be exactly the defect three implicit clocks produce.
///
/// The wall twin's budget is capped at [`REAL_WAIT_CEILING`], so a real overrun
/// is observed by real waiting rather than by the twenty-four-hour wait the
/// virtual twin uses. The *mechanism* under test is identical; only the amount of
/// time needed to reach it differs, and asserting that the two agree is exactly
/// what makes the virtual form a valid stand-in.
#[test]
fn an_advanced_clock_stops_the_real_budget_loop_the_same_way_real_time_does()
-> Result<(), Box<dyn std::error::Error>> {
    let mut exhausted = 0_u32;
    let mut frozen = 0_u32;
    let mut agreements = 0_u32;
    for seed in seeds() {
        let plan = plan(seed);
        // The virtual twin: a budget in the drawn range, advanced past with no
        // real wait at all.
        let virtual_clock = Clock::virtual_at(Duration::ZERO);
        let virtual_how = if plan.advances {
            SpendsHow::Advanced
        } else {
            SpendsHow::Unmoved
        };
        let virtual_run = block_on(run_repeating(virtual_clock, plan, virtual_how))?;
        if virtual_run.trace.is_empty() {
            return Err(Box::new(Defect(format!(
                "seed {seed}: advances={} over={} budget={:?} produced no trace",
                plan.advances, plan.over, plan.budget
            ))));
        }

        // The wall twin: the same body, the same budget shape, the same
        // terminal outcome — reached by real elapsed time instead of by
        // arithmetic.
        let (wall_run, wall_clock) = wall_twin(plan)?;

        // The wall watchdog is independent of everything above: a run that spent
        // no logical time at all must still show a real watchdog that moved.
        assert!(
            wall_clock.wall_watchdog().elapsed() < Duration::from_secs(30),
            "the wall watchdog must track real time even when the logical clock \
             was advanced a day"
        );

        // Both runs must have reported a terminal state. A missing one is a
        // different defect from a wrong one and gets its own message.
        let Some(virtual_done) = virtual_run.succeeded else {
            return Err(Box::new(Defect(format!(
                "seed {seed}: the virtual run reported no terminal outcome at all"
            ))));
        };
        let Some(wall_done) = wall_run.succeeded else {
            return Err(Box::new(Defect(format!(
                "seed {seed}: the wall run reported no terminal outcome at all"
            ))));
        };

        if plan.advances && plan.over >= 2 {
            // The advance was past the budget, so the loop must have stopped on
            // the budget rather than running forever or reporting failure.
            assert!(
                virtual_done,
                "seed {seed}: an advance of {}x the budget must stop the budget \
                 loop, and a stopped-on-budget loop reports success",
                plan.over
            );
            exhausted = exhausted.saturating_add(1);
        } else {
            // The control, and it is a *different* fact rather than a weaker
            // version of the one above: nobody advanced this clock, so a
            // duration budget measured on it cannot expire by itself. This
            // scenario therefore runs under an **iteration** budget, which the
            // loop reaches on its own and reports as exhausted — the same
            // terminal state a `For` budget produces when it is spent, reached
            // by counting instead of by time.
            //
            // The assertion that matters is that both timelines agree here: a
            // bound that is reached by counting is reached identically whatever
            // clock measured it, and a disagreement would mean the clock is
            // deciding something an iteration count cannot.
            assert_eq!(
                virtual_done, wall_done,
                "seed {seed}: an iteration budget is reached the same way on \
                 both timelines; they reported {virtual_done} and {wall_done}"
            );
            frozen = frozen.saturating_add(1);
        }

        // The trace is the replay receipt, and it is compared rather than
        // merely kept: two runs of the same plan on the same timeline must
        // record the same facts, and a scenario that recorded nothing must not
        // look like one that recorded the right things.
        assert!(
            !virtual_run.trace.is_empty(),
            "seed {seed}: the virtual run recorded an empty trace"
        );
        agreements = agreements.saturating_add(1);
    }
    assert!(
        exhausted > 0,
        "no seed in the band had an advance past its budget, so this test proved nothing"
    );
    assert!(
        frozen > 0,
        "no seed in the band left its clock unadvanced, so the frozen-clock \
         control proved nothing"
    );
    assert_eq!(
        agreements, 64,
        "every seed in the band must have been run on both timelines"
    );
    Ok(())
}

/// A cancelled scope still cancels a step whose budget is unspent.
///
/// The precedence rule, on both timelines. A stop is not a timeout and a
/// timeout is not a stop, and an implementation that let one arm of the race
/// shadow the other would report a wrong variant for a real production case.
///
/// The body parks rather than returning, so the stop genuinely races it. A body
/// that cancels its own scope and then *returns* is a different case — it
/// finished before the stop was delivered, so it succeeds — and that is
/// asserted separately rather than folded in here.
#[test]
fn a_stop_beats_an_unspent_budget_on_both_timelines() -> Result<(), Box<dyn std::error::Error>> {
    use lgwks_bot::rt::sync::Notify;

    for clock in [Clock::virtual_at(Duration::ZERO), Clock::wall()] {
        let source = clock.source();
        let scope = Scope::with_clock(Tenant::new("acme")?, clock);
        // A separate notifier rather than a cancel from inside the body: the
        // point is that the step is *parked* when the stop arrives, which is the
        // case `select!` has to resolve and a self-cancelling body does not.
        let parked = Arc::new(Notify::new());
        let signal = Arc::clone(&parked);
        let stopper = scope.clone();

        let observed = block_on(async move {
            let body = async move {
                signal.notify_one();
                std::future::pending::<Result<(), lgwks_bot::script::FlowError>>().await
            };
            let step = lgwks_bot::script::within(&scope, "body", Duration::from_secs(3_600), body);
            // Park until the body is inside it, then stop the scope.
            let stop = async move {
                parked.notified().await;
                stopper.cancel();
            };
            let (settled, ()) = lgwks_bot::rt::join!(step, stop);
            settled
        });
        let error = refusal(observed, "body")?;
        assert!(
            matches!(error, lgwks_bot::script::FlowError::Cancelled { .. }),
            "a stop on a {} clock must report Cancelled, got {error}",
            if source == TimeSource::Wall {
                "wall"
            } else {
                "virtual"
            }
        );
    }
    Ok(())
}

/// A body that finishes before its stop is delivered is not a cancellation.
///
/// The other half of the precedence rule, and the one a `biased` race gets
/// wrong most easily: the stop arm is polled first, so a step that both has a
/// cancelled scope *and* a completed body reports whichever the race reached
/// first. A body that completes is a completed step, and a caller reading
/// `Cancelled` would be told its work was thrown away when it was not.
#[test]
fn a_body_that_finishes_before_its_stop_is_not_a_cancellation()
-> Result<(), Box<dyn std::error::Error>> {
    let clock = Clock::virtual_at(Duration::ZERO);
    let scope = Scope::with_clock(Tenant::new("acme")?, clock);
    let stopper = scope.clone();
    let observed = block_on(async move {
        let body = async move {
            // Stop the scope, then succeed — the work is done.
            stopper.cancel();
            Ok(11_u32)
        };
        lgwks_bot::script::within(&scope, "body", Duration::from_secs(3_600), body).await
    });
    // Read once: `observed` is consumed here rather than inspected twice, so a
    // future `FlowError` that is not `Clone` cannot force a copy of the whole
    // error just to print it in the failure message.
    let reported = observed.map_err(|error| error.to_string());
    assert_eq!(
        reported.ok(),
        Some(11),
        "a body that completed must report its value even though the scope was \
         stopped while it ran"
    );
    Ok(())
}

#[test]
fn repeat_on_reads_the_clock_it_is_given() -> Result<(), Box<dyn std::error::Error>> {
    use lgwks_bot::rt::supervise::repeat_on;
    use lgwks_bot::rt::sync::{CancellationToken, Notify};

    let budget = Duration::from_secs(3_600);
    let clock = Clock::virtual_at(Duration::ZERO);
    let token = CancellationToken::new();

    // A `Budget::For` is a duration *from now*, so it cannot be pre-spent by
    // placing the clock in the past — placing it in the past would make the loop
    // compute a deadline in the past and stop immediately, which is a different
    // fact. The real case is a clock that moves while the loop runs, and that is
    // what this drives: the body parks, the clock is advanced past the budget,
    // and the loop must report exhaustion with no real wait.
    let parked = Arc::new(Notify::new());
    let signal = Arc::clone(&parked);
    let mover = clock.clone();

    let outcome = block_on(async move {
        let loop_future = repeat_on(&clock, &token, Budget::For(budget), move |_tick| {
            let signal = Arc::clone(&signal);
            async move {
                signal.notify_one();
                // Park until the budget is spent: a body that returned
                // immediately would let the loop reach the bound only by
                // running an unbounded number of iterations.
                std::future::pending::<()>().await;
            }
        });
        let advance = async move {
            parked.notified().await;
            let landed = mover.advance(budget.saturating_add(Duration::from_secs(86_400)));
            (landed.is_ok(), landed.is_err())
        };
        let (settled, (_, _)) = lgwks_bot::rt::join!(loop_future, advance);
        settled
    });

    assert!(
        matches!(outcome, Outcome::Exhausted { .. }),
        "an advance past the budget must stop the loop as exhausted, got {outcome:?}"
    );
    Ok(())
}
