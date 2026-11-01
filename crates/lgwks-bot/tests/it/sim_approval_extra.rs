//! Seeded approval interleavings, second family: the fix-pass scenarios for #784.
//!
//! Each test names the requirement it proves in its doc comment and sweeps its
//! band twice through [`sim::assert_replays`], so the same seed reaching a
//! different trace hash fails even when every assertion passes. The store, the
//! hosts and the decisions are real; only the seed is virtualised.

#![cfg(all(feature = "script", feature = "ephemeral", feature = "time"))]

use std::time::Duration;

use lgwks_bot::script::{ApprovalDecision, Clock, DecisionError, FlowError, Scope};
use lgwks_bot::task::{Disposition, Host, RunStore, StoreError, task};

use crate::approval_error::ApprovalError as ApprovalSimError;
use crate::scratch::Scratch;
use crate::sim;

/// A test's result: an error fails it with the refusal behind it.
pub(crate) type TestResult = Result<(), ApprovalSimError>;

/// One run of a byte flow, boxed so a task over it has a type a helper names.
/// Not `Send`: a flow's future holds the run's own store handles, and the
/// host polls it on the thread that runs it.
pub(crate) type BoxedRun =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, FlowError>>>>;

/// A task over a byte flow, named so fixtures can hand one back.
pub(crate) type ByteTask = lgwks_bot::task::Task<fn(Scope, Vec<u8>) -> BoxedRun>;

/// One run of `ship`.
fn ship_run(scope: Scope, payload: Vec<u8>) -> BoxedRun {
    Box::pin(async move { ship(&scope, payload).await })
}

/// The `ship` flow as a task a host runs and resumes.
pub(crate) fn shipping() -> Result<ByteTask, ApprovalSimError> {
    let body: fn(Scope, Vec<u8>) -> BoxedRun = ship_run;
    Ok(task("ship", body)?)
}

/// A scratch store for one sweep, or the fixture refusal that names it.
pub(crate) fn scratch(tag: &str) -> Result<Scratch, ApprovalSimError> {
    Scratch::new(tag).map_err(|error| {
        lgwks_std::trace::debug!(error = %error, tag, "approval fixture: the scratch dir failed");
        ApprovalSimError::Setup {
            what: "the scratch dir failed",
        }
    })
}

/// Sweep every seed in `band` twice through `body`: the same seed must reach
/// the same trace hash, so a nondeterministic run fails even when every
/// assertion passes.
pub(crate) fn sweep_twice(
    band: sim::Band,
    mut body: impl FnMut(&mut sim::Sim) -> TestResult,
) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: sweeping one band twice");
    for seed in band.seeds() {
        let mut first = sim::Sim::new(seed);
        body(&mut first)?;
        let hash_first = first.hash();
        let mut second = sim::Sim::new(seed);
        body(&mut second)?;
        if hash_first != second.hash() {
            lgwks_std::trace::debug!(seed, "extra: one seed replayed to two traces");
            return Err(ApprovalSimError::Setup {
                what: "one seed replays to one trace",
            });
        }
    }
    Ok(())
}

lgwks_bot::script! {
    /// Park the payload for a human decision, then give it back.
    pub(crate) flow ship(payload: Vec<u8>) -> Vec<u8>:
        let approved = await approval ship within 30s:
            payload
        give back approved.payload().to_vec()
}

/// A host for `tenant` over the run store in `dir`, on a virtual clock.
pub(crate) fn host_on(dir: &std::path::Path, tenant: &str) -> Result<Host, ApprovalSimError> {
    Ok(Host::builder(tenant)?
        .run_store(dir)?
        .clock(Clock::virtual_at(Duration::ZERO))
        .build()?)
}

/// The parked id a report carries.
pub(crate) fn parked_id_of(
    report: &lgwks_bot::task::Report<Vec<u8>>,
) -> Result<String, ApprovalSimError> {
    lgwks_std::trace::debug!("extra: reading the parked id out of the report");
    let Some(error) = report.error() else {
        return Err(ApprovalSimError::Setup {
            what: "a parked run names its id",
        });
    };
    match *error {
        FlowError::Parked { ref id, .. } => Ok(id.clone()),
        _ => Err(ApprovalSimError::Setup {
            what: "a parked run names its id",
        }),
    }
}

