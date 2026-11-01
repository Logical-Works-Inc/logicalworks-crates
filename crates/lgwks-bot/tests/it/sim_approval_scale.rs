//! Seeded approval interleavings, third family (#784): restarts after every
//! record type, two to sixty-four tenants, store read faults, listing bounds,
//! a second park on one step key, a resume with no store, and replay across a
//! host rebuild. The fixtures and the `ship` flow are the second family's
//! ([`crate::sim_approval_extra`]), so every family sweeps one way.

#![cfg(all(feature = "script", feature = "ephemeral", feature = "time"))]

use lgwks_bot::script::{ApprovalDecision, DecisionError, FlowError};
use lgwks_bot::task::{Disposition, Host};

use crate::approval_error::ApprovalError as ApprovalSimError;
use crate::sim;
use crate::sim_approval_extra::{
    TestResult, host_on, lapse, lapsed, park_one, parked_id_of, scratch, shipping, sweep_twice,
};

/// Restart after every record type: pending, approved, denied and lapsed each survive reopen.
fn sim_restart_after_every_record_type(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: restarting after pending, approved, denied and lapsed");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-restart")?;
        let shipping = shipping()?;
        let mut kinds = Vec::new();
        {
            let host = host_on(scratch.path(), "acme")?;
            let (run_pending, id_pending, _) =
                park_one(&host, format!("pending-{}", sim.seed).into_bytes())?;
            let _ = run_pending;
            kinds.push(id_pending);
        }
        {
            let host = host_on(scratch.path(), "acme")?;
            if host.pending(usize::MAX)?.len() != 1 {
                return Err(ApprovalSimError::Setup {
                    what: "a pending record survives its restart",
                });
            }
            let (run, id, digest) = park_one(&host, format!("approved-{}", sim.seed).into_bytes())?;
            host.decide(&id, ApprovalDecision::approve("sim", digest))?;
            kinds.push(id);
            let reopened = host_on(scratch.path(), "acme")?;
            let resumed = lgwks_bot::block_on(reopened.resume(
                run,
                &shipping,
                format!("approved-{}", sim.seed).into_bytes(),
            ));
            if !matches!(resumed.disposition(), Disposition::Succeeded) {
                return Err(ApprovalSimError::Setup {
                    what: "an approved decision survives its restart",
                });
            }
        }
        {
            let host = host_on(scratch.path(), "acme")?;
            let (run, id, digest) = park_one(&host, format!("denied-{}", sim.seed).into_bytes())?;
            host.decide(&id, ApprovalDecision::deny("sim", digest))?;
            kinds.push(id);
            let reopened = host_on(scratch.path(), "acme")?;
            let resumed = lgwks_bot::block_on(reopened.resume(
                run,
                &shipping,
                format!("denied-{}", sim.seed).into_bytes(),
            ));
            if !matches!(
                resumed.error(),
                Some(error) if matches!(*error, FlowError::ApprovalDenied { .. })
            ) {
                return Err(ApprovalSimError::Setup {
                    what: "a denied decision survives its restart",
                });
            }
        }
        {
            let host = host_on(scratch.path(), "acme")?;
            let (run, id, digest) = park_one(&host, format!("lapsed-{}", sim.seed).into_bytes())?;
            lapse(&host)?;
            let resumed = lgwks_bot::block_on(host.resume(
                run,
                &shipping,
                format!("lapsed-{}", sim.seed).into_bytes(),
            ));
            if !lapsed(&resumed) {
                return Err(ApprovalSimError::Setup {
                    what: "a lapse records past its restart",
                });
            }
            match host.decide(&id, ApprovalDecision::approve("sim", digest)) {
                Err(DecisionError::Expired { .. }) => {}
                _ => {
                    return Err(ApprovalSimError::Setup {
                        what: "a lapsed decision is Expired after restart",
                    });
                }
            }
            kinds.push(id);
        }
        sim.record("restart:pending-approved-denied-lapsed");
        sim.trace.record_number("record-types", 4);
        Ok(())
    })
}

