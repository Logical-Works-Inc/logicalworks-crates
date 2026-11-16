//! The deadline family: an overrun budget is the request's own recorded verdict, and a repeat reattaches to it.

use super::*;

/// The shortest budget a deadline sweep draws, in milliseconds.
///
/// Not zero: a budget of zero would refuse the run *before* its first step
/// recorded, which is a different observation — a run that never started cannot
/// show that a request's recorded work survives its own overrun. The floor keeps
/// every drawn request in the state this family is about: progress committed,
/// then the deadline ends it.
///
/// One millisecond is safe because the budget is measured on the host's virtual
/// clock, which moves only when the body moves it: the first step's `fsync`-ed
/// records take no logical time however long the disk takes, so even the
/// tightest budget is reached only by the overrun the seed drew.
const MIN_DEADLINE_MILLIS: u32 = 1;

/// The longest budget a deadline sweep draws, in milliseconds: the crate's own
/// ceiling ([`lgwks_bot::task::MAX_TASK_DEADLINE`], a day).
///
/// On the wall clock this family was held to 1.5 s, and a band of it spent
/// twenty seconds waiting. On the virtual clock a day costs what a millisecond
/// does, so the sweep covers every budget a host accepts.
const MAX_DEADLINE_MILLIS: u32 = 86_400_000;

/// The longest a drawn overrun goes past its budget, in milliseconds.
///
/// Zero included: the clock reaching the deadline *exactly* is the boundary,
/// and it must already be an overrun.
const MAX_OVERRUN_MILLIS: u32 = 60_000;

/// How many requests one seeded deadline sweep drives.
const MAX_DEADLINE_REQUESTS: u32 = 4;

/// One request of the deadline sweep: overrun a drawn budget, then reattach to the recorded verdict.
fn deadline_request(
    sim_run: &mut sim::Sim,
    scratch: &Scratch,
    runs: &FirstStepRuns,
    index: u32,
    reached_terminal: &mut u32,
) -> TestResult {
    let tenant = tenant_for(index)?;
    let millis = sim_run
        .rng()
        .between(MIN_DEADLINE_MILLIS, MAX_DEADLINE_MILLIS);
    let past = sim_run.rng().between(0, MAX_OVERRUN_MILLIS);
    let payload = sim_run.rng().between(0, 1000);
    let key = RequestKey::new(&format!("deadline-{tenant}-{index}"))?;
    let budget = std::time::Duration::from_millis(u64::from(millis));
    let overrun = budget.saturating_add(std::time::Duration::from_millis(u64::from(past)));

    // A fresh host per request, because the declared budget is part of what
    // the request is and a host carries one budget for its whole life. Its
    // clock is virtual and starts at zero, and only the body moves it: by the
    // drawn budget plus the drawn overrun, once its first step has recorded.
    let clock = lgwks_bot::clock::Clock::virtual_at(std::time::Duration::ZERO);
    let host = host_with_deadline_on(tenant, scratch.path(), budget, clock.clone())?;
    let work = overrunning_task_on(runs.clone(), clock, overrun)?;

    let first = lgwks_bot::script::block_on(host.submit(&key, &work, payload))?;
    let disposition = first
        .report()
        .map(lgwks_bot::task::Report::disposition)
        .ok_or("a first submission carries a report")?;
    assert_eq!(
        disposition,
        Disposition::DeadlineExceeded,
        "a body waiting on something that never arrives outruns its declared budget \
         ({millis}ms, overrun by {past}ms, for {tenant}/{index}); got {disposition}"
    );
    sim_run
        .trace
        .record_number("budget-millis", u64::from(millis));
    sim_run
        .trace
        .record_number("overrun-millis", u64::from(past));
    sim_run.trace.record(&format!("overran {disposition}"));

    // A fresh host over the same store file — a new *host*, which is what a
    // restart is, rather than a second writer, which the store's chain fence
    // refuses. It carries the *same* declared budget, because the budget is
    // part of what this request is: a repeat under a longer budget is a
    // different declaration, and letting the crate default in here would let
    // the body overrun for thirty seconds and report a fresh `Executed`
    // rather than reattaching to the recorded verdict. That is the defect
    // this assertion exists to catch, not a nuisance to work around.
    let reopened = host_with_deadline_on(
        tenant,
        scratch.path(),
        budget,
        lgwks_bot::clock::Clock::virtual_at(std::time::Duration::ZERO),
    )?;
    let seen = lgwks_bot::script::block_on(reopened.submit(&key, &work, payload))?;
    let recorded = match seen {
        Submission::Reattached(report) => report.disposition(),
        other => {
            let refusal = Err(format!(
                "a request that overran its budget reported {other:?} on a repeat; the \
             deadline is the request's own verdict and must be recorded"
            )
            .into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "deadline_scenario: returning an error to the caller");
            return refusal;
        }
    };
    assert_eq!(
        recorded,
        Disposition::DeadlineExceeded,
        "the reattached report carries the recorded deadline"
    );
    *reached_terminal = reached_terminal.saturating_add(1);
    sim_run.trace.record(&format!("reattached {recorded}"));
    Ok(())
}

