//! The waiting-bound family: many tenants share one supervisor bound, and a declared bound is clamped to the ceiling.

use super::*;
use lgwks_bot::rt::tenancy::MAX_TOTAL_QUEUE;

/// What one waiting-bound sweep observed.
#[derive(Debug, Default)]
struct Sweep {
    /// Arrivals that queued rather than being admitted.
    queued: u64,
    /// Arrivals refused because their own tenant's queue was full.
    per_tenant_refused: u64,
    /// Arrivals refused because the supervisor's own bound was reached.
    supervisor_refused: u64,
    /// Waiters abandoned before being served.
    abandoned: u64,
    /// The largest retained total the sweep saw.
    widest: usize,
}

impl Sweep {
    /// Append this sweep to `trace`, in a fixed field order.
    fn record(&self, trace: &mut Trace) {
        trace.record_number("bound-queued", self.queued);
        trace.record_number("bound-per-tenant-refused", self.per_tenant_refused);
        trace.record_number("bound-supervisor-refused", self.supervisor_refused);
        trace.record_number("bound-abandoned", self.abandoned);
        trace.record_number("bound-widest", self.widest);
    }
}

/// Run one seeded sweep at `tenants` tenants against a waiting bound of
/// `queue_total`, and report what it saw.
///
/// One world shape rather than a second scenario type: the questions are the same
/// at every tenant count, and a family that only ran at the tier it was written
/// for would leave the other tiers untested.
fn sweep_bound(seed: u64, tenants: u32, queue_total: usize) -> Result<Sweep, Box<dyn Error>> {
    let mut rng = Rng::new(seed ^ 0x6a0a_6a0a_6a0a_6a0a);
    let mut trace = Trace::new();
    let mut observed = Sweep::default();
    let roster = roster(tenants)?;
    let mut named: Vec<Tenant> = Vec::with_capacity(roster);
    for index in 0..tenants {
        named.push(Tenant::new(&format!("bound-tenant-{index}"))?);
    }
    let mut core: DeficitRoundRobin<u64> = DeficitRoundRobin::new(
        TenancyPolicy::new(2, QUEUE_PER_TENANT).with_queue_total(queue_total),
    );
    let mut abandoned: BTreeSet<u64> = BTreeSet::new();
    let mut next_waiter = 0_u64;
    for _ in 0..ACTS {
        let index = self::roster(rng.below(tenants.max(1)))?;
        let Some(tenant) = named.get(index).cloned() else {
            trace.record("a-tenant-index-past-the-roster");
            continue;
        };
        match rng.below(10) {
            0..=6 => {
                let waiter = next_waiter;
                next_waiter = next_waiter.saturating_add(1);
                match core.arrive(&tenant, waiter, || None::<u64>) {
                    Arrival::Queued => observed.queued = observed.queued.saturating_add(1),
                    Arrival::Refused { .. } => {
                        observed.per_tenant_refused = observed.per_tenant_refused.saturating_add(1);
                    }
                    Arrival::SupervisorFull { limit } => {
                        observed.supervisor_refused = observed.supervisor_refused.saturating_add(1);
                        trace.record_number("supervisor-refused-at", limit);
                    }
                    _ => trace.record("an-arrival-was-admitted-with-a-spent-pool"),
                }
            }
            _ => {
                let span = u64::from(rng.below(8));
                let Some(waiter) = next_waiter.saturating_sub(1).checked_sub(span) else {
                    continue;
                };
                if abandoned.insert(waiter) {
                    observed.abandoned = observed.abandoned.saturating_add(1);
                }
                let mut is_live = |waiter: &u64| !abandoned.contains(waiter);
                core.note_abandoned(&tenant, &mut is_live);
            }
        }
        let retained = core.retained_total();
        observed.widest = observed.widest.max(retained);
        trace.record_number("retained", retained);
        trace.record_number("tenants-with-queues", core.tenants_with_queues());
        assert!(
            retained <= queue_total,
            "seed {seed}: {tenants} tenants retained {retained} waiters against a \
             supervisor bound of {queue_total}"
        );
    }
    observed.record(&mut trace);
    Ok(observed)
}

/// The waiting bound is the supervisor's, not one tenant's: many tenants share
/// one bound, the retained total never passes it, and the arrival that would pass
/// it is refused by the arm that names the supervisor rather than a tenant.
///
/// Run at the estate's declared tenant-provision tier as well as below it,
/// because the tier is the point: a bound that only holds at four tenants is not
/// a bound on a fleet.
fn the_supervisor_waiting_bound_is_global(band: sim::Band) -> TestResult {
    /// The tiers the sweep runs at. 5,000 is the estate's declared
    /// tenant-provision tier.
    const TIERS: [u32; 2] = [4, 5_000];
    /// The bound each tier runs at. Small enough that a tier's whole sweep stays
    /// under it, so the refusal is reached inside the run rather than at a total
    /// the run never reaches.
    const BOUNDS: [usize; 2] = [64, 256];

    for (tier_index, tenants) in TIERS.iter().copied().enumerate() {
        let Some(bound) = BOUNDS.get(tier_index).copied() else {
            let refusal = Err("a tier had no bound beside it".into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "the_supervisor_waiting_bound_is_global: returning an error to the caller");
            return refusal;
        };
        let mut widest = 0_usize;
        let mut refused = 0_u64;
        for seed in band.seeds() {
            let observed = sweep_bound(seed, tenants, bound)?;
            widest = widest.max(observed.widest);
            refused = refused.saturating_add(observed.supervisor_refused);
        }
        assert!(
            widest <= bound,
            "{tenants} tenants retained {widest} waiters against a bound of {bound}"
        );
        assert!(
            refused > 0,
            "no arrival was refused by the supervisor's own bound at {tenants} tenants, \
             so the bound was never reached and never refused anything"
        );
    }
    Ok(())
}

/// The declared default is the ceiling, and a larger declaration is clamped to
/// it, so no policy can author an unbounded supervisor.
#[test]
fn the_waiting_bound_is_declared_and_clamped() -> TestResult {
    let default = TenancyPolicy::new(1, 1);
    assert_eq!(
        default.queue_total(),
        MAX_TOTAL_QUEUE,
        "a policy that says nothing about the supervisor's queue gets the ceiling"
    );
    let clamped = TenancyPolicy::new(1, 1).with_queue_total(MAX_TOTAL_QUEUE.saturating_add(1));
    assert_eq!(
        clamped.queue_total(),
        MAX_TOTAL_QUEUE,
        "a declaration past the ceiling is clamped to it"
    );
    Ok(())
}

band_family::band_family! {
    the_supervisor_waiting_bound_is_global_band_00 => the_supervisor_waiting_bound_is_global, 8;
}