/// Move `host`'s virtual clock one second past `ship`'s `within 30s`, so the
/// next resume or decision finds the approval lapsed.
pub(crate) fn lapse(host: &Host) -> TestResult {
    match host.clock().advance(Duration::from_secs(31)) {
        Ok(_) => Ok(()),
        Err(error) => {
            lgwks_std::trace::debug!(error = %error, "extra: the virtual clock advances");
            Err(ApprovalSimError::Setup {
                what: "the virtual clock advances",
            })
        }
    }
}

/// Whether `report` failed as a lapsed approval, the typed `ApprovalExpired`.
pub(crate) fn lapsed(report: &lgwks_bot::task::Report<Vec<u8>>) -> bool {
    matches!(
        report.error(),
        Some(error) if matches!(*error, FlowError::ApprovalExpired { .. })
    )
}

/// The run a parked report was recorded under, which every resume names.
pub(crate) fn run_of(
    report: &lgwks_bot::task::Report<Vec<u8>>,
) -> Result<lgwks_bot::effect::RunId, ApprovalSimError> {
    match report.run_id() {
        Some(run) => Ok(run),
        None => {
            lgwks_std::trace::debug!("extra: a parked report named no run");
            Err(ApprovalSimError::Setup {
                what: "a parked run names its run",
            })
        }
    }
}

/// Park one payload for `tenant` and return its run, id and digest.
pub(crate) fn park_one(
    host: &Host,
    payload: Vec<u8>,
) -> Result<(lgwks_bot::effect::RunId, String, lgwks_std::hash::Digest), ApprovalSimError> {
    lgwks_std::trace::debug!("extra: parking one payload for a decision");
    let shipping = shipping()?;
    let report = lgwks_bot::block_on(host.run(&shipping, payload));
    if !matches!(report.disposition(), Disposition::Parked) {
        return Err(ApprovalSimError::Setup {
            what: "a first run parks",
        });
    }
    let id = parked_id_of(&report)?;
    let run = run_of(&report)?;
    let digest = *host
        .pending(usize::MAX)?
        .iter()
        .find(|entry| entry.id() == id)
        .map(|entry| entry.digest())
        .ok_or(ApprovalSimError::Setup {
            what: "the park is listed",
        })?;
    Ok((run, id, digest))
}

/// APR-2 deny path: a denial fails the resume typed and never dispatches.
fn sim_deny_then_resume_denied(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: denying then resuming one park");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-deny")?;
        let host = host_on(scratch.path(), "acme")?;
        let (run, id, digest) = park_one(&host, b"deny-me".to_vec())?;
        host.decide(&id, ApprovalDecision::deny("sim", digest))?;
        let shipping = shipping()?;
        let resumed = lgwks_bot::block_on(host.resume(run, &shipping, b"deny-me".to_vec()));
        if !matches!(
            resumed.error(),
            Some(error) if matches!(*error, FlowError::ApprovalDenied { .. })
        ) {
            return Err(ApprovalSimError::Setup {
                what: "a denial fails the resume typed",
            });
        }
        sim.record("deny-then-resume:denied");
        Ok(())
    })
}

/// APR-5 second half: a resume past the deadline lapses, then a decision is expired.
fn sim_expire_then_decide_reports_expired(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: expiring then deciding one park");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-expire")?;
        let host = host_on(scratch.path(), "acme")?;
        let (run, id, digest) = park_one(&host, b"lapse-me".to_vec())?;
        lapse(&host)?;
        let shipping = shipping()?;
        let resumed = lgwks_bot::block_on(host.resume(run, &shipping, b"lapse-me".to_vec()));
        if !lapsed(&resumed) {
            return Err(ApprovalSimError::Setup {
                what: "a lapse fails the resume typed",
            });
        }
        match host.decide(&id, ApprovalDecision::approve("sim", digest)) {
            Err(DecisionError::Expired { .. }) => {}
            _ => {
                return Err(ApprovalSimError::Setup {
                    what: "a decision past the deadline is Expired",
                });
            }
        }
        sim.record("expire-then-decide:expired");
        Ok(())
    })
}

