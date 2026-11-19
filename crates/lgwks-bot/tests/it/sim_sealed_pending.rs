//! Simulation family: hostile key faults against the pending listing of a
//! sealed run store (#1085, ADR 0065), through the real host only.
//!
//! The seed decides how many approvals each tenant parks on one shared sealed
//! store, which tenant asks when, how many unwraps the provider refuses, and
//! how often the plug is pulled. The oracle is a reference model: the set of
//! ids each tenant parked. A listing is either the typed transient refusal or
//! exactly that tenant's set. A shorter set is the silent drop of #1085;
//! another tenant's id is a leak.
//!
//! | Family | Fool | Property it pins |
//! |---|---|---|
//! | `saboteur_faults_never_shrink_a_listing` | saboteur, neighbour, monkey | an armed refusal fails every listing that opens a step, and a tenant with nothing parked still lists empty |
//! | `pulled_plug_reopen_keeps_every_park` | pulled-plug, wrong-order | after each seeded reopen, with fresh parks between, a faulted listing fails typed and a healed one equals the model |
//!
//! Same seed, same trace hash: both families sweep their band twice through
//! [`sim::assert_replays`]. The trace records counts and outcomes, never
//! approval ids, which the run's entropy draws fresh.

#![cfg(all(feature = "script", feature = "ephemeral", feature = "time"))]

use std::collections::BTreeSet;
use std::sync::Arc;

use lgwks_bot::sealed::keys::KeyProvider;
use lgwks_bot::task::{Host, RunStore};

use super::pending_fault::{host_over, ids, park, transient_key_refusal};
use crate::sealed_harness::{FaultingProvider, open_store};
use crate::sim;
use sim::Band;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Tenants a seed may put on one store. The last parks nothing in the
/// saboteur family, so an empty neighbour is always in reach.
const TENANTS: [&str; 4] = ["acme", "globex", "initech", "umbrella"];

/// One tenant's view: its name, its host and the ids it parked.
type Held = (&'static str, Host, BTreeSet<String>);

/// Check one listing of `held` against the model. `armed` says a refusal was
/// armed and this tenant has a step to open, so the listing must fail;
/// otherwise it must equal the model exactly. Returns whether it failed.
fn judge(seed: u64, held: &Held, armed: bool) -> Result<bool, Box<dyn std::error::Error>> {
    let (name, ref host, ref model) = *held;
    match host.pending(usize::MAX) {
        Err(error) if transient_key_refusal(&error) && !model.is_empty() => Ok(true),
        Err(error) => {
            lgwks_std::trace::debug!(seed, name, error = ?error, "sim pending: a wrong refusal");
            Err(format!(
                "seed {seed} {name}: refused {error:?} with {} parked",
                model.len()
            )
            .into())
        }
        Ok(_) if armed => {
            lgwks_std::trace::debug!(seed, name, "sim pending: an armed fault answered");
            Err(format!("seed {seed} {name}: an armed fault never answers a listing").into())
        }
        Ok(listed) => {
            let found: BTreeSet<String> = ids(&listed).into_iter().collect();
            if found == *model {
                return Ok(false);
            }
            lgwks_std::trace::debug!(seed, name, "sim pending: the listing left the model");
            Err(format!(
                "seed {seed} {name}: listed {} ids, the model holds {}, overlap {}",
                found.len(),
                model.len(),
                found.intersection(model).count()
            )
            .into())
        }
    }
}

/// A sealed store in `dir` over the shared faulting `provider`.
fn store_over(
    dir: &std::path::Path,
    provider: &Arc<FaultingProvider>,
) -> Result<RunStore, Box<dyn std::error::Error>> {
    let keyed: Arc<dyn KeyProvider> = Arc::<FaultingProvider>::clone(provider);
    open_store(dir, keyed)
}

/// Park `count` seeded payloads for `held`, adding each id to its model.
fn park_into(sim: &mut sim::Sim, held: &mut Held, count: u32) -> TestResult {
    for _ in 0..count {
        let tag = sim.rng().below(1 << 20);
        let id = park(&held.1, format!("{}-{tag}", held.0).as_bytes())?;
        held.2.insert(id);
    }
    Ok(())
}

/// The saboteur arms a seeded number of refusals before a seeded tenant
/// lists, the neighbours list in a seeded order, and a tenant that parked
/// nothing must still list empty under the fault: it has nothing to open.
fn saboteur_faults_never_shrink_a_listing(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("sealed-pending-saboteur")?;
        let provider = Arc::new(FaultingProvider::open(&dir.join("keys"))?);
        let store = store_over(&dir, &provider)?;
        let mut held: Vec<Held> = Vec::new();
        for name in TENANTS {
            held.push((name, host_over(&store, name)?, BTreeSet::new()));
        }
        for entry in held.iter_mut().take(TENANTS.len().saturating_sub(1)) {
            let count = sim.rng().between(0, 5);
            park_into(sim, entry, count)?;
        }
        let mut refused = 0_u64;
        let rounds = sim.rng().between(4, 12);
        for _ in 0..rounds {
            let pick = usize::try_from(sim.rng().below(u32::try_from(held.len())?))?;
            let Some(entry) = held.get(pick) else {
                lgwks_std::trace::debug!(
                    seed = sim.seed,
                    pick,
                    "sim pending: a pick past the tenants"
                );
                return Err(format!("seed {}: a pick past the tenants", sim.seed).into());
            };
            let arm = sim.rng().below(4);
            if arm == 3 {
                provider.fail_all_unwraps(true);
            } else {
                provider.fail_next_unwraps(u64::from(arm));
            }
            if judge(sim.seed, entry, arm > 0 && !entry.2.is_empty())? {
                refused = refused.saturating_add(1);
            }
            provider.fail_all_unwraps(false);
            provider.fail_next_unwraps(0);
        }
        for entry in &held {
            judge(sim.seed, entry, false)?;
        }
        sim.record("saboteur-faults-never-shrink-a-listing");
        sim.trace.record_number("refused", refused);
        let parked: usize = held.iter().map(|entry| entry.2.len()).sum();
        sim.trace.record_number("parked", u64::try_from(parked)?);
        Ok(())
    })
}

