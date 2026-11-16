//! The host-stop family: a drawn stop point never poisons a key, and the same seed replays to one trace hash.

use super::*;

/// Where in a request's life a host's stop arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopPoint {
    /// Before the run is admitted: the host is already stopping.
    BeforeAdmission,
    /// After the first durable step's record is committed.
    AfterFirstStep,
}

impl StopPoint {
    /// The name the trace records, so a replay compares the same word a reader
    /// sees.
    const fn label(self) -> &'static str {
        match self {
            Self::BeforeAdmission => "before-admission",
            Self::AfterFirstStep => "after-first-step",
        }
    }
}

/// How many requests one seeded stop sweep drives.
const MAX_STOP_REQUESTS: u32 = 3;

/// One request of the stop sweep: draw a host stop, reopen, settle, and reattach.
fn stop_request(
    sim_run: &mut sim::Sim,
    store: &RunStore,
    index: u32,
    reaches_terminal: &mut u32,
) -> TestResult {
    let tenant = tenant_for(index)?;
    // A fresh host per request, over one shared store handle: a *stopped*
    // host stays stopped, so a host that has already been used to draw a
    // `BeforeAdmission` stop could not drive anything afterwards. Two store
    // handles over one file would be two writers, which the store's own chain
    // fence refuses, so the file is opened once and the hosts share it.
    let host = host_on(tenant, store.clone())?;
    let point = if sim_run.rng().chance(500) {
        StopPoint::BeforeAdmission
    } else {
        StopPoint::AfterFirstStep
    };
    let payload = sim_run.rng().between(0, 1000);
    let key = RequestKey::new(&format!("stop-{tenant}-{index}"))?;
    let runs = FirstStepRuns::new();
    let recorded = Arc::new(AtomicBool::new(false));
    // The first run's body waits for a release that never comes, so the
    // host's stop is what ends it. The settling run uses a body that goes
    // on to the second step.
    let parked = two_step_task(
        Arc::clone(&recorded),
        Arc::new(AtomicBool::new(false)),
        runs.clone(),
    )?;
    let released = two_step_task(
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(true)),
        runs.clone(),
    )?;

    let first = match point {
        StopPoint::BeforeAdmission => {
            host.cancel();
            lgwks_bot::script::block_on(host.submit(&key, &parked, payload))?
        }
        StopPoint::AfterFirstStep => stop_mid_run(&host, &key, &parked, payload, &recorded)?,
    };
    let stopped = first
        .report()
        .map(lgwks_bot::task::Report::disposition)
        .ok_or("a first submission carries a report")?;
    assert!(
        matches!(stopped, Disposition::Cancelled | Disposition::Refused),
        "the drawn stop point must produce a host stop, drew {stopped} at {tenant}/{index}"
    );
    sim_run
        .trace
        .record(&format!("stopped {point:?} {stopped}"));
    sim_run
        .trace
        .record(&format!("stop-point {}", point.label()));

    // A fresh host over the same store file — a new *host*, which is what a
    // restart is, rather than a second writer, which the store's own chain
    // fence refuses. This is the whole claim: a host stop recorded as the
    // verdict would reattach here, forever.
    let reopened = host_on(tenant, store.clone())?;
    let seen = lgwks_bot::script::block_on(reopened.submit(&key, &released, payload))?;
    let seen_disposition = match seen {
        Submission::Reattached(report) => {
            let refusal = Err(format!(
                "a request stopped at {point:?} reattached to {:?}; a host stop is not the \
             request's outcome",
                report.disposition()
            )
            .into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "stop_scenario: returning an error to the caller");
            return refusal;
        }
        Submission::InFlight(ref in_flight) => {
            sim_run
                .trace
                .record_number("in-flight-records", u64::try_from(in_flight.records())?);
            None
        }
        Submission::Executed(ref report) => Some(report.disposition()),
        // `Submission` is `#[non_exhaustive]`: a new arm is a compile-time
        // prompt here, and a request whose settlement this family has not
        // been taught to read is not silently treated as unsettled.
        other => {
            let refusal = Err(format!(
                "a request this family does not know how to read came back as {other:?}"
            )
            .into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "stop_scenario: returning an error to the caller");
            return refusal;
        }
    };
    sim_run.trace.record(&format!("after-stop {seen:?}"));
    assert_eq!(
        seen_disposition, None,
        "a stopped request is unsettled: the reopen must report it in flight, not settle it"
    );

    // Settle it on a host that can admit it, then reattach and read the one
    // recorded terminal.
    let run = seen.run_id().ok_or("a submission names a run")?;
    let settled = lgwks_bot::script::block_on(reopened.resume(run, &released, payload));
    assert_eq!(
        settled.disposition(),
        Disposition::Succeeded,
        "the resumed run reaches the request's own verdict"
    );
    let repeat = lgwks_bot::script::block_on(reopened.submit(&key, &released, payload))?;
    let reattached = match repeat {
        Submission::Reattached(report) => report.disposition(),
        other => {
            let refusal = Err(format!(
                "a request driven to a verdict must record exactly one terminal; a later \
             submission saw {other:?}"
            )
            .into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "stop_scenario: returning an error to the caller");
            return refusal;
        }
    };
    assert_eq!(
        reattached,
        Disposition::Succeeded,
        "the recorded terminal is the request's own verdict"
    );
    assert_eq!(
        runs.count(),
        1,
        "the first durable step's effect ran once across the stop, the settle and the reattach"
    );
    *reaches_terminal = reaches_terminal.saturating_add(1);
    sim_run
        .trace
        .record_number("effects", u64::from(runs.count()));
    Ok(())
}

