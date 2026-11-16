//! The tenant family: two tenants on two clocks cannot see each other's time, even racing one task id.

use super::*;

/// A tenant's clock is not another tenant's clock.
///
/// The multi-tenant axis, on the real run path: two hosts for two tenants run
/// the *same task name* at the same time, each on its own clock. Advancing one
/// tenant's clock to exhaustion must leave the other's run untouched — not its
/// deadline, not its snapshot, not its report. Two runs whose identities differ
/// only by tenant are the case where a shared clock would be invisible.
#[test]
fn two_tenants_on_two_clocks_cannot_see_each_others_time() -> Result<(), Box<dyn std::error::Error>>
{
    use lgwks_bot::task::Host;

    let acme_clock = Clock::virtual_at(Duration::ZERO);
    let globex_clock = Clock::virtual_at(Duration::ZERO);

    let acme = Host::builder("acme")?.clock(acme_clock.clone()).build()?;
    let globex = Host::builder("globex")?
        .clock(globex_clock.clone())
        .build()?;

    // A day of acme's time, spent. Globex's clock has not moved.
    acme_clock.advance(Duration::from_secs(86_400))?;
    let acme_now = acme_clock.now();
    assert_eq!(acme_now, Duration::from_secs(86_400));
    assert_eq!(
        globex_clock.now(),
        Duration::ZERO,
        "one tenant's clock must not move another's"
    );
    assert_eq!(
        globex_clock.snapshot().elapsed(),
        Duration::ZERO,
        "one tenant's snapshot must not carry another's time"
    );
    // The hosts are separate installations, so the clock each reports is its
    // own and neither observed the advance.
    assert_eq!(acme.clock().now(), Duration::from_secs(86_400));
    assert_eq!(globex.clock().now(), Duration::ZERO);
    Ok(())
}

/// An exhausted tenant clock refuses its run; an unexhausted one does not.
///
/// The same pair, now actually running work, so the isolation is observed
/// rather than asserted from two counters. `acme`'s budget is already spent and
/// `globex`'s is untouched, and the two runs must disagree.
#[test]
fn only_the_tenant_whose_clock_advanced_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    use lgwks_bot::task::{Disposition, Host, task};

    let deadline = Duration::from_secs(60);
    // Both hosts declare the *same* 60-second budget on their *own* clocks, and
    // the task name is identical across the two tenants — the case a shared
    // clock would collapse.
    let acme_clock = Clock::virtual_at(Duration::ZERO);
    let globex_clock = Clock::virtual_at(Duration::ZERO);

    let acme = Host::builder("acme")?
        .default_deadline(deadline)
        .clock(acme_clock.clone())
        .build()?;
    let globex = Host::builder("globex")?
        .default_deadline(deadline)
        .clock(globex_clock.clone())
        .build()?;

    // The body reaches its own scope's clock and spends its host's budget. The
    // input decides whether it does, so the *same* body, task name and declared
    // budget differ only in which tenant's clock was advanced. That is the
    // isolation claim: a run reaches the clock of its own host and can move
    // nothing else.
    let work = task("work", |scope: Scope, spend: bool| async move {
        if spend {
            let landed = scope
                .clock()
                .advance(deadline.saturating_add(Duration::from_secs(1)));
            if let Err(error) = landed {
                return Err(lgwks_bot::script::FlowError::failed(error.to_string()));
            }
        }
        Ok(7_u32)
    })?;

    let (acme_report, globex_report) = block_on(async {
        // acme spends its own budget; globex spends nothing.
        let acme_report = acme.run(&work, true).await;
        let globex_report = globex.run(&work, false).await;
        (acme_report, globex_report)
    });

    assert_spent_refused_and_idle_succeeded(
        &acme_report,
        Disposition::DeadlineExceeded,
        &globex_report,
        Disposition::Succeeded,
        "60-second",
    );

    // The decisive isolation claim: acme's advance moved acme's clock and left
    // globex's untouched, despite the shared task name and budget.
    assert_eq!(
        acme_clock.now(),
        deadline.saturating_add(Duration::from_secs(1)),
        "acme's own clock must carry exactly the advance its run made"
    );
    assert_eq!(
        globex_clock.now(),
        Duration::ZERO,
        "acme's run advanced acme's clock; globex's must not have moved"
    );
    Ok(())
}

/// Where a body leaves the clock readings it took, for the parent to read back.
///
/// An alias rather than the spelled-out type because the closure that consumes
/// it is already inside three layers of generic syntax, and a nested `>>` in a
/// parameter position is unreadable — this is what the parser first refused.
type Reading = Arc<std::sync::Mutex<Option<(Duration, Duration)>>>;

