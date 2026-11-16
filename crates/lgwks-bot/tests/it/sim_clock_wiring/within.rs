//! The `within` family: a step budget is refused by the clock and not by a real wait, and a real overrun and a logical one report the same refusal.

use super::*;

/// A `within` budget is refused by the clock, not by a real wait.
///
/// The blocking wiring claim, on the path a flow author actually writes: a
/// step whose budget is 24 hours is refused because the caller advanced its
/// declared clock past the budget, and the assertion is that this happened
/// without waiting for anything. A wall-clock step with the same budget is not
/// run here — it would cost 24 hours — which is precisely why the wall twin is
/// run with a short budget in the supervisor test above.
#[test]
fn a_within_budget_is_refused_by_an_advance_with_no_real_wait()
-> Result<(), Box<dyn std::error::Error>> {
    let budget = Duration::from_secs(86_400);
    let clock = Clock::virtual_at(Duration::ZERO);
    let scope = Scope::with_clock(Tenant::new("acme")?, clock.clone());
    // The body does the work and then spends the budget doing it. A wall-clock
    // step that overran by any amount reports `TimedOut` rather than its value,
    // because the timer already fired; this must reach the same verdict with no
    // real wait at all, which is the whole claim.
    let outcome = block_on(lgwks_bot::script::within(
        &scope,
        "overrun",
        budget,
        async {
            // The advance is bound rather than discarded: a refusal here would
            // mean the ceiling moved, which a budget of this size cannot cause,
            // and `let_underscore_must_use` forbids hiding a `Result` behind a
            // discard. Returning it as the body's error would change the
            // outcome under test, so it is read into a value the assertion
            // below does not depend on.
            let landed = clock.advance(budget.saturating_add(Duration::from_secs(1)));
            debug_assert!(landed.is_ok(), "a day must stay inside the ceiling");
            Ok(())
        },
    ));
    let error = refusal(outcome, "overrun")?;
    assert!(
        matches!(error, lgwks_bot::script::FlowError::TimedOut { .. }),
        "a spent budget must report TimedOut, got {error}"
    );
    Ok(())
}

/// A `within` budget unspent is honoured, and the body runs to completion.
///
/// The control for the test above, and the one that catches a `within` that
/// refuses everything. A clock nobody advanced must leave a short step alone.
#[test]
fn a_within_budget_nobody_advanced_is_honoured() -> Result<(), Box<dyn std::error::Error>> {
    let clock = Clock::virtual_at(Duration::ZERO);
    let scope = Scope::with_clock(Tenant::new("acme")?, clock);
    let outcome = block_on(lgwks_bot::script::within(
        &scope,
        "healthy",
        Duration::from_secs(3_600),
        async { Ok(()) },
    ));
    assert!(
        outcome.is_ok(),
        "an unspent budget must not refuse a step, got {:?}",
        outcome.err()
    );
    Ok(())
}

/// The refusal a real overrun produces is the refusal an advance produces.
///
/// Both variants, side by side on the same program: a step on a virtual clock
/// whose budget is spent, and a step on a wall clock whose budget is spent in
/// real time. They must be the same error variant at the same step path with
/// the same budget, or the facade is describing two mechanisms.
#[test]
fn a_real_overrun_and_a_logical_one_report_the_same_refusal()
-> Result<(), Box<dyn std::error::Error>> {
    let budget = Duration::from_millis(20);

    let virtual_clock = Clock::virtual_at(budget);
    let virtual_scope = Scope::with_clock(Tenant::new("acme")?, virtual_clock);
    let virtual_error = refusal(
        block_on(lgwks_bot::script::within(
            &virtual_scope,
            "body",
            budget,
            std::future::pending::<Result<(), lgwks_bot::script::FlowError>>(),
        )),
        "body",
    )?;

    let wall_clock = Clock::wall();
    let wall_scope = Scope::with_clock(Tenant::new("acme")?, wall_clock);
    let wall_error = refusal(
        block_on(lgwks_bot::script::within(
            &wall_scope,
            "body",
            budget,
            std::future::pending::<Result<(), lgwks_bot::script::FlowError>>(),
        )),
        "body",
    )?;

    assert!(
        matches!(virtual_error, lgwks_bot::script::FlowError::TimedOut { .. }),
        "the logical overrun must be TimedOut, got {virtual_error}"
    );
    assert!(
        matches!(wall_error, lgwks_bot::script::FlowError::TimedOut { .. }),
        "the real overrun must be TimedOut, got {wall_error}"
    );
    Ok(())
}