/// A seeded stop sweep: a drawn stop point and disposition per request, over both
/// tenants sharing one store file.
///
/// Three facts must hold for every drawn request, and the seed decides which
/// case each one is rather than a scenario per case:
///
/// 1. **No key ever reattaches to a host stop.** A `Cancelled` or `Refused`
///    reattach is the defect this family exists to catch: it is a key whose only
///    recorded outcome is the host declining to finish it, which nothing can
///    ever complete.
/// 2. **Every request reaches exactly one recorded terminal** once driven on a
///    healthy host. Counted by reattaching and reading the disposition, because
///    a request whose terminal was never recorded reports `InFlight` forever.
/// 3. **Each recorded step's body ran once.** The counter is on the step's
///    *effect*, not on body entries, so a replayed record is a pass and a
///    re-run effect is a failure.
fn stop_scenario(sim_run: &mut sim::Sim) -> TestResult {
    let requests = sim_run.rng().between(1, MAX_STOP_REQUESTS);
    let scratch = Scratch::new("sim-req-stop")?;
    // One store handle, cloned into both tenants: two handles over one file
    // would be two writers, which the store's own length fence refuses.
    let store = RunStore::open(scratch.path().join("shared.runstore"))?;
    // How many requests reached a recorded terminal, so the trace can show the
    // sweep was not vacuous at a tier that drew no requests.
    let mut reaches_terminal = 0_u32;

    for index in 0..requests {
        stop_request(sim_run, &store, index, &mut reaches_terminal)?;
    }
    sim_run.trace.record_number("requests", u64::from(requests));
    sim_run
        .trace
        .record_number("settled", u64::from(reaches_terminal));
    Ok(())
}

/// A seeded sweep of the stop family.
fn host_stops_never_poison_a_key(band: Band) -> TestResult {
    sim::assert_replays(band, stop_scenario)?;
    Ok(())
}

/// The same seed produces the same trace hash, twice, over the stop family.
///
/// The stop family is the one whose whole claim is a *replay receipt*: a key
/// that reattaches to a host stop is exactly the kind of defect a single run
/// can miss, and a trace that hashes differently between two runs of one seed
/// cannot be compared against anything.
fn same_seed_replays_host_stops(band: Band) -> TestResult {
    sim::assert_replays(band, stop_scenario)?;
    Ok(())
}

band_family::band_family! {
    host_stops_never_poison_a_key_band_00 => host_stops_never_poison_a_key, 0;
    host_stops_never_poison_a_key_band_01 => host_stops_never_poison_a_key, 1;
    host_stops_never_poison_a_key_band_02 => host_stops_never_poison_a_key, 2;
    host_stops_never_poison_a_key_band_03 => host_stops_never_poison_a_key, 3;
    same_seed_replays_host_stops_band_00 => same_seed_replays_host_stops, 0;
    same_seed_replays_host_stops_band_01 => same_seed_replays_host_stops, 1;
    same_seed_replays_host_stops_band_02 => same_seed_replays_host_stops, 2;
    same_seed_replays_host_stops_band_03 => same_seed_replays_host_stops, 3;
}