/// Two tenants, two clocks, one task id, joined rather than awaited in turn.
#[test]
fn two_tenants_racing_the_same_task_id_stay_separate_under_their_own_clocks()
-> Result<(), Box<dyn std::error::Error>> {
    use lgwks_bot::task::{Disposition, Host, task};

    // The sequential twin above proves the clocks are separate objects. It runs
    // acme to completion and only then starts globex, so it cannot see a clock
    // that reads another tenant's state *while both are live* — which is the
    // only shape a shared-atomic defect takes in production. Here the two runs
    // are joined, so both bodies are inside their host at once, under one task
    // name and one declared budget, and each spends only its own clock.
    let deadline = Duration::from_secs(60);
    let acme_clock = Clock::virtual_at(Duration::ZERO);
    let globex_clock = Clock::virtual_at(Duration::ZERO);

    let acme = Host::builder("acme")?
        .default_deadline(deadline)
        .clock(acme_clock)
        .build()?;
    let globex = Host::builder("globex")?
        .default_deadline(deadline)
        .clock(globex_clock.clone())
        .build()?;

    // The *same* task name on both hosts, and the same body: only the input
    // differs, so a difference in the verdict can only be the clock. The
    // readings leave through slots rather than through the run's output,
    // because the run that *fails* is exactly the one whose pre-spend reading
    // matters, and a failed run carries no output.
    let acme_slot = Arc::new(std::sync::Mutex::new(None));
    let globex_slot = Arc::new(std::sync::Mutex::new(None));
    let work = task(
        "shared-task-id",
        |scope: Scope, (spend, slot): (bool, Reading)| async move {
            // Read the clock the scope handed us *before* spending, so the reading
            // is the one a shared clock would have already polluted.
            let seen_before = scope.clock().now();
            let mut observed = (seen_before, seen_before);
            if spend {
                let landed = scope
                    .clock()
                    .advance(deadline.saturating_add(Duration::from_secs(1)));
                observed.1 = scope.clock().now();
                if let Err(error) = landed {
                    if let Ok(mut held) = slot.lock() {
                        *held = Some(observed);
                    }
                    return Err(lgwks_bot::script::FlowError::failed(error.to_string()));
                }
            }
            if let Ok(mut held) = slot.lock() {
                *held = Some(observed);
            }
            Ok(observed)
        },
    )?;

    let (acme_report, globex_report) = block_on(async {
        lgwks_bot::rt::join!(
            acme.run(&work, (true, Arc::clone(&acme_slot))),
            globex.run(&work, (false, Arc::clone(&globex_slot)))
        )
    });
    let acme_slot = Arc::into_inner(acme_slot);
    let globex_slot = Arc::into_inner(globex_slot);
    let acme_seen = acme_slot
        .and_then(|held| held.into_inner().ok())
        .flatten()
        .ok_or("acme's body recorded no clock reading")?;
    let globex_seen = globex_slot
        .and_then(|held| held.into_inner().ok())
        .flatten()
        .ok_or("globex's body recorded no clock reading")?;

    assert_spent_refused_and_idle_succeeded(
        &acme_report,
        Disposition::DeadlineExceeded,
        &globex_report,
        Disposition::Succeeded,
        "60-second",
    );

    // The reading each body took *during* the race: globex's must show the
    // instant before its own (never-taken) spend, with acme's advance nowhere
    // in it. A shared clock would have shown globex acme's elapsed time here.
    assert_eq!(
        acme_seen.0,
        Duration::ZERO,
        "acme read its clock before spending, and must have seen a zero origin"
    );
    assert_eq!(
        globex_seen,
        (Duration::ZERO, Duration::ZERO),
        "globex's clock must read zero before and after, while acme's was racing ahead: \
         a shared clock would have carried acme's advance into globex's body"
    );
    assert_eq!(
        globex_clock.now(),
        Duration::ZERO,
        "acme's concurrent run must not leave a mark on globex's clock"
    );
    Ok(())
}

/// The pair of verdicts every isolation row in this family must produce: the
/// tenant that spent its own budget is refused, and the one that spent nothing
/// succeeds.
///
/// Written once because two rows assert it and the assertion is the family:
/// the sequential twin and the joined race differ in *scheduling*, and a
/// difference in verdicts between them is exactly the defect the second exists
/// to find. Stating it in one place keeps the two rows from drifting into
/// asserting different things under the same name.
fn assert_spent_refused_and_idle_succeeded<O>(
    spent: &Report<O>,
    spent_want: Disposition,
    idle: &Report<O>,
    idle_want: Disposition,
    shape: &str,
) {
    assert_eq!(
        spent.disposition(),
        spent_want,
        "the tenant that spent its own {shape} budget must be refused ({:?})",
        spent.error()
    );
    assert_eq!(
        idle.disposition(),
        idle_want,
        "the tenant that spent nothing {shape} must be unaffected ({:?})",
        idle.error()
    );
}