/// One decider thread for `id`, released by `barrier` with the other racer, so
/// both appends reach the store together; it answers which way its append
/// went. The handle is joined by the caller, so no thread outlives the test.
fn racer(
    host: &std::sync::Arc<Host>,
    id: &str,
    digest: lgwks_std::hash::Digest,
    barrier: &std::sync::Arc<std::sync::Barrier>,
    by: &'static str,
) -> Result<std::thread::JoinHandle<&'static str>, ApprovalSimError> {
    let (host, id, barrier) = (
        std::sync::Arc::clone(host),
        id.to_owned(),
        std::sync::Arc::clone(barrier),
    );
    std::thread::Builder::new()
        .name(format!("extra-racer-{by}"))
        .spawn(move || {
            barrier.wait();
            match host.decide(&id, ApprovalDecision::approve(by, digest)) {
                Ok(()) => "recorded",
                Err(DecisionError::AlreadyDecided { .. }) => "decided",
                Err(_) => "other",
            }
        })
        .map_err(|error| {
            lgwks_std::trace::debug!(error = %error, by, "extra: a racer failed to spawn");
            ApprovalSimError::Setup {
                what: "a racer spawns",
            }
        })
}

/// APR-9 at the append point: a barrier starts two deciders together, five parks in a row.
fn sim_decide_racing_at_each_append_point(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: racing two deciders behind a barrier");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-race")?;
        let host = std::sync::Arc::new(host_on(scratch.path(), "acme")?);
        for round in 0..5u32 {
            let payload = format!("race-{round}-{}", sim.seed).into_bytes();
            let (run, id, digest) = park_one(&host, payload.clone())?;
            let _ = run;
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let first = racer(&host, &id, digest, &barrier, "a")?;
            let second = racer(&host, &id, digest, &barrier, "b")?;
            let mut outcomes = [
                first.join().map_err(|error| {
                    lgwks_std::trace::debug!(?error, "extra: the first racer joins");
                    ApprovalSimError::Setup {
                        what: "the first racer joins",
                    }
                })?,
                second.join().map_err(|error| {
                    lgwks_std::trace::debug!(?error, "extra: the second racer joins");
                    ApprovalSimError::Setup {
                        what: "the second racer joins",
                    }
                })?,
            ];
            outcomes.sort();
            if outcomes != ["decided", "recorded"] {
                return Err(ApprovalSimError::Setup {
                    what: "exactly one racer records the decision",
                });
            }
            sim.record(&format!("race:{round}:one-winner"));
        }
        Ok(())
    })
}

/// APR-4 under load: ten parks for acme, ten foreign attempts by zed, ten audit rows.
fn sim_foreign_tenant_audit_under_load(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: refusing ten foreign decisions under load");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-foreign")?;
        let path = scratch.path().join("shared.runstore");
        let store_a = RunStore::open(&path)?;
        let store_b = store_a.clone();
        let host_a = Host::builder("acme")?.store(store_a).build()?;
        let host_b = Host::builder("zed")?.store(store_b).build()?;
        let mut ids = Vec::new();
        for index in 0..10u32 {
            let (_, id, _) = park_one(
                &host_a,
                format!("foreign-{index}-{}", sim.seed).into_bytes(),
            )?;
            ids.push(id);
        }
        for id in &ids {
            let digest = *host_a
                .pending(usize::MAX)?
                .iter()
                .find(|entry| entry.id() == *id)
                .map(|entry| entry.digest())
                .ok_or(ApprovalSimError::Setup {
                    what: "the park is listed",
                })?;
            match host_b.decide(id, ApprovalDecision::approve("zed", digest)) {
                Err(DecisionError::ForeignTenant { .. }) => {}
                _ => {
                    return Err(ApprovalSimError::Setup {
                        what: "another tenant is ForeignTenant",
                    });
                }
            }
        }
        let audit = host_b.audit();
        if audit.len() != 10 {
            return Err(ApprovalSimError::Setup {
                what: "every foreign attempt is audited",
            });
        }
        sim.record("foreign:ten-refused");
        sim.trace.record_number("foreign", 10);
        Ok(())
    })
}

