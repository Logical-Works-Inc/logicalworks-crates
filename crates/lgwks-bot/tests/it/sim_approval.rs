//! Simulation family: seeded park/decide/crash/resume interleavings.
//!
//! The public journeys in `tests/approval.rs` pin one shape each: a park, a
//! decision, a cross-tenant refusal, a lapse, a ceiling and a race. This
//! target asks the same questions of *interleavings* — every order in which
//! many tenants park, decide, crash and resume the same store — over a seeded
//! sweep. What the seed controls is the input: the tenants, the payloads, the
//! verdict each park gets, and where the crashes land. The store, the hosts
//! and the decisions are real; no wall clock is consulted.
//!
//! | Family | Property it pins |
//! |---|---|
//! | `parked_runs_resume_after_a_decision` | parks, seeded decisions, crashes and resumes interleave across tenants; exactly one dispatch per approved, none for denied or expired, the same trace every replay |
//! | `the_same_seed_replays_the_same_approval_trace` | the same seed run twice reaches the same trace hash |
//! | `distinct_seeds_diverge_in_their_approval_trace` | distinct seeds reach distinct trace hashes |
//!
//! Each declared test sweeps its band twice and requires identical trace
//! hashes, so a nondeterministic run fails even when every assertion passes.

#![cfg(all(feature = "script", feature = "ephemeral", feature = "time"))]

use std::cell::RefCell;
use std::time::Duration;

use lgwks_bot::script::{ApprovalDecision, Clock, DecisionError, FlowError, Scope};
use lgwks_bot::task::{Disposition, Host, task};

use crate::band_family::band_family;
use crate::scratch::Scratch;
use crate::sim::{self, Band};

use crate::approval_error::ApprovalError as ApprovalSimError;
use crate::sim_approval_extra::{parked_id_of, run_of};

/// A test's result: an error fails it with the refusal behind it.
type TestResult = Result<(), ApprovalSimError>;

// The payloads one run's payload block produced.
//
// Thread-local, like the payload recorder the point tests share: the suite
// runs families on many threads, and one global count would let two seeds
// move each other's numbers.
thread_local! {
    static SIM_NOTED: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
}

/// Record `payload` as produced, and hand it back.
fn note(payload: Vec<u8>) -> Vec<u8> {
    // A plain borrow: nothing on this thread holds the cell across a call, and
    // a note dropped on a conflict would hide a re-poll from the trace.
    SIM_NOTED.with(|noted| noted.borrow_mut().push(payload.clone()));
    payload
}

/// Every payload produced on this thread, and forget them.
fn take_noted() -> Vec<Vec<u8>> {
    SIM_NOTED.with(|noted| std::mem::take(&mut *noted.borrow_mut()))
}

lgwks_bot::script! {
    /// Park the payload for a human decision, then give it back.
    pub(crate) flow ship(payload: Vec<u8>) -> Vec<u8>:
        let approved = await approval ship within 30s:
            note(payload)
        give back approved.payload().to_vec()
}

/// A host for `tenant` over the run store in `dir`, on a virtual clock.
fn host_on(dir: &std::path::Path, tenant: &str) -> Result<Host, ApprovalSimError> {
    Ok(Host::builder(tenant)?
        .run_store(dir)?
        .clock(Clock::virtual_at(Duration::ZERO))
        .build()?)
}

/// A draw below `bound`, as a `usize` the scenario indexes with.
fn draw_below(sim: &mut sim::Sim, bound: u32) -> Result<usize, ApprovalSimError> {
    usize::try_from(sim.rng().below(bound)).map_err(|error| {
        lgwks_std::trace::debug!(error = %error, "draw_below: a draw fits its bound");
        ApprovalSimError::Setup {
            what: "a draw fits its bound",
        }
    })
}
/// What the seed says happens to one parked approval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fate {
    /// Decide approval with the pending digest, then resume.
    Approve,
    /// Decide denial with the pending digest, then resume.
    Deny,
    /// Advance past the deadline, resume into the lapse, then decide.
    Expire,
    /// Never decide: the resume parks again under the same id.
    Leave,
}

/// The fates, in the order a seed draws them.
const FATES: [Fate; 4] = [Fate::Approve, Fate::Deny, Fate::Expire, Fate::Leave];

