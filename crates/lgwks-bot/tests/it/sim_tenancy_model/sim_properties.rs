//! The properties of the tenancy model, one seeded sweep each: every arm drives
//! the shipped round against the reference world of the parent module.

use std::num::NonZeroU32;

use lgwks_bot::rt::tenancy::{
    MAX_QUEUE_PER_TENANT, MAX_TENANT_WEIGHT, MAX_TOTAL_QUEUE, TenancyError, TenancyPolicy,
};
use lgwks_bot::script::Tenant;

use super::{Property, Shape, TestResult, ensure, sweep_all};
use crate::sim;

/// No tenant is ever charged more admissions than its ceiling.
#[test]
fn in_flight_never_passes_the_tenant_ceiling() -> TestResult {
    sweep_all(Shape::MIXED, Property::Ceiling)
}

/// No tenant ever holds more live waiters than its queue bound.
#[test]
fn live_waiters_never_pass_the_tenant_queue_bound() -> TestResult {
    sweep_all(Shape::MIXED, Property::TenantQueue)
}

/// The round never retains more waiters than the supervisor's bound, abandoned
/// ones included, under a supervisor bound tighter than any tenant's.
#[test]
fn retained_waiters_never_pass_the_supervisor_bound() -> TestResult {
    sweep_all(Shape::SUPERVISOR_BOUND, Property::SupervisorQueue)
}

/// Every permit is in the pool or charged to exactly one running admission,
/// and the round's charges agree with the admissions that are running.
#[test]
fn permits_are_conserved_between_the_pool_and_the_round() -> TestResult {
    sweep_all(Shape::LATE, Property::Conservation)
}

/// A permit is handed back to the pool only when no live waiter is under its
/// tenant's ceiling: the round never idles with servable work.
#[test]
fn the_round_idles_only_when_no_live_waiter_has_room() -> TestResult {
    sweep_all(Shape::MIXED, Property::WorkConserving)
}

/// A tenant's live waiters are granted oldest first, abandoned ones skipped.
#[test]
fn each_tenant_is_served_in_arrival_order() -> TestResult {
    sweep_all(Shape::MIXED, Property::Fifo)
}

/// Once every admission has ended, every waiter whose owner stayed was granted
/// exactly once.
#[test]
fn every_live_waiter_is_served_exactly_once() -> TestResult {
    sweep_all(Shape::MIXED, Property::ExactlyOnce)
}

/// No grant ever names a waiter whose owner left before the round chose it.
#[test]
fn an_abandoned_waiter_is_never_granted() -> TestResult {
    sweep_all(Shape::MIXED, Property::NoDeadGrant)
}

/// A non-blocking arrival that is refused changes no count: no queue slot, no
/// tenant entry, no permit.
#[test]
fn a_contended_try_arrival_retains_nothing() -> TestResult {
    sweep_all(Shape::TRY_HEAVY, Property::TryRetainsNothing)
}

/// Nobody, through either door, is admitted from the pool while another
/// tenant is waiting.
#[test]
fn an_immediate_admission_never_jumps_a_waiting_tenant() -> TestResult {
    sweep_all(Shape::TRY_HEAVY, Property::NoJump)
}

/// Each refusal arm fires only when its bound is reached, and names the bound
/// the policy declared.
#[test]
fn a_refusal_names_the_bound_it_reached() -> TestResult {
    sweep_all(Shape::SUPERVISOR_BOUND, Property::RefusalNamesBound)?;
    sweep_all(Shape::MIXED, Property::RefusalNamesBound)
}

/// Once every admission has ended, the round retains no waiter and no tenant
/// with a queue, and every permit is back in the pool.
#[test]
fn a_drained_round_retains_nothing() -> TestResult {
    sweep_all(Shape::LATE, Property::DrainedIsEmpty)
}

/// `queued_of` is exactly the number of a tenant's owners still waiting, after
/// every act.
#[test]
fn the_live_count_matches_the_owners_still_waiting() -> TestResult {
    sweep_all(Shape::MIXED, Property::LiveCount)
}

/// An owner walking away between the round's choice and the delivery leaves
/// every tenant's live count exact, whether or not the tenant had other
/// abandoned waiters at the time.
#[test]
fn a_late_abandonment_leaves_the_counts_exact() -> TestResult {
    sweep_all(Shape::LATE, Property::LiveCount)
}

/// When `has_eligible` answers false, the grant it guards hands the permit
/// back rather than reaching a waiter.
#[test]
fn an_empty_ring_means_the_round_is_idle() -> TestResult {
    sweep_all(Shape::MIXED, Property::EmptyRingIdles)
}

/// The same seed produces the same trace twice, with every property checked.
#[test]
fn the_same_seed_replays_the_same_round() -> TestResult {
    sweep_all(Shape::LATE, Property::Every)?;
    sweep_all(Shape::MIXED, Property::Every)
}

/// Every seeded policy reads back the bounds and weights it enforces: a zero
/// ceiling reads as one, oversized bounds clamp to their ceilings, a weight
/// past its bound is refused naming both numbers, and an unnamed tenant
/// weighs one.
#[test]
fn declared_bounds_and_weights_read_back_as_enforced() -> TestResult {
    for band in sim::bands(sim::SEED_SPACE, sim::BANDS) {
        sim::assert_replays(band, |run| {
            let ceiling = usize::try_from(run.rng().below(64))?;
            let queue = usize::try_from(run.rng().below(u32::MAX))?;
            let total = usize::try_from(run.rng().below(u32::MAX))?;
            let policy = TenancyPolicy::new(ceiling, queue).with_queue_total(total);
            run.trace
                .record_number("ceiling", policy.per_tenant_limit());
            run.trace.record_number("queue", policy.queue_per_tenant());
            run.trace.record_number("total", policy.queue_total());
            let read_back = policy.per_tenant_limit() == ceiling.max(1)
                && policy.queue_per_tenant() == queue.min(MAX_QUEUE_PER_TENANT)
                && policy.queue_total() == total.min(MAX_TOTAL_QUEUE);
            ensure(Property::Every, Property::Every, read_back, || {
                format!("declared ({ceiling}, {queue}, {total}) did not read back as enforced")
            })?;
            let heavy = Tenant::new("heavy")?;
            let weight = run.rng().between(1, MAX_TENANT_WEIGHT.saturating_mul(2));
            let declared = NonZeroU32::new(weight).ok_or("a weight of at least one")?;
            run.trace.record_number("weight", weight);
            let honest = match policy.with_weight(&heavy, declared) {
                Ok(weighted) => {
                    weight <= MAX_TENANT_WEIGHT
                        && weighted.weight_of(&heavy) == declared
                        && weighted.weight_of(&Tenant::new("light")?) == NonZeroU32::MIN
                }
                Err(TenancyError::WeightTooHigh {
                    weight: named,
                    limit,
                }) => weight > MAX_TENANT_WEIGHT && named == weight && limit == MAX_TENANT_WEIGHT,
                Err(other) => return Err(format!("weight {weight} was refused: {other:?}").into()),
            };
            ensure(Property::Every, Property::Every, honest, || {
                format!("a weight of {weight} was not answered as the policy declares")
            })
        })?;
    }
    Ok(())
}