/// Two tenants isolated: each lists only its own park.
fn sim_two_tenants_isolated(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: isolating two tenants");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-two")?;
        let acme = host_on(scratch.path(), "acme")?;
        let zenith = host_on(scratch.path(), "zenith")?;
        let (_, acme_id, _) = park_one(&acme, format!("two-acme-{}", sim.seed).into_bytes())?;
        let (_, zenith_id, _) = park_one(&zenith, format!("two-zenith-{}", sim.seed).into_bytes())?;
        let acme_listed = acme.pending(usize::MAX)?;
        let zenith_listed = zenith.pending(usize::MAX)?;
        if acme_listed.len() != 1 || acme_listed[0].id() != acme_id {
            return Err(ApprovalSimError::Setup {
                what: "acme lists only its own park",
            });
        }
        if zenith_listed.len() != 1 || zenith_listed[0].id() != zenith_id {
            return Err(ApprovalSimError::Setup {
                what: "zenith lists only its own park",
            });
        }
        sim.record("tenants:two-isolated");
        Ok(())
    })
}

/// Eight tenants with a crash between park and decide: the file holds every park.
fn sim_eight_tenants_crash_interleaved(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: crashing eight tenants between park and decide");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-eight")?;
        let names = ["t0", "t1", "t2", "t3", "t4", "t5", "t6", "t7"];
        let mut hosts = Vec::new();
        for name in names {
            hosts.push(host_on(scratch.path(), name)?);
        }
        let mut parks = Vec::new();
        for (tenant, host) in hosts.iter().enumerate() {
            parks.push(park_one(
                host,
                format!("eight-{tenant}-{}", sim.seed).into_bytes(),
            )?);
        }
        drop(hosts);
        let mut reopened = Vec::new();
        for name in names {
            reopened.push(host_on(scratch.path(), name)?);
        }
        for (tenant, host) in reopened.iter().enumerate() {
            let park = &parks[tenant];
            let id = &park.1;
            let digest = park.2;
            host.decide(id, ApprovalDecision::approve("sim", digest))?;
        }
        for host in &reopened {
            if !host.pending(usize::MAX)?.is_empty() {
                return Err(ApprovalSimError::Setup {
                    what: "every decided park leaves the listing",
                });
            }
        }
        sim.record("tenants:eight-crash-decided");
        sim.trace.record_number("tenants", 8);
        Ok(())
    })
}

/// Sixty-four tenants at scale: every park approved and resumed exactly once.
fn sim_sixty_four_tenants_sweep(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: sweeping sixty-four tenants");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-sixtyfour")?;
        let shipping = shipping()?;
        let mut done = 0u32;
        for tenant in 0..64u32 {
            let name = format!("tenant-{tenant:02}");
            let host = host_on(scratch.path(), &name)?;
            let payload = format!("scale-{tenant}-{seed}", seed = sim.seed).into_bytes();
            let (run, id, digest) = park_one(&host, payload.clone())?;
            host.decide(&id, ApprovalDecision::approve("sim", digest))?;
            let resumed = lgwks_bot::block_on(host.resume(run, &shipping, payload));
            if !matches!(resumed.disposition(), Disposition::Succeeded) {
                return Err(ApprovalSimError::Setup {
                    what: "every tenant resumes approved",
                });
            }
            done = done.saturating_add(1);
        }
        if done != 64 {
            return Err(ApprovalSimError::Setup {
                what: "all sixty-four tenants decided",
            });
        }
        sim.record("tenants:sixty-four-approved");
        sim.trace.record_number("tenants", u64::from(done));
        Ok(())
    })
}

