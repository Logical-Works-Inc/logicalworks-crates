//! The flood family: the live supervisor's flood round completes inside a declared bound on both queue arms.

use super::*;
use lgwks_bot::rt::supervise::{SpawnRefused, Supervisor};

/// The pools and bounds the flood-round simulation runs at.
///
/// Four permits with a per-tenant ceiling of two is the smallest world in which
/// one tenant can be at its ceiling while another still has room, which is the
/// whole shape of the question. Small on purpose: the simulation exists to reach
/// a wedged state in milliseconds rather than in minutes, and a bound that takes
/// minutes to trip is a bound nobody will keep.
const FLOOD_POOL: usize = 4;
/// The per-tenant ceiling both arms run under.
const FLOOD_CEILING: usize = 2;
/// The per-tenant queue bound of the arm that refuses rather than parks.
const FLOOD_REFUSING_QUEUE: usize = 0;
/// The per-tenant queue bound of the arm that parks.
const FLOOD_PARKING_QUEUE: usize = 1;
/// How long one submission may wait before the run is a failure.
///
/// A bound, not a budget: the work under test completes in microseconds, so this
/// is how long a *failure* to complete is allowed to look before it is named. A
/// submission that would never resolve is a wedged admission, not a slow one.
const FLOOD_BOUND: std::time::Duration = std::time::Duration::from_millis(250);
/// How many rounds one seed drives.
const FLOOD_ROUNDS: u32 = 16;
/// How many times a body hands the executor back before it returns.
///
/// The bodies complete on their own, and that is the load-bearing property of
/// this simulation: **the work that returns a permit must not be released by the
/// same caller that is waiting for it.** A caller that owns both is a caller that
/// parks forever, because parked it opens nothing. See
/// [`flood_round_structure`].
const BODY_YIELDS: u32 = 4;

/// One arm of the sweep: how the tenants' queues are bounded, and so whether a
/// submission parks or is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arm {
    /// The per-tenant queue is bounded but non-empty, so a contended arrival
    /// **parks** and waits for a permit some other admitted task must return.
    Parks,
    /// The per-tenant queue is zero, so a contended arrival is **refused** and a
    /// refusal waits for nothing.
    Refuses,
}

impl Arm {
    /// The per-tenant queue bound this arm declares.
    fn queue(self) -> usize {
        match self {
            Self::Parks => FLOOD_PARKING_QUEUE,
            Self::Refuses => FLOOD_REFUSING_QUEUE,
        }
    }

    /// A name for the arm, for a failure that has to say which one wedged.
    fn label(self) -> &'static str {
        match self {
            Self::Parks => "parking",
            Self::Refuses => "refusing",
        }
    }
}

/// A body that returns its permit without waiting for anybody.
///
/// Bounded by a count rather than a clock, so the simulation's timing comes from
/// nothing but how fast the host is -- and a body that never returns would be a
/// hang in every harness that used one.
async fn self_completing_body() {
    for _ in 0..BODY_YIELDS {
        lgwks_bot::rt::task::yield_now().await;
    }
}

/// Drive the live supervisor through the flood round structure for one seed.
///
/// The structure is the one a noisy-neighbour harness needs: a tenant is driven
/// to its ceiling, and another tenant submits into the permits the first returns.
/// **Every** submission is awaited inside [`FLOOD_BOUND`], so a state in which one
/// would never resolve is a failure naming the arm, the round and the slot, not a
/// hung suite. That bound is the point of this family: the first version of this
/// structure wedged a run for twelve minutes with every worker parked, nothing
/// runnable and no timer pending, and reached the same state in 250 ms once the
/// bound existed.
///
/// **The rule the structure has to obey.** A contended submission parks, and a
/// parked submission can only be resolved by a permit some *other* admitted task
/// returns. So the work that returns permits must complete on its own. A harness
/// whose bodies park on a gate the submitting loop opens cannot do that: the loop
/// opens the gate only between submissions, so a submission that parks leaves
/// nothing to open it. The bodies here complete by themselves, and the live
/// flood test uses the refusing arm instead, where no submission parks at all.
///
/// The seed chooses which tenant takes each slot, so a band covers interleavings
/// rather than one hand-picked order.
fn flood_round_structure(seed: u64) -> Result<(), Box<dyn Error>> {
    for arm in [Arm::Refuses, Arm::Parks] {
        flood_round_structure_in(seed, arm)?;
    }
    Ok(())
}

