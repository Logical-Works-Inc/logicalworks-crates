//! The fairness family: backlogged tenants take shares in proportion to their weights, and a weighted round replays.

use super::*;
use lgwks_bot::rt::tenancy::{GrantOutcome, MAX_TOTAL_QUEUE};
use std::num::NonZeroU32;

/// The tenant tiers the fairness sweep runs at.
///
/// Two is the smallest world in which a share can differ from another's, 100 and
/// 1,000 are the tiers a real fleet passes through, and 5,000 is the estate's
/// declared tenant-provision tier.
const FAIRNESS_TIERS: [u32; 4] = [2, 100, 1_000, 5_000];

/// The per-tenant ceiling the fairness sweep declares.
const FAIRNESS_CEILING: usize = 64;

/// A whole number of rounds, at least `minimum` of them.
///
/// A share is only readable at a round boundary: inside a round the algorithm is
/// *supposed* to have handed a weight-2 tenant two permits in a row, so a
/// mid-round reading is the algorithm working rather than failing. Rounding the
/// sweep up to a whole number of rounds is therefore what makes the share a
/// meaningful measurement instead of a snapshot of a turn in progress.
fn grants_for(tenants: u32, total_weight: u64, minimum: u64) -> u64 {
    let wanted = u64::from(tenants).saturating_mul(4);
    let rounds = wanted.div_ceil(total_weight).max(minimum);
    rounds.saturating_mul(total_weight)
}

/// One weighted round's outcome.
#[derive(Debug)]
struct Fairness {
    /// The weight every tenant drew.
    weights: Vec<u64>,
    /// The grants every tenant received, in tenant order.
    grants: Vec<u64>,
    /// The sum of every tenant's weight, which is one round of grants.
    total_weight: u64,
    /// The grants handed out, a whole number of rounds.
    handed_out: u64,
    /// The largest per-tenant deviation from its ideal share, in shares of one
    /// grant: the tightest the algorithm ever came to a proportional split.
    worst_deviation: u64,
    /// The longest a backlogged tenant waited between grants, measured in grants
    /// handed out.
    longest_wait: u64,
}

impl Fairness {
    /// Append this round to `trace`, in tenant order.
    fn record(&self, trace: &mut Trace, tenants: u32) {
        trace.record_number("fair-tenants", u64::from(tenants));
        trace.record_number("fair-total-weight", self.total_weight);
        trace.record_number("fair-handed-out", self.handed_out);
        trace.record_number("fair-worst-deviation", self.worst_deviation);
        trace.record_number("fair-longest-wait", self.longest_wait);
        for (weight, grants) in self.weights.iter().zip(&self.grants) {
            trace.record_number("fair-weight", *weight);
            trace.record_number("fair-grants", *grants);
        }
    }
}

/// Waiters parked per tenant.
///
/// Deep enough that a tier's whole sweep cannot drain any of them -- a drained
/// tenant stops being a claimant for a share -- and shallow enough that the whole
/// roster's backlog fits under the supervisor's own waiting bound. The second
/// half is not a shortcut: the sweep has to live inside that bound rather than be
/// exempt from it, and at the largest tier a 24-deep roster reaches it.
fn depth_for(tenants: u32) -> Result<usize, Box<dyn Error>> {
    let roster = roster(tenants)?.max(1);
    // Floor, not a rounding up: the depth has to *fit* the supervisor's bound,
    // and a ceiling here would put the roster's own backlog past the very bound
    // it is living inside.
    match MAX_TOTAL_QUEUE.checked_div(roster) {
        Some(room) => Ok(room.min(24)),
        None => Err(format!(
            "a roster of {roster} has no place under a supervisor bound of {MAX_TOTAL_QUEUE}"
        )
        .into()),
    }
}