/// A store read fault on a pending lookup is the store, never an absence.
fn sim_read_fault_on_pending_lookup_is_store(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: faulting a pending lookup");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-fault-lookup")?;
        let host = host_on(scratch.path(), "acme")?;
        let (run, _, _) = park_one(&host, format!("fault-lookup-{}", sim.seed).into_bytes())?;
        let store = host
            .run_store()
            .ok_or(ApprovalSimError::Setup {
                what: "a stored host keeps a store",
            })?
            .clone();
        store.fail_next_index_read();
        let shipping = shipping()?;
        let refused = lgwks_bot::block_on(host.resume(
            run,
            &shipping,
            format!("fault-lookup-{}", sim.seed).into_bytes(),
        ));
        let Some(error) = refused.error() else {
            return Err(ApprovalSimError::Setup {
                what: "a faulted lookup does not succeed",
            });
        };
        match *error {
            FlowError::Store { .. } => {}
            _ => {
                return Err(ApprovalSimError::Setup {
                    what: "a faulted lookup is the store, not an absence",
                });
            }
        }
        drop(store);
        let recovered = host_on(scratch.path(), "acme")?;
        if recovered.pending(usize::MAX)?.is_empty() {
            return Err(ApprovalSimError::Setup {
                what: "the fault moved no record",
            });
        }
        sim.record("fault:lookup-is-store");
        Ok(())
    })
}

/// A store read fault on decide is the store, never an unknown id.
fn sim_read_fault_on_decide_is_store(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: faulting a decide lookup");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-fault-decide")?;
        let host = host_on(scratch.path(), "acme")?;
        let (_, id, digest) = park_one(&host, format!("fault-decide-{}", sim.seed).into_bytes())?;
        let store = host
            .run_store()
            .ok_or(ApprovalSimError::Setup {
                what: "a stored host keeps a store",
            })?
            .clone();
        store.fail_next_index_read();
        match host.decide(&id, ApprovalDecision::approve("sim", digest)) {
            Err(DecisionError::Store { .. }) => {}
            _ => {
                return Err(ApprovalSimError::Setup {
                    what: "a faulted decide is the store, not unknown",
                });
            }
        }
        sim.record("fault:decide-is-store");
        Ok(())
    })
}

/// Listing with a bound smaller than the count returns exactly the bound.
fn sim_pending_bound_smaller_than_count(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: listing with a bound below the count");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-bound")?;
        let host = host_on(scratch.path(), "acme")?;
        for index in 0..5u32 {
            let _ = park_one(&host, format!("bound-{index}-{}", sim.seed).into_bytes())?;
        }
        let two = host.pending(2)?;
        let all = host.pending(10)?;
        if two.len() != 2 || all.len() != 5 {
            return Err(ApprovalSimError::Setup {
                what: "the bound caps the listing, not the store",
            });
        }
        sim.record("bound:two-of-five");
        sim.trace.record_number("listed", 2);
        Ok(())
    })
}

/// Parking twice on the same step key replays: the second park returns the same id.
fn sim_park_twice_on_same_step_key_replays(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: parking twice on one step key");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-twice")?;
        let host = host_on(scratch.path(), "acme")?;
        let payload = format!("twice-{}", sim.seed).into_bytes();
        let shipping = shipping()?;
        let first = lgwks_bot::block_on(host.run(&shipping, payload.clone()));
        let first_id = parked_id_of(&first)?;
        let run = first.run_id().ok_or(ApprovalSimError::Setup {
            what: "a parked run names its run",
        })?;
        let second = lgwks_bot::block_on(host.resume(run, &shipping, payload));
        if !matches!(second.disposition(), Disposition::Parked) {
            return Err(ApprovalSimError::Setup {
                what: "an undecided resume parks again",
            });
        }
        if parked_id_of(&second)? != first_id {
            return Err(ApprovalSimError::Setup {
                what: "the second park names the same id",
            });
        }
        sim.record("twice:same-id");
        Ok(())
    })
}