/// One seeded park and everything drawn for it: park, maybe crash, decide by
/// fate, resume, and exactly the outcome its fate names.
fn tend_approval(
    hosts: &mut Vec<Host>,
    dir: &std::path::Path,
    sim: &mut sim::Sim,
    tenant: usize,
    index: usize,
) -> Result<(), ApprovalSimError> {
    lgwks_std::trace::debug!(
        tenant,
        index,
        seed = sim.seed,
        "tend_approval: tending one seeded park"
    );
    let payload = format!("t{tenant}-a{index}-{}", sim.seed).into_bytes();
    let shipping = task("ship", |scope: Scope, payload: Vec<u8>| async move {
        ship(&scope, payload).await
    })?;
    let host = hosts.get(tenant).ok_or(ApprovalSimError::Setup {
        what: "the tenant has a host",
    })?;
    let report = lgwks_bot::block_on(host.run(&shipping, payload.clone()));
    if !matches!(report.disposition(), Disposition::Parked) {
        return Err(ApprovalSimError::Setup {
            what: "a first run parks",
        });
    }
    let id = parked_id_of(&report)?;
    let run = run_of(&report)?;
    // A crash between the park and the decision: drop every host and reopen
    // over the same directory, so the decision and the resume find only what
    // the file holds.
    if sim.rng().below(4) == 0 {
        hosts.clear();
        for name in ["acme", "zenith", "apex"] {
            hosts.push(host_on(dir, name)?);
        }
    }
    let Some(&fate) = FATES.get(draw_below(sim, 4)?) else {
        lgwks_std::trace::debug!("tend_approval: a draw below four named no fate");
        return Err(ApprovalSimError::Setup {
            what: "a draw below four names a fate",
        });
    };
    let decider = hosts.get(tenant).ok_or(ApprovalSimError::Setup {
        what: "the tenant has a host",
    })?;
    let digest = *decider
        .pending(usize::MAX)?
        .iter()
        .find(|entry| entry.id() == id)
        .map(|entry| entry.digest())
        .ok_or(ApprovalSimError::Setup {
            what: "the park is listed",
        })?;
    let outcome = match fate {
        Fate::Approve => {
            decider.decide(&id, ApprovalDecision::approve("sim", digest))?;
            let resumed = lgwks_bot::block_on(decider.resume(run, &shipping, payload.clone()));
            if !matches!(resumed.disposition(), Disposition::Succeeded)
                || resumed.output() != Some(&payload)
            {
                return Err(ApprovalSimError::Setup {
                    what: "an approval resumes with its payload",
                });
            }
            "approved"
        }
        Fate::Deny => {
            decider.decide(&id, ApprovalDecision::deny("sim", digest))?;
            let resumed = lgwks_bot::block_on(decider.resume(run, &shipping, payload));
            if !matches!(
                resumed.error(),
                Some(error) if matches!(*error, FlowError::ApprovalDenied { .. })
            ) {
                return Err(ApprovalSimError::Setup {
                    what: "a denial fails the resume typed",
                });
            }
            "denied"
        }
        Fate::Expire => {
            decider
                .clock()
                .advance(Duration::from_secs(31))
                .map_err(|error| {
                    lgwks_std::trace::debug!(error = %error, "tend_approval: the virtual clock advances");
                    ApprovalSimError::Setup {
                        what: "the virtual clock advances",
                    }
                })?;
            let resumed = lgwks_bot::block_on(decider.resume(run, &shipping, payload));
            if !matches!(
                resumed.error(),
                Some(error) if matches!(*error, FlowError::ApprovalExpired { .. })
            ) {
                return Err(ApprovalSimError::Setup {
                    what: "a lapse fails the resume typed",
                });
            }
            match decider.decide(&id, ApprovalDecision::approve("sim", digest)) {
                Err(DecisionError::Expired { .. }) => {}
                _ => {
                    return Err(ApprovalSimError::Setup {
                        what: "a decision past the deadline is Expired",
                    });
                }
            }
            // The clock stays advanced: later parks on this tenant's host
            // start owed past their own deadline, so they lapse at once.
            "expired"
        }
        Fate::Leave => {
            let parked = lgwks_bot::block_on(decider.resume(run, &shipping, payload));
            if !matches!(parked.disposition(), Disposition::Parked) {
                return Err(ApprovalSimError::Setup {
                    what: "an undecided resume parks again",
                });
            }
            "undecided"
        }
    };
    sim.record(&format!("t{tenant}:a{index}:{outcome}"));
    Ok(())
}

/// Parks, seeded decisions, crashes and resumes interleaved across tenants:
/// exactly one dispatch per approved, none for denied or expired, and no
/// payload block polled twice.
fn interleaved_parks_decisions_and_resumes(band: Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "interleaved: sweeping seeded parks, decisions and resumes");
    for seed in band.seeds() {
        let mut sim = sim::Sim::new(seed);
        let scratch = Scratch::new("approval-sim").map_err(|error| {
            lgwks_std::trace::debug!(error = %error, "interleaved: the scratch dir failed");
            ApprovalSimError::Setup {
                what: "the scratch dir failed",
            }
        })?;
        take_noted();
        let mut hosts = Vec::new();
        for name in ["acme", "zenith", "apex"] {
            hosts.push(host_on(scratch.path(), name)?);
        }
        let tenants = draw_below(&mut sim, 3)?.saturating_add(1);
        for tenant in 0..tenants {
            let approvals = draw_below(&mut sim, 3)?.saturating_add(1);
            for index in 0..approvals {
                tend_approval(&mut hosts, scratch.path(), &mut sim, tenant, index)?;
            }
        }
        // Every payload the blocks produced, exactly once each: a resume
        // replays the recorded payload and never re-polls its block.
        let mut noted = take_noted();
        if noted.is_empty() {
            return Err(ApprovalSimError::Setup {
                what: "the seed parked at least one approval",
            });
        }
        noted.sort();
        let mut unique = noted.clone();
        unique.dedup();
        if noted != unique {
            return Err(ApprovalSimError::Setup {
                what: "every payload block ran exactly once",
            });
        }
    }
    Ok(())
}