/// APR-6 at exactly N and N+1: three parks land under a ceiling of three, the fourth is refused.
fn sim_ceiling_at_exactly_n_and_n_plus_one(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: parking to the ceiling and one past it");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-ceiling")?;
        let host = Host::builder("acme")?
            .run_store(scratch.path())?
            .max_pending_approvals(3)
            .build()?;
        for index in 0..3u32 {
            let (_, _, _) = park_one(&host, format!("n-{index}-{}", sim.seed).into_bytes())?;
        }
        if host.pending(usize::MAX)?.len() != 3 {
            return Err(ApprovalSimError::Setup {
                what: "exactly N parks are listed",
            });
        }
        let shipping = shipping()?;
        let over = lgwks_bot::block_on(
            host.run(&shipping, format!("n-plus-one-{}", sim.seed).into_bytes()),
        );
        let Some(failed) = over.error() else {
            return Err(ApprovalSimError::Setup {
                what: "a park past the ceiling is refused",
            });
        };
        match *failed {
            FlowError::Store { ref source, .. } => match **source {
                StoreError::Limit {
                    kind: lgwks_bot::task::StoreLimitKind::PendingApprovals,
                    ..
                } => {}
                _ => {
                    return Err(ApprovalSimError::Setup {
                        what: "a park past the ceiling names PendingApprovals",
                    });
                }
            },
            _ => {
                return Err(ApprovalSimError::Setup {
                    what: "a park past the ceiling is refused",
                });
            }
        }
        sim.record("ceiling:n-then-refused");
        Ok(())
    })
}

/// APR-3 as a storm: five parks decided with rotated digests, every one mismatched.
fn sim_digest_mismatch_storm(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: mismatching five parks in a storm");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-mismatch")?;
        let host = host_on(scratch.path(), "acme")?;
        let mut parks = Vec::new();
        for index in 0..5u32 {
            parks.push(park_one(
                &host,
                format!("storm-{index}-{}", sim.seed).into_bytes(),
            )?);
        }
        for (position, park) in parks.iter().enumerate() {
            let id = &park.1;
            let next = position.saturating_add(1);
            let rotated = if next >= parks.len() { 0 } else { next };
            let other = parks[rotated].2;
            match host.decide(id, ApprovalDecision::approve("storm", other)) {
                Err(DecisionError::DigestMismatch { .. }) => {}
                _ => {
                    return Err(ApprovalSimError::Setup {
                        what: "rotated bytes are a DigestMismatch",
                    });
                }
            }
        }
        if host.pending(usize::MAX)?.len() != 5 {
            return Err(ApprovalSimError::Setup {
                what: "mismatched parks stay undecided",
            });
        }
        sim.record("mismatch:five-refused");
        sim.trace.record_number("mismatched", 5);
        Ok(())
    })
}

#[test]
fn sim_deny_then_resume_denied_band_10() -> TestResult {
    sim_deny_then_resume_denied(sim::band_of(10))
}

#[test]
fn sim_expire_then_decide_reports_expired_band_11() -> TestResult {
    sim_expire_then_decide_reports_expired(sim::band_of(11))
}

#[test]
fn sim_decide_racing_at_each_append_point_band_12() -> TestResult {
    sim_decide_racing_at_each_append_point(sim::band_of(12))
}

#[test]
fn sim_foreign_tenant_audit_under_load_band_13() -> TestResult {
    sim_foreign_tenant_audit_under_load(sim::band_of(13))
}

#[test]
fn sim_ceiling_at_exactly_n_and_n_plus_one_band_14() -> TestResult {
    sim_ceiling_at_exactly_n_and_n_plus_one(sim::band_of(14))
}

#[test]
fn sim_digest_mismatch_storm_band_15() -> TestResult {
    sim_digest_mismatch_storm(sim::band_of(15))
}