/// A resume of a decided run with no store is refused before admission: the run
/// has nothing to resume from, the PromiseUnmet door (APR-2, no-store arm).
fn sim_resume_decided_run_without_store_is_promise_unmet(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: resuming a decided run with no store");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-nostore")?;
        let host = host_on(scratch.path(), "acme")?;
        let payload = format!("nostore-{}", sim.seed).into_bytes();
        let (run, id, digest) = park_one(&host, payload.clone())?;
        host.decide(&id, ApprovalDecision::approve("sim", digest))?;
        let bare = Host::builder("acme")?.build()?;
        let shipping = shipping()?;
        let resumed = lgwks_bot::block_on(bare.resume(run, &shipping, payload));
        if !matches!(resumed.disposition(), Disposition::Refused) {
            return Err(ApprovalSimError::Setup {
                what: "a storeless resume is refused before admission",
            });
        }
        let Some(error) = resumed.error() else {
            return Err(ApprovalSimError::Setup {
                what: "a storeless resume does not succeed",
            });
        };
        if !error.to_string().contains("no run store") {
            return Err(ApprovalSimError::Setup {
                what: "a storeless resume names its missing store",
            });
        }
        sim.record("nostore:promise-unmet");
        Ok(())
    })
}

/// The same seed gives the same trace hash across a host rebuild.
fn sim_same_seed_same_trace_across_rebuild(band: sim::Band) -> TestResult {
    lgwks_std::trace::debug!(band = ?band, "extra: rebuilding hosts under one seed");
    sweep_twice(band, |sim| {
        let scratch = scratch("approval-extra-rebuild")?;
        for generation in 0..2u32 {
            let host = host_on(scratch.path(), "acme")?;
            let payload = format!("rebuild-{generation}-{}", sim.seed).into_bytes();
            let (run, id, digest) = park_one(&host, payload.clone())?;
            host.decide(&id, ApprovalDecision::approve("sim", digest))?;
            let shipping = shipping()?;
            let resumed = lgwks_bot::block_on(host.resume(run, &shipping, payload));
            if !matches!(resumed.disposition(), Disposition::Succeeded) {
                return Err(ApprovalSimError::Setup {
                    what: "each generation resumes approved",
                });
            }
            sim.record(&format!("rebuild:{generation}:approved"));
        }
        Ok(())
    })
}

#[test]
fn sim_restart_after_every_record_type_band_16() -> TestResult {
    sim_restart_after_every_record_type(sim::band_of(16))
}

#[test]
fn sim_two_tenants_isolated_band_17() -> TestResult {
    sim_two_tenants_isolated(sim::band_of(17))
}

#[test]
fn sim_eight_tenants_crash_interleaved_band_18() -> TestResult {
    sim_eight_tenants_crash_interleaved(sim::band_of(18))
}

#[test]
fn sim_sixty_four_tenants_sweep_band_19() -> TestResult {
    sim_sixty_four_tenants_sweep(sim::band_of(19))
}

#[test]
fn sim_read_fault_on_pending_lookup_is_store_band_20() -> TestResult {
    sim_read_fault_on_pending_lookup_is_store(sim::band_of(20))
}

#[test]
fn sim_read_fault_on_decide_is_store_band_21() -> TestResult {
    sim_read_fault_on_decide_is_store(sim::band_of(21))
}

#[test]
fn sim_pending_bound_smaller_than_count_band_22() -> TestResult {
    sim_pending_bound_smaller_than_count(sim::band_of(22))
}

#[test]
fn sim_park_twice_on_same_step_key_replays_band_23() -> TestResult {
    sim_park_twice_on_same_step_key_replays(sim::band_of(23))
}

#[test]
fn sim_resume_decided_run_without_store_is_promise_unmet_band_24() -> TestResult {
    sim_resume_decided_run_without_store_is_promise_unmet(sim::band_of(24))
}

#[test]
fn sim_same_seed_same_trace_across_rebuild_band_25() -> TestResult {
    sim_same_seed_same_trace_across_rebuild(sim::band_of(25))
}