band_family! {
    parked_runs_resume_after_a_decision_band_00 => parked_runs_resume_after_a_decision, 0;
    parked_runs_resume_after_a_decision_band_01 => parked_runs_resume_after_a_decision, 1;
}

/// The seeded interleaving, replayed twice for one trace hash.
fn parked_runs_resume_after_a_decision(band: Band) -> TestResult {
    interleaved_parks_decisions_and_resumes(band)
}

/// The same seed run twice reaches the same trace hash.
fn the_same_seed_replays_the_same_approval_trace(_band: Band) -> TestResult {
    lgwks_std::trace::debug!("same_seed: replaying one seed twice for one trace hash");
    let mut first = sim::Sim::new(7);
    replay_seed(&mut first)?;
    let mut second = sim::Sim::new(7);
    replay_seed(&mut second)?;
    if first.hash() != second.hash() {
        return Err(ApprovalSimError::Setup {
            what: "one seed replays to one trace",
        });
    }
    Ok(())
}

/// The scenario one seed replays: two tenants, one park each, decided
/// opposite ways.
fn replay_seed(sim: &mut sim::Sim) -> Result<(), ApprovalSimError> {
    lgwks_std::trace::debug!(
        seed = sim.seed,
        "replay_seed: replaying two tenants decided opposite ways"
    );
    let scratch = Scratch::new("approval-replay").map_err(|error| {
        lgwks_std::trace::debug!(error = %error, "replay_seed: the scratch dir failed");
        ApprovalSimError::Setup {
            what: "the scratch dir failed",
        }
    })?;
    take_noted();
    let shipping = task("ship", |scope: Scope, payload: Vec<u8>| async move {
        ship(&scope, payload).await
    })?;
    for (tenant, verdict) in [("acme", true), ("zenith", false)] {
        let host = host_on(scratch.path(), tenant)?;
        let payload = format!("{tenant}-{}", sim.seed).into_bytes();
        let report = lgwks_bot::block_on(host.run(&shipping, payload.clone()));
        let id = {
            let Some(error) = report.error() else {
                return Err(ApprovalSimError::Setup {
                    what: "a first run parks",
                });
            };
            match *error {
                FlowError::Parked { ref id, .. } => id.clone(),
                _ => {
                    return Err(ApprovalSimError::Setup {
                        what: "a first run parks",
                    });
                }
            }
        };
        let run = report.run_id().ok_or(ApprovalSimError::Setup {
            what: "a parked run names its run",
        })?;
        let digest = *host
            .pending(usize::MAX)?
            .iter()
            .find(|entry| entry.id() == id)
            .map(|entry| entry.digest())
            .ok_or(ApprovalSimError::Setup {
                what: "the park is listed",
            })?;
        let decision = if verdict {
            ApprovalDecision::approve("sim", digest)
        } else {
            ApprovalDecision::deny("sim", digest)
        };
        host.decide(&id, decision)?;
        let resumed = lgwks_bot::block_on(host.resume(run, &shipping, payload.clone()));
        sim.record(&format!(
            "{tenant}:{}:{}",
            sim.seed,
            if verdict { "approved" } else { "denied" }
        ));
        if verdict != matches!(resumed.disposition(), Disposition::Succeeded) {
            return Err(ApprovalSimError::Setup {
                what: "the resume matches the decision",
            });
        }
    }
    Ok(())
}

/// Distinct seeds reach distinct trace hashes.
fn distinct_seeds_diverge_in_their_approval_trace(_band: Band) -> TestResult {
    lgwks_std::trace::debug!("distinct_seeds: three seeds must reach three trace hashes");
    let mut hashes = std::collections::HashSet::new();
    for seed in [11u64, 12, 13] {
        let mut sim = sim::Sim::new(seed);
        replay_seed(&mut sim)?;
        hashes.insert(sim.hash());
    }
    if hashes.len() != 3 {
        return Err(ApprovalSimError::Setup {
            what: "distinct seeds diverge in their trace",
        });
    }
    Ok(())
}

band_family! {
    the_same_seed_replays_the_same_approval_trace_band_00 => the_same_seed_replays_the_same_approval_trace, 0;
    distinct_seeds_diverge_in_their_approval_trace_band_00 => distinct_seeds_diverge_in_their_approval_trace, 0;
}
