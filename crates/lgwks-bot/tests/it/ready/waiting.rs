//! The bounded-wait family: a wait is budgeted, cancellable and immediate when released, and a readiness admits at most its cap.

use super::*;

// ── The wait is bounded, cancellable, and immediate ─────────────────────────

/// The wait has a budget of its own, obeys the scope's stop, and costs nothing
/// when the answer already exists.
///
/// Three facts in one journey because they are one contract. The last is the one
/// a guessed sleep cannot have: a sleep costs its full duration even when the
/// readiness was released a microsecond ago, so a wait that took measurable
/// time on an already-released readiness would be a timer wearing a readiness's
/// name.
#[test]
fn a_wait_is_budgeted_cancellable_and_immediate_when_already_released() -> TestResult {
    let scope = scope_of("acme")?;

    // 1. An already-released readiness resolves without spending its budget.
    let released: Named = Readiness::new("db")?;
    released.ready(Generation::FIRST, "db.internal:5432".to_owned())?;
    let started = Instant::now();
    let ready = drive(released.wait(&scope, BUDGET))?;
    let spent = started.elapsed();
    assert_eq!(ready.awaited(), "db.internal:5432", "the released value");
    assert!(
        spent < BUDGET,
        "an already-released readiness resolved in {spent:?}, so the wait did not \
         sleep for its budget"
    );

    // 2. A stop ends the wait as a cancellation, not as a timeout. The wait runs
    //    under the very scope that is stopped, because cancellation walks *up*:
    //    cancelling a child token stops what descends from it and nothing above
    //    it, so a wait under a parent could only be stopped by that parent. This
    //    is the shape a dependant actually has — a step under a run that a
    //    caller stops.
    let waiting: Named = Readiness::new("db")?;
    let wait_under = scope.enter("use-db")?;
    let outcome = drive(cancel_soon(&waiting, &wait_under));
    assert!(
        outcome.is_cancelled(),
        "a stopped wait must report a cancellation, got {outcome}"
    );
    assert!(
        outcome.to_string().contains("use-db/await-ready"),
        "and locate it at the wait's own step under the stopped scope, got {outcome}"
    );

    // 3. A wait with no signal and no stop is ended by its own budget. The
    // budget is short here so the assertion costs milliseconds; a longer one
    // would prove the same thing more slowly.
    let silent: Named = Readiness::new("db")?;
    let quiet = Duration::from_millis(40);
    let started = Instant::now();
    let error = refused(
        drive(silent.wait(&scope, quiet)).map(|_ready| ()),
        "a readiness that is never signalled must not wait forever",
    )?;
    let spent = started.elapsed();
    assert!(
        matches!(error, FlowError::TimedOut { .. }),
        "the wait must be bounded by its own budget, got {error}"
    );
    assert!(
        spent < BUDGET,
        "the budget ended the wait in {spent:?}, well inside the harness's ceiling"
    );
    Ok(())
}

/// Cancel `scope` once the wait has been admitted, and return the wait's
/// outcome.
///
/// The stop is raised from a second task rather than after a delay, so the
/// journey observes a cancellation and not a race between two clocks: the wait
/// is already suspended when the token is cancelled, which is the only state a
/// cancellation is meant to end. The admission is read from the readiness
/// itself rather than from a flag the wait sets, so the ordering this asserts —
/// admitted, then stopped — is the ordering that happened.
async fn cancel_soon<T: Clone>(readiness: &Readiness<T>, scope: &Scope) -> FlowError {
    let admitted_at_start = readiness.admitted();
    let stop = async move {
        while readiness.admitted() == admitted_at_start {
            lgwks_bot::rt::task::yield_now().await;
        }
        scope.cancel();
    };
    let wait = async move {
        match readiness.wait(scope, Duration::from_secs(30)).await {
            // The wait resolving is the failure this row rules out, and it is
            // reported as the wait's own error rather than as a test assertion:
            // the caller returns it, so a resolved wait cannot pass.
            Ok(_ready) => FlowError::failed("the wait resolved instead of stopping"),
            // A stop is not a failure, so the error travels unchanged: this is
            // the arm that makes a routine restart read as `Cancelled`.
            Err(error) => error,
        }
    };
    let (outcome, ()) = lgwks_bot::rt::join!(wait, stop);
    assert!(
        readiness.admitted() > admitted_at_start,
        "the wait was admitted before it was stopped, so the cancellation \
         reached a suspended wait rather than one that had already answered"
    );
    outcome
}

// ── The bound ───────────────────────────────────────────────────────────────

/// A readiness admits at most its declared cap, and the charge is refused
/// rather than the cap silently raised.
#[test]
fn dependants_are_capped_at_the_declared_bound() -> TestResult {
    let scope = scope_of("acme")?;
    let cap = 2_usize;
    let readiness: Named = Readiness::named("db", Generation::FIRST, cap)?;
    assert_eq!(readiness.cap(), cap, "the cap is the one that was declared");

    readiness.ready(Generation::FIRST, "db.internal:5432".to_owned())?;
    for index in 0..cap {
        let ready = drive(readiness.wait(&scope, BUDGET))?;
        assert_eq!(
            ready.awaited(),
            "db.internal:5432",
            "dependant {index} is served"
        );
    }
    let served = u64::try_from(cap)?;
    assert_eq!(
        readiness.admitted(),
        served,
        "each admission was charged once"
    );

    let over_cap = refused(
        drive(readiness.wait(&scope, BUDGET)).map(|_ready| ()),
        "a dependant past the cap must be refused",
    )?;
    assert!(
        over_cap.to_string().contains("cap"),
        "the refusal must name the bound it reached, got {over_cap}"
    );
    // The refusal charged nothing: the cap is a bound on admissions, not a
    // budget a refused attempt spends, so a caller that sheds load sees the cap
    // again rather than a permanently exhausted readiness.
    assert_eq!(
        readiness.admitted(),
        served,
        "a refused admission charged no slot"
    );

    // The declared maximum is the ceiling the crate states, and a cap of zero is
    // raised to one rather than admitting nothing.
    let empty: Named = Readiness::named("db", Generation::FIRST, 0)?;
    assert_eq!(
        empty.cap(),
        1,
        "a zero cap is raised to one: a readiness that admitted nothing could never \
         release a dependant"
    );
    let maxed = Readiness::<String>::new("db")?;
    assert_eq!(
        maxed.cap(),
        lgwks_bot::script::MAX_DEPENDANTS,
        "the default cap is the declared maximum"
    );
    Ok(())
}