/// One weighted round at `tenants` tenants, with the fairness invariants checked
/// at every grant.
///
/// Every tenant is continuously backlogged -- each holds a deep queue of live
/// waiters from the first act -- because a share is only defined for a tenant
/// that had work waiting when every permit freed. A tenant whose queue empties
/// has nothing to be denied, and a tenant parked at its ceiling is not on the
/// ring at all, so the admission is released as it is granted and the ceiling is
/// above what the sweep can reach.
///
/// Two invariants, both O(1) and both checked after **every** grant:
///
/// 1. **No starvation.** A backlogged tenant is served within one round -- the
///    total weight of the world's weights -- of the grants handed out since it
///    was last served. That is the bound deficit round robin is defined by: a
///    complete round serves every backlogged tenant at least once.
/// 2. **Bounded deviation, at every round boundary.** A tenant's cumulative
///    grants stay within one round of its weight's ideal share. Checked for
///    every tenant at each boundary rather than for one tenant at every grant,
///    because inside a round the deviation is *supposed* to reach a weight's
///    worth of grants: that is what a weight means. Reading it at a boundary is
///    what makes it a fairness measurement.
fn fair_round(tenants: u32, seed: u64) -> Result<Fairness, Box<dyn Error>> {
    let mut rng = Rng::new(seed ^ 0xfa11_0fa1_1fa1_1fa1);
    let roster = roster(tenants)?;
    let mut weights: Vec<u64> = Vec::with_capacity(roster);
    let mut named: Vec<Tenant> = Vec::with_capacity(roster);
    let depth = depth_for(tenants)?;
    let backlog = roster.saturating_mul(depth).saturating_add(1);
    let mut policy = TenancyPolicy::new(FAIRNESS_CEILING, depth).with_queue_total(backlog);
    for index in 0..tenants {
        let weight = u64::from(rng.between(1, 4));
        let Ok(declared) = u32::try_from(weight) else {
            let refusal = Err("a weight drawn between one and four is not a u32".into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fair_round: returning an error to the caller");
            return refusal;
        };
        let Some(nonzero) = NonZeroU32::new(declared) else {
            let refusal = Err("a weight drawn between one and four is never zero".into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fair_round: returning an error to the caller");
            return refusal;
        };
        let tenant = Tenant::new(&format!("fair-{index}"))?;
        policy = policy.with_weight(&tenant, nonzero)?;
        named.push(tenant);
        weights.push(weight);
    }
    let total_weight = weights.iter().copied().sum();
    let mut core: DeficitRoundRobin<u64> = DeficitRoundRobin::new(policy);
    // Backlog every tenant deeply enough that a tier's whole sweep cannot drain
    // any of them.
    let mut next_waiter = 0_u64;
    for tenant in &named {
        for _ in 0..depth {
            let arrival = core.arrive(tenant, next_waiter, || None::<u64>);
            match arrival {
                Arrival::Queued => {}
                Arrival::Refused { limit } => {
                    let refusal = Err(format!(
                        "at tier {tenants} a backlogged tenant hit its own queue bound \
                         of {limit} on its {next_waiter}th arrival"
                    )
                    .into());
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fair_round: returning an error to the caller");
                    return refusal;
                }
                Arrival::Immediate(_) => {
                    let refusal =
                        Err("a backlogged tenant was admitted against a spent pool".into());
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fair_round: returning an error to the caller");
                    return refusal;
                }
                Arrival::SupervisorFull { limit } => {
                    let refusal = Err(format!(
                        "at tier {tenants} a backlogged tenant hit the supervisor's own \
                         bound of {limit} on its {next_waiter}th arrival"
                    )
                    .into());
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fair_round: returning an error to the caller");
                    return refusal;
                }
                _ => {
                    let refusal = Err("an unknown arrival outcome".into());
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fair_round: returning an error to the caller");
                    return refusal;
                }
            }
            next_waiter = next_waiter.saturating_add(1);
        }
    }

    let mut grants: Vec<u64> = vec![0; named.len()];
    // The last grant count at which each tenant was served, so a wait is a
    // subtraction rather than a per-tenant counter advanced on every grant.
    let mut last_served: Vec<u64> = vec![0; named.len()];
    let mut handed_out = 0_u64;
    let mut worst_deviation = 0_u64;
    let mut longest_wait = 0_u64;
    let target = grants_for(tenants, total_weight, 2);
    while handed_out < target {
        let mut is_live = |_waiter: &u64| true;
        let outcome = core.grant(handed_out, &mut is_live);
        let GrantOutcome::Granted(grant) = outcome else {
            let refusal = Err(format!(
                "a continuously backlogged world of {tenants} tenants ran out of waiters \
                 after {handed_out} grants"
            )
            .into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fair_round: returning an error to the caller");
            return refusal;
        };
        let Some(index) = named
            .iter()
            .position(|candidate| *candidate == grant.tenant)
        else {
            let refusal = Err("the round granted to a tenant this sweep never built".into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fair_round: returning an error to the caller");
            return refusal;
        };
        // The admission the grant took is released immediately: the question is
        // the order, and a tenant held at its ceiling would drop off the ring and
        // turn a fairness question into a queue-length one.
        core.note_release(&grant.tenant);
        let Some(previous) = last_served.get(index).copied() else {
            let refusal = Err("the grant named a tenant this sweep never sized a tally for".into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fair_round: returning an error to the caller");
            return refusal;
        };
        let waited = handed_out.saturating_sub(previous);
        longest_wait = longest_wait.max(waited);
        assert!(
            waited <= total_weight,
            "at tier {tenants} a backlogged tenant waited {waited} grants for one, \
             against a round of {total_weight}"
        );
        handed_out = handed_out.saturating_add(1);
        if let Some(slot) = grants.get_mut(index) {
            *slot = slot.saturating_add(1);
        }
        if let Some(slot) = last_served.get_mut(index) {
            *slot = handed_out;
        }
        // Invariant 2, at every round boundary and over every tenant. The last
        // boundary is the sweep's end, so the final share is read at one too.
        if handed_out.is_multiple_of(total_weight) || handed_out == target {
            for (position, count) in grants.iter().copied().enumerate() {
                let Some(weight) = weights.get(position).copied() else {
                    let refusal =
                        Err("the round named a tenant this sweep never drew a weight for".into());
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "fair_round: returning an error to the caller");
                    return refusal;
                };
                let above = count
                    .saturating_mul(total_weight)
                    .saturating_sub(weight.saturating_mul(handed_out));
                let below = weight
                    .saturating_mul(handed_out)
                    .saturating_sub(count.saturating_mul(total_weight));
                let deviation = above.max(below);
                worst_deviation = worst_deviation.max(deviation);
                assert!(
                    deviation <= total_weight,
                    "at tier {tenants}, after {handed_out} grants, tenant {position} \
                     took {count} at weight {weight} of {total_weight}, which is \
                     {deviation} shares of one grant away from its ideal -- more than \
                     one round"
                );
            }
        }
    }
    assert!(
        grants.iter().all(|count| *count > 0),
        "at tier {tenants} a continuously backlogged tenant took no grant across \
         {target} grants"
    );
    Ok(Fairness {
        weights,
        grants,
        total_weight,
        handed_out,
        worst_deviation,
        longest_wait,
    })
}

/// Every continuously backlogged tenant's share stays inside one round of its
/// weight's share, at every tier, and each tenant's share is recorded.
///
/// The tiers are two, one hundred, one thousand and five thousand. Two is the
/// smallest world in which a share can differ from another's; five thousand is
/// the estate's declared tenant-provision tier, and a bound that only held at two
/// would not be a bound on a fleet.
fn backlogged_tenants_take_shares_in_proportion_to_their_weights(band: sim::Band) -> TestResult {
    let mut trace = Trace::new();
    for tenants in FAIRNESS_TIERS {
        let mut worst_deviation = 0_u64;
        let mut longest = 0_u64;
        let mut served_all = true;
        let mut rounds = 0_u64;
        let mut world_weight = 0_u64;
        for seed in band.seeds() {
            let round = fair_round(tenants, seed)?;
            worst_deviation = worst_deviation.max(round.worst_deviation);
            longest = longest.max(round.longest_wait);
            served_all &= round.grants.iter().all(|grants| *grants > 0);
            rounds = round.handed_out;
            world_weight = round.total_weight;
            if seed == band.first {
                round.record(&mut trace, tenants);
            }
        }
        assert!(
            served_all,
            "at tier {tenants} a continuously backlogged tenant took no grant at all"
        );
        assert!(
            longest > 0,
            "at tier {tenants} no tenant ever waited for a grant, so the starvation \
             invariant was never under pressure"
        );
        // No cross-seed bound on `longest` here: every seed draws its own
        // weights, so each has its own round length, and the longest gap across a
        // band is not measured against any one of them. The bound is enforced
        // where it means something -- inside `fair_round`, against the round
        // length of the world that produced the gap -- and the widest round this
        // band drew is recorded beside the longest gap so a reader has both.
        trace.record_number("fair-tier-grants", rounds);
        trace.record_number("fair-tier-widest-round", world_weight);
        trace.record_number("fair-tier-worst-deviation", worst_deviation);
        trace.record_number("fair-tier-longest-wait", longest);
    }
    Ok(())
}

/// A weighted round replays: the same seed produces the same shares at every
/// tier, so a fairness number above is a measurement of the algorithm rather than
/// of the draw that produced it.
fn a_weighted_round_replays(band: sim::Band) -> TestResult {
    for tenants in FAIRNESS_TIERS {
        for seed in band.seeds() {
            let first = fair_round(tenants, seed)?;
            let second = fair_round(tenants, seed)?;
            assert_eq!(
                first.grants, second.grants,
                "seed {seed} at tier {tenants} produced two different share vectors"
            );
            assert_eq!(
                (first.handed_out, first.total_weight),
                (second.handed_out, second.total_weight),
                "seed {seed} at tier {tenants} produced two different world shapes"
            );
        }
    }
    Ok(())
}

band_family::band_family! {
    backlogged_tenants_take_shares_in_proportion_to_their_weights_band_00 => backlogged_tenants_take_shares_in_proportion_to_their_weights, 4;
    backlogged_tenants_take_shares_in_proportion_to_their_weights_band_01 => backlogged_tenants_take_shares_in_proportion_to_their_weights, 5;
    a_weighted_round_replays_band_00 => a_weighted_round_replays, 6;
    a_weighted_round_replays_band_01 => a_weighted_round_replays, 14;
}