/// [`flood_round_structure`] for one arm.
fn flood_round_structure_in(seed: u64, arm: Arm) -> Result<(), Box<dyn Error>> {
    let mut rng = Rng::new(seed ^ 0x7100_0d00_0000_0001);
    let loud = Tenant::new("flood-loud")?;
    let quiet = Tenant::new("flood-quiet")?;
    let runtime = lgwks_bot::rt::runtime::Runtime::new()?;

    let driven = runtime.block_on(async {
        let mut supervisor =
            Supervisor::with_tenancy(FLOOD_POOL, TenancyPolicy::new(FLOOD_CEILING, arm.queue()));
        for round in 1..=FLOOD_ROUNDS {
            for slot in 0..FLOOD_CEILING {
                let quiet_slot = rng.below(10) < 3;
                let tenant = if quiet_slot { &quiet } else { &loud };
                let label = if quiet_slot { "quiet" } else { "loud" };
                let submitted = lgwks_bot::rt::time::timeout(
                    FLOOD_BOUND,
                    supervisor.spawn_for(tenant, |_t| async {
                        self_completing_body().await;
                    }),
                )
                .await;
                match submitted {
                    Ok(Ok(())) => {}
                    // A refusal is an ordinary answer under load and waits for
                    // nothing, so it is accepted rather than failed.
                    Ok(Err(SpawnRefused::TenantAtCapacity { .. }))
                    | Ok(Err(SpawnRefused::SupervisorQueueFull { .. })) => {}
                    Ok(Err(SpawnRefused::Cancelled)) => {
                        let refusal = Err(format!(
                            "seed {seed}: the {} arm refused a submission as cancelled at \
                             round {round} slot {slot}",
                            arm.label()
                        ));
                        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "flood: a submission was refused as cancelled");
                        return refusal;
                    }
                    _ => {
                        let loud_state = supervisor.tenant_capacity(&loud);
                        let quiet_state = supervisor.tenant_capacity(&quiet);
                        let free = supervisor.snapshot().free();
                        let refusal = Err(format!(
                            "seed {seed}: the {} arm wedged -- round {round} slot {slot} on \
                             the {label} tenant never resolved within {FLOOD_BOUND:?}. A \
                             contended submission parked and no admitted task returned its \
                             permit to it. State at the wedge: {free} permit(s) free, loud \
                             {} in flight and {} queued, quiet {} in flight and {} queued.",
                            arm.label(),
                            loud_state.0,
                            loud_state.1,
                            quiet_state.0,
                            quiet_state.1,
                        ));
                        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "flood: a contended submission wedged");
                        return refusal;
                    }
                }
            }
        }
        let _report = supervisor.shutdown().await;
        Ok::<(), String>(())
    });
    driven.map_err(|why| -> Box<dyn Error> { why.into() })
}

/// Every flood round completes inside its bound, at every seed in the band.
///
/// The regression this pins is a wedged admission, not a slow one: a parked
/// submission that no completed task ever resolves. A bound turns that into a
/// named failure in a quarter of a second instead of a hung suite.
fn a_flood_round_completes_within_its_bound(band: sim::Band) -> TestResult {
    for seed in band.seeds() {
        flood_round_structure(seed)?;
    }
    Ok(())
}

band_family::band_family! {
    a_flood_round_completes_within_its_bound_band_00 => a_flood_round_completes_within_its_bound, 7;
}