/// A seeded deadline sweep: a drawn budget per request, over both tenants
/// sharing one store file.
///
/// The claim is the one a host stop does *not* make, and the two families sit
/// together so neither can be true by accident. A deadline is the run reaching
/// the budget its own declaration fixed: a fact about the request, not about
/// this host, so it is recorded and a later client reattaches to it. Every drawn
/// request must therefore show all four of:
///
/// 1. the submission reports `DeadlineExceeded`;
/// 2. the key's `@terminal` **is** recorded — proven by the reattach below
///    rather than by reading the store, because the reattach is the door a
///    caller actually comes through;
/// 3. a later submission of the same key reattaches carrying
///    `DeadlineExceeded`, and the body did not run a second time;
/// 4. the recorded first step's effect ran exactly once across the overrun, the
///    reattach and the sweep's other requests.
fn deadline_scenario(sim_run: &mut sim::Sim) -> TestResult {
    let requests = sim_run.rng().between(1, MAX_DEADLINE_REQUESTS);
    let scratch = Scratch::new("sim-req-deadline")?;
    // Each host opens the shared store file itself, so the two tenants meet on
    // one file without either holding a second writer to it.
    // Every request shares one counter, so "each recorded step ran once" is one
    // number for the whole sweep rather than a per-request tally a defect could
    // hide inside.
    let runs = FirstStepRuns::new();
    let mut reached_terminal = 0_u32;

    for index in 0..requests {
        deadline_request(sim_run, &scratch, &runs, index, &mut reached_terminal)?;
    }

    // One count for the whole sweep, and exactly one effect per request: a
    // reattach that re-entered a body would show up here as an extra.
    assert_eq!(
        runs.count(),
        requests,
        "each request's recorded step ran exactly once across its overrun and its reattach"
    );
    assert_eq!(
        reached_terminal, requests,
        "every drawn request recorded its deadline, so none is left in flight forever"
    );
    sim_run.trace.record_number("requests", u64::from(requests));
    sim_run
        .trace
        .record_number("recorded", u64::from(reached_terminal));
    sim_run
        .trace
        .record_number("effects", u64::from(runs.count()));
    Ok(())
}

/// A seeded sweep of the deadline family.
fn expired_deadlines_are_recorded(band: Band) -> TestResult {
    sim::assert_replays(band, deadline_scenario)?;
    Ok(())
}

band_family::band_family! {
    expired_deadlines_are_recorded_band_16 => expired_deadlines_are_recorded, 16;
    expired_deadlines_are_recorded_band_17 => expired_deadlines_are_recorded, 17;
    expired_deadlines_are_recorded_band_18 => expired_deadlines_are_recorded, 18;
    expired_deadlines_are_recorded_band_19 => expired_deadlines_are_recorded, 19;
    expired_deadlines_are_recorded_band_20 => expired_deadlines_are_recorded, 20;
    expired_deadlines_are_recorded_band_21 => expired_deadlines_are_recorded, 21;
    expired_deadlines_are_recorded_band_22 => expired_deadlines_are_recorded, 22;
    expired_deadlines_are_recorded_band_23 => expired_deadlines_are_recorded, 23;
}