/// The plug is pulled a seeded number of times: every handle dropped, the
/// store reopened, fresh parks between. After each reopen a faulted listing
/// fails typed and a healed one equals the model across every restart.
fn pulled_plug_reopen_keeps_every_park(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("sealed-pending-plug")?;
        let provider = Arc::new(FaultingProvider::open(&dir.join("keys"))?);
        let names = ["acme", "globex"];
        let mut models: Vec<BTreeSet<String>> = vec![BTreeSet::new(); names.len()];
        let mut refused = 0_u64;
        let lives = sim.rng().between(2, 4);
        for _ in 0..lives {
            let store = store_over(&dir, &provider)?;
            let mut held: Vec<Held> = Vec::new();
            for (name, model) in names.into_iter().zip(models.iter()) {
                held.push((name, host_over(&store, name)?, model.clone()));
            }
            // Wrong order: list under a fault before this life parks anything.
            provider.fail_all_unwraps(true);
            for entry in &held {
                if judge(sim.seed, entry, !entry.2.is_empty())? {
                    refused = refused.saturating_add(1);
                }
            }
            provider.fail_all_unwraps(false);
            for entry in held.iter_mut() {
                let count = sim.rng().between(0, 3);
                park_into(sim, entry, count)?;
            }
            for entry in &held {
                judge(sim.seed, entry, false)?;
            }
            models = held.into_iter().map(|entry| entry.2).collect();
            drop(store);
        }
        sim.record("pulled-plug-reopen-keeps-every-park");
        sim.trace.record_number("refused", refused);
        let parked: usize = models.iter().map(BTreeSet::len).sum();
        sim.trace.record_number("parked", u64::try_from(parked)?);
        Ok(())
    })
}

#[test]
fn saboteur_faults_never_shrink_a_listing_band_20() -> TestResult {
    saboteur_faults_never_shrink_a_listing(sim::band_of(20))
}

#[test]
fn pulled_plug_reopen_keeps_every_park_band_21() -> TestResult {
    pulled_plug_reopen_keeps_every_park(sim::band_of(21))
}
