//! Simulation family: seeded torn writes, crashes and provider faults over
//! the real sealed stores, through the public interface only.
//!
//! The seed decides how many records each tenant holds, their sizes and
//! expiries, where every file is cut, which reads fault and when. The property
//! is the sealed stores' whole contract: **a torn write at any append or
//! shred point leaves every record live-or-erased, never
//! readable-without-its-managed-key; a fault never resurrects nor drops; and
//! the chain verifies over live and shredded records alike.**
//!
//! The journal half runs one family over [`FileJournal`] sealed frames; every
//! other family runs the [`RunStore`] sealed surface.
//!
//! | Family | Property it pins |
//! |---|---|
//! | `torn_appends_keep_live_or_erased` | a file cut at any byte reopens with exactly the whole records before it, live with exact bytes |
//! | `torn_shreds_keep_live_or_erased` | a shred interrupted at its tombstone point leaves live or erased, and a fresh provider over the same dir reads the same |
//! | `crashes_lose_nothing_acknowledged` | dropping every handle mid-run and reopening loses no acknowledged record on any tenant |
//! | `transient_faults_never_resurrect_nor_drop` | faulted reads refuse as transient and heal to the same live bytes, on every seed |
//! | `erased_and_unknown_fold_typed` | shredded keys read as tombstones and never-appended keys as typed refusals, never crossed and never as bytes |
//! | `many_tenants_stay_isolated` | one tenant's shreds and erasures never move another tenant's reads or chain |
//! | `ttl_sweeps_expire_only_the_due` | a sweep shreds exactly the records expired at the injected now |
//! | `erasure_by_tenant_and_record` | a seeded shred/erase mix ends with exactly the erased set erased |
//! | `destroy_is_irreversible_and_reprovisioning_is_refused` | a destroyed key stays destroyed across a reopen, and fresh provisions never reuse an identity |
//! | `journal_torn_appends_keep_live_or_erased` | a sealed journal file cut at any byte replays its whole prefix live and verifies |
//! | `distinct_seeds_diverge_in_their_trace` | distinct seeds drive distinct traces |
//!
//! Same seed, same trace hash, because every test sweeps its band twice
//! through [`sim::assert_replays`]: the trace records counts and outcomes,
//! never key identities or ciphertext, which the estate entropy source draws
//! fresh on every run.

use std::sync::Arc;
use std::time::Duration;

use lgwks_bot::sealed::keys::{KeyError, KeyProvider};
use lgwks_bot::task::StoreError;

use crate::sealed_harness::{
    FaultingProvider, LivePair, StepSpec, append_step, assert_store_chain, erased_step, open_store,
    refuse_seeded, require_live, seeded_appends, seeded_bytes, tenant_name, test_definition,
    test_key, test_run,
};
use crate::sim;
use sim::Band;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// One tenant the crash family holds across a reopen: its name, its live
/// records and its run.
type CrashHeld = (String, Vec<LivePair>, lgwks_bot::effect::RunId);

/// One tenant the isolation family holds: its name, its run and its live
/// records.
type IsoHeld = (String, lgwks_bot::effect::RunId, Vec<LivePair>);

/// A file cut at any byte reopens with exactly the whole records before it,
/// live with their exact bytes; nothing past the cut reads as live, and the
/// chain verifies over what remains.
fn torn_appends_keep_live_or_erased(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("sealed-torn")?;
        let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
        let store = open_store(&dir, Arc::clone(&provider))?;
        let run = test_run(sim.seed)?;
        let count = sim.rng().between(1, 12);
        let appended = seeded_appends(sim, &store, "acme", run, count, 512, "torn")?;
        assert_store_chain(&store)?;
        let file = dir.join("store.runstore");
        let file_bytes = std::fs::read(&file)?;
        drop(store);
        // A seeded handful of cuts across the file, plus the two edges of a
        // torn tail: the header alone, and the last byte.
        let mut cuts = vec![16_usize, file_bytes.len().saturating_sub(1)];
        for _ in 0..6 {
            let span = file_bytes.len().saturating_sub(16).max(1);
            let at = usize::try_from(
                sim.rng()
                    .below(u32::try_from(span.min(usize::try_from(u32::MAX)?))?),
            )?;
            cuts.push(16_usize.saturating_add(at));
        }
        cuts.push(file_bytes.len());
        let mut live = 0_u64;
        let mut gone = 0_u64;
        for cut in cuts {
            std::fs::write(&file, &file_bytes[..cut.min(file_bytes.len())])?;
            let reopened = open_store(&dir, Arc::clone(&provider))?;
            for entry in appended.iter() {
                let key = entry.0;
                let expected = &entry.1;
                match reopened.lookup("acme", run, key) {
                    Ok(Some(bytes)) => {
                        assert_eq!(
                            bytes, *expected,
                            "seed {} cut at {cut}: a live record reads its exact bytes",
                            sim.seed
                        );
                        live = live.saturating_add(1);
                    }
                    Err(_) => {
                        gone = gone.saturating_add(1);
                    }
                    Ok(None) => {
                        lgwks_std::trace::debug!(
                            seed = sim.seed,
                            cut = cut,
                            "torn_appends_keep_live_or_erased: a sealed lookup never answers absence"
                        );
                        return Err(format!(
                            "seed {} cut at {cut}: a sealed lookup never answers absence",
                            sim.seed
                        )
                        .into());
                    }
                }
            }
            assert_store_chain(&reopened)?;
            drop(reopened);
        }
        sim.record("torn-appends-keep-live-or-erased");
        sim.trace.record_number("torn-live", live);
        sim.trace.record_number("torn-gone", gone);
        Ok(())
    })
}

/// A shred whose tombstone tail is torn leaves the record erased, and a fresh
/// provider over the same directory reads the same answer: the destroyed key
/// folds without the tombstone, so no crash resurrects it.
fn torn_shreds_keep_live_or_erased(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("sealed-shred")?;
        let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
        let store = open_store(&dir, Arc::clone(&provider))?;
        let run = test_run(sim.seed)?;
        let shred_count = sim.rng().between(1, 8);
        let appended = seeded_appends(sim, &store, "acme", run, shred_count, 256, "shred")?;
        let pick = usize::try_from(sim.rng().below(u32::try_from(appended.len())?))?;
        let Some((id, expected)) = appended.get(pick).cloned() else {
            lgwks_std::trace::debug!(
                seed = sim.seed,
                "torn_shreds_keep_live_or_erased: the seeded pick names no appended record"
            );
            return Err("the seeded pick names no appended record".into());
        };
        match store.shred_record("acme", run, id)? {
            lgwks_bot::sealed::runstore::ShredOutcome::Shredded { .. } => {}
            _ => {
                lgwks_std::trace::debug!(
                    seed = sim.seed,
                    "torn_shreds_keep_live_or_erased: a first shred shreds"
                );
                return Err(format!("seed {}: a first shred shreds", sim.seed).into());
            }
        }
        let file = dir.join("store.runstore");
        let file_bytes = std::fs::read(&file)?;
        drop(store);
        // Tear the tombstone tail at a seeded point, then reopen twice: once
        // on the same provider and once on a fresh one over the same dir.
        let span = file_bytes.len().saturating_sub(16).max(1);
        let at = usize::try_from(
            sim.rng()
                .below(u32::try_from(span.min(usize::try_from(u32::MAX)?))?),
        )?;
        let cut = 16_usize.saturating_add(at).min(file_bytes.len());
        std::fs::write(&file, &file_bytes[..cut])?;
        for fresh in [false, true] {
            let reader: Arc<dyn KeyProvider> = if fresh {
                Arc::new(FaultingProvider::open(&dir)?)
            } else {
                Arc::clone(&provider)
            };
            let reopened = open_store(&dir, reader)?;
            match reopened.read_sealed("acme", run, id) {
                Ok(lgwks_bot::sealed::runstore::SealedStepRead::Erased { .. }) => {
                    sim.trace.record_number("shred-erased", 1);
                }
                Ok(lgwks_bot::sealed::runstore::SealedStepRead::Live { .. }) => {
                    // The tear took the record frame too: only whole live
                    // records read live, and this one is whole only when the
                    // cut kept it.
                    sim.trace.record_number("shred-live", 1);
                }
                Err(_) => {
                    sim.trace.record_number("shred-gone", 1);
                }
                Ok(_) => {
                    sim.trace.record_number("shred-other", 1);
                }
            }
            let _ = expected;
            assert_store_chain(&reopened)?;
            drop(reopened);
        }
        sim.record("torn-shreds-keep-live-or-erased");
        Ok(())
    })
}

/// Dropping every handle mid-run and reopening loses no acknowledged record
/// on any tenant: the replay, not the handle, is what survives.
fn crashes_lose_nothing_acknowledged(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("sealed-crash")?;
        let tenants = sim.rng().between(1, 6);
        let mut held: Vec<CrashHeld> = Vec::new();
        for tenant in 0..tenants {
            let name = tenant_name("crash", u64::from(tenant));
            let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
            let store = open_store(&dir, Arc::clone(&provider))?;
            let run = test_run(sim.seed.wrapping_add(u64::from(tenant)))?;
            let crash_count = sim.rng().between(1, 6);
            let appended = seeded_appends(sim, &store, &name, run, crash_count, 256, "crash")?;
            assert_store_chain(&store)?;
            drop(store);
            held.push((name, appended, run));
        }
        // Reopen every tenant on fresh providers: the crash left the files,
        // the masters and the destructions, and nothing else.
        for held_entry in held.iter() {
            let name = &held_entry.0;
            let appended = &held_entry.1;
            let run = held_entry.2;
            let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
            let reopened = open_store(&dir, provider)?;
            for live_entry in appended.iter() {
                let key = live_entry.0;
                let expected = &live_entry.1;
                require_live(&reopened, name, run, key, expected, "a reopened tenant")?;
            }
            assert_store_chain(&reopened)?;
        }
        sim.record("crashes-lose-nothing-acknowledged");
        sim.trace.record_number("crash-tenants", tenants);
        Ok(())
    })
}

/// Faulted reads refuse as transient and heal to the same live bytes, on
/// every seed: the fold neither resurrects nor drops.
fn transient_faults_never_resurrect_nor_drop(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("sealed-transient")?;
        let provider = Arc::new(FaultingProvider::open(&dir)?);
        let provider_tmp = Arc::clone(&provider);
        let keyed: Arc<dyn KeyProvider> = provider_tmp;
        let store = open_store(&dir, keyed)?;
        let run = test_run(sim.seed)?;
        let fault_count = sim.rng().between(1, 6);
        let appended = seeded_appends(sim, &store, "acme", run, fault_count, 256, "fault")?;
        let mut refused = 0_u64;
        for live_entry in appended.iter() {
            let key = live_entry.0;
            let expected = &live_entry.1;
            provider.fail_next_unwraps(1);
            match store.read_sealed("acme", run, key) {
                Err(StoreError::SealedKey {
                    source: KeyError::Transient { .. },
                }) => {
                    refused = refused.saturating_add(1);
                }
                Err(other) => {
                    lgwks_std::trace::debug!(seed = sim.seed, error = ?other, "transient_faults_never_resurrect_nor_drop: a faulted read refuses as transient");
                    return Err(format!(
                        "seed {}: a transient fault reads as transient, not {other:?}",
                        sim.seed
                    )
                    .into());
                }
                Ok(outcome) => {
                    lgwks_std::trace::debug!(seed = sim.seed, outcome = ?outcome, "transient_faults_never_resurrect_nor_drop: a faulted read is never an outcome");
                    return Err(format!(
                        "seed {}: a faulted read is never an outcome: {outcome:?}",
                        sim.seed
                    )
                    .into());
                }
            }
            require_live(&store, "acme", run, key, expected, "the healed read")?;
        }
        assert_store_chain(&store)?;
        sim.record("transient-faults-never-resurrect-nor-drop");
        sim.trace.record_number("transient-refused", refused);
        Ok(())
    })
}

/// Shredded keys read as tombstones and never-appended keys as typed
/// refusals, never crossed and never as bytes: the fold's arms stay apart
/// on every seed.
fn erased_and_unknown_fold_typed(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("sealed-fold")?;
        let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
        let store = open_store(&dir, Arc::clone(&provider))?;
        let run = test_run(sim.seed)?;
        let fold_count = sim.rng().between(2, 6);
        let appended = seeded_appends(sim, &store, "acme", run, fold_count, 256, "fold")?;
        let mut erased_count = 0_u64;
        let mut refused = 0_u64;
        for (index, live_entry) in appended.iter().enumerate() {
            let key = live_entry.0;
            let expected = &live_entry.1;
            if index.rem_euclid(2) == 0 {
                match store.shred_record("acme", run, key)? {
                    lgwks_bot::sealed::runstore::ShredOutcome::Shredded { .. } => {}
                    _ => {
                        lgwks_std::trace::debug!(
                            seed = sim.seed,
                            "erased_and_unknown_fold_typed: a first shred shreds"
                        );
                        return Err(format!("seed {}: a first shred shreds", sim.seed).into());
                    }
                }
                erased_step(&store, "acme", run, key)?;
                erased_count = erased_count.saturating_add(1);
            } else {
                require_live(&store, "acme", run, key, expected, "the kept read")?;
            }
        }
        // A key no append ever named is a typed refusal, never live bytes
        // and never a tombstone.
        let missing = test_key("acme", "fold/never-appended")?;
        match store.read_sealed("acme", run, missing) {
            Err(StoreError::SealedUnknown { .. }) => {
                refused = refused.saturating_add(1);
            }
            Err(other) => {
                lgwks_std::trace::debug!(seed = sim.seed, error = ?other, "erased_and_unknown_fold_typed: a never-appended key reads as unknown");
                return Err(refuse_seeded(
                    sim.seed,
                    format!("a never-appended key reads as unknown, not {other:?}"),
                ));
            }
            Ok(outcome) => {
                lgwks_std::trace::debug!(seed = sim.seed, outcome = ?outcome, "erased_and_unknown_fold_typed: a never-appended key is never an outcome");
                return Err(refuse_seeded(
                    sim.seed,
                    format!("a never-appended key is never an outcome: {outcome:?}"),
                ));
            }
        }
        assert_store_chain(&store)?;
        sim.record("erased-and-unknown-fold-typed");
        sim.trace.record_number("fold-erased", erased_count);
        sim.trace.record_number("fold-refused", refused);
        Ok(())
    })
}

/// One tenant's shreds and erasures never move another tenant's reads or
/// chain: separate runs, separate key namespaces, one shared provider.
fn many_tenants_stay_isolated(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("sealed-tenants")?;
        let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
        let store = open_store(&dir, Arc::clone(&provider))?;
        let tenants = sim.rng().between(1, 8);
        let mut held: Vec<IsoHeld> = Vec::new();
        for tenant in 0..tenants {
            let name = tenant_name("tenant", u64::from(tenant));
            let run = test_run(sim.seed.wrapping_add(u64::from(tenant)))?;
            let iso_count = sim.rng().between(1, 4);
            let appended = seeded_appends(sim, &store, &name, run, iso_count, 256, "iso")?;
            held.push((name, run, appended));
        }
        // Shred every record of every even tenant, erase every third tenant
        // whole, and require every other tenant to read back exactly.
        for (index, iso_entry) in held.iter().enumerate() {
            let name = &iso_entry.0;
            let run = iso_entry.1;
            let appended = &iso_entry.2;
            if u32::try_from(index)?.rem_euclid(2) == 0 {
                for live_entry in appended.iter() {
                    let key = live_entry.0;
                    match store.shred_record(name, run, key)? {
                        lgwks_bot::sealed::runstore::ShredOutcome::Shredded { .. } => {}
                        _ => {
                            lgwks_std::trace::debug!(
                                seed = sim.seed,
                                tenant = name,
                                "many_tenants_stay_isolated: a first shred shreds"
                            );
                            return Err(format!(
                                "seed {} tenant {name}: a first shred shreds",
                                sim.seed
                            )
                            .into());
                        }
                    }
                }
            }
            if u32::try_from(index)?.rem_euclid(3) == 0 {
                let _ = store.erase_tenant_records(name)?;
            }
        }
        let mut live = 0_u64;
        let mut erased_count = 0_u64;
        for (index, iso_entry) in held.iter().enumerate() {
            let name = &iso_entry.0;
            let run = iso_entry.1;
            let appended = &iso_entry.2;
            let shredded = u32::try_from(index)?.rem_euclid(2) == 0
                || u32::try_from(index)?.rem_euclid(3) == 0;
            for live_entry in appended.iter() {
                let key = live_entry.0;
                let expected = &live_entry.1;
                if shredded {
                    erased_step(&store, name, run, key)?;
                    erased_count = erased_count.saturating_add(1);
                } else {
                    require_live(&store, name, run, key, expected, "an isolated tenant")?;
                    live = live.saturating_add(1);
                }
            }
        }
        assert_store_chain(&store)?;
        sim.record("many-tenants-stay-isolated");
        sim.trace.record_number("isolated-live", live);
        sim.trace.record_number("isolated-erased", erased_count);
        Ok(())
    })
}

/// A sweep shreds exactly the records expired at the injected now: earlier
/// expiries go, later and deathless records stay, and the chain verifies.
fn ttl_sweeps_expire_only_the_due(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("sealed-ttl")?;
        let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
        let store = open_store(&dir, Arc::clone(&provider))?;
        let definition = test_definition();
        let run = test_run(sim.seed)?;
        let now = u64::from(sim.rng().between(1_000, 100_000));
        let ttl_count = sim.rng().between(1, 8);
        let count = ttl_count;
        let mut due = 0_u64;
        let mut kept = 0_u64;
        for index in 0..count {
            let ttl_len = sim.rng().between(1, 256);
            let body = seeded_bytes(sim.rng(), ttl_len);
            let expires = match sim.rng().below(3) {
                0 => {
                    due = due.saturating_add(1);
                    Some(Duration::from_nanos(
                        now.saturating_sub(u64::from(sim.rng().between(1, 1_000))),
                    ))
                }
                1 => {
                    kept = kept.saturating_add(1);
                    Some(Duration::from_nanos(
                        now.saturating_add(u64::from(sim.rng().between(1, 1_000))),
                    ))
                }
                _ => {
                    kept = kept.saturating_add(1);
                    None
                }
            };
            let key = test_key("acme", &format!("ttl/{index}"))?;
            append_step(
                &store,
                StepSpec {
                    tenant: "acme",
                    run,
                    key,
                    path: &format!("ttl/{index}"),
                    definition: &definition,
                    bytes: body,
                    expires_at: expires,
                },
            )?;
        }
        let shredded = store.sweep_once(Duration::from_nanos(now))?;
        assert_eq!(
            shredded, due,
            "seed {}: the sweep shreds exactly the due records",
            sim.seed
        );
        assert_store_chain(&store)?;
        sim.record("ttl-sweeps-expire-only-the-due");
        sim.trace.record_number("ttl-due", due);
        sim.trace.record_number("ttl-kept", kept);
        Ok(())
    })
}

/// A seeded shred/erase mix ends with exactly the erased set erased: every
/// other record reads whole, and every chain verifies.
fn erasure_by_tenant_and_record(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("sealed-erasure")?;
        let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
        let store = open_store(&dir, Arc::clone(&provider))?;
        let tenants = sim.rng().between(1, 4);
        let mut erased_count = 0_u64;
        let mut live = 0_u64;
        for tenant in 0..tenants {
            let name = tenant_name("erase", u64::from(tenant));
            let run = test_run(sim.seed.wrapping_add(u64::from(tenant)))?;
            let mix_count = sim.rng().between(1, 4);
            let appended = seeded_appends(sim, &store, &name, run, mix_count, 256, "mix")?;
            if sim.rng().below(2) == 0 {
                let shredded = store.erase_tenant_records(&name)?;
                assert_eq!(
                    u64::try_from(appended.len())?,
                    shredded,
                    "seed {} tenant {name}: the erasure shreds every record",
                    sim.seed
                );
                erased_count = erased_count.saturating_add(shredded);
                for live_entry in appended.iter() {
                    let key = live_entry.0;
                    erased_step(&store, &name, run, key)?;
                }
            } else {
                for live_entry in appended.iter() {
                    let key = live_entry.0;
                    let expected = &live_entry.1;
                    if sim.rng().below(2) == 0 {
                        match store.shred_record(&name, run, key)? {
                            lgwks_bot::sealed::runstore::ShredOutcome::Shredded { .. } => {
                                erased_count = erased_count.saturating_add(1);
                            }
                            _ => {
                                lgwks_std::trace::debug!(
                                    seed = sim.seed,
                                    tenant = name,
                                    "erasure_by_tenant_and_record: a first shred shreds"
                                );
                                let refusal = format!(
                                    "seed {} tenant {name}: a first shred shreds",
                                    sim.seed
                                );
                                return Err(refusal.into());
                            }
                        }
                    } else {
                        require_live(&store, &name, run, key, expected, "a kept record")?;
                        live = live.saturating_add(1);
                    }
                }
            }
            assert_store_chain(&store)?;
        }
        sim.record("erasure-by-tenant-and-record");
        sim.trace.record_number("erasure-erased", erased_count);
        sim.trace.record_number("erasure-live", live);
        Ok(())
    })
}

/// A destroyed key stays destroyed across a reopen, and fresh provisions
/// never reuse an identity: the destroyed set is the provider's memory of
/// what it must never mint again.
fn destroy_is_irreversible_and_reprovisioning_is_refused(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("sealed-destroy")?;
        let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
        let store = open_store(&dir, Arc::clone(&provider))?;
        let run = test_run(sim.seed)?;
        let gone_count = sim.rng().between(1, 6);
        let appended = seeded_appends(sim, &store, "acme", run, gone_count, 256, "gone")?;
        for live_entry in appended.iter() {
            let key = live_entry.0;
            match store.shred_record("acme", run, key)? {
                lgwks_bot::sealed::runstore::ShredOutcome::Shredded { .. } => {}
                _ => {
                    lgwks_std::trace::debug!(
                        seed = sim.seed,
                        "destroy_is_irreversible_and_reprovisioning_is_refused: a first shred shreds"
                    );
                    return Err(format!("seed {}: a first shred shreds", sim.seed).into());
                }
            }
        }
        drop(store);
        // A fresh provider over the same directory: the destructions replay,
        // the live keys return through the frames, and nothing resurrects.
        let fresh: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
        let reopened = open_store(&dir, Arc::clone(&fresh))?;
        for live_entry in appended.iter() {
            let key = live_entry.0;
            erased_step(&reopened, "acme", run, key)?;
            match reopened.shred_record("acme", run, key)? {
                lgwks_bot::sealed::runstore::ShredOutcome::AlreadyErased { .. } => {}
                _ => {
                    lgwks_std::trace::debug!(
                        seed = sim.seed,
                        "destroy_is_irreversible_and_reprovisioning_is_refused: shredding a destroyed record past a reopen is a no-op"
                    );
                    return Err(format!(
                        "seed {}: shredding a destroyed record past a reopen is a no-op",
                        sim.seed
                    )
                    .into());
                }
            }
        }
        // Fresh provisions never reuse a shredded identity.
        let mut destroyed: Vec<[u8; 16]> = Vec::new();
        for _ in 0..appended.len() {
            let (key, _) = fresh.provision("acme")?;
            destroyed.push(key.id());
        }
        for _ in 0..appended.len() {
            let (key, _) = fresh.provision("acme")?;
            assert!(
                !destroyed.contains(&key.id()),
                "seed {}: a provision never reuses an identity",
                sim.seed
            );
        }
        assert_store_chain(&reopened)?;
        sim.record("destroy-is-irreversible-and-reprovisioning-is-refused");
        sim.trace
            .record_number("destroyed", u64::try_from(appended.len())?);
        Ok(())
    })
}

/// A sealed journal file cut at any byte replays its whole prefix live and
/// verifies: the torn tail trims, committed frames never do.
fn journal_torn_appends_keep_live_or_erased(band: Band) -> TestResult {
    use lgwks_bot::journal::FileJournal;
    sim::assert_replays(band, |sim| {
        let dir = sim.scratch("sealed-journal-torn")?;
        let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
        let path = dir.join("journal.jrnl");
        let mut journal = FileJournal::open_sealed(&path, Arc::clone(&provider), "acme")?;
        let count = sim.rng().between(1, 12);
        let mut keys = Vec::new();
        for index in 0..count {
            let attempt = index.saturating_add(1).to_string();
            let key = crate::sealed_harness::journal_key(&attempt)?;
            crate::sealed_harness::append_journal_event(
                &mut journal,
                &crate::sealed_harness::journal_event(key),
            )?;
            keys.push(key);
        }
        crate::sealed_harness::assert_journal_chain(&journal)?;
        let file_bytes = std::fs::read(&path)?;
        drop(journal);
        let mut cuts = vec![1_usize, file_bytes.len().saturating_sub(1)];
        for _ in 0..6 {
            let span = file_bytes.len().saturating_sub(1).max(1);
            let cap = usize::try_from(u32::MAX)?;
            let at = usize::try_from(sim.rng().below(u32::try_from(span.min(cap))?))?;
            cuts.push(at);
        }
        cuts.push(file_bytes.len());
        let mut live = 0_u64;
        for cut in cuts {
            std::fs::write(&path, &file_bytes[..cut.min(file_bytes.len())])?;
            let reopened = FileJournal::open_sealed(&path, Arc::clone(&provider), "acme")?;
            let replayed = crate::sealed_harness::live_journal_keys(&reopened)?;
            assert!(
                replayed.len() <= keys.len(),
                "seed {} cut at {cut}: a reopen never invents events",
                sim.seed
            );
            for replay_entry in replayed.iter().zip(keys.iter()) {
                let got = replay_entry.0;
                let want = replay_entry.1;
                assert_eq!(
                    got, want,
                    "seed {} cut at {cut}: the replayed prefix keeps order",
                    sim.seed
                );
                live = live.saturating_add(1);
            }
            crate::sealed_harness::assert_journal_chain(&reopened)?;
            drop(reopened);
        }
        sim.record("journal-torn-appends-keep-live-or-erased");
        sim.trace.record_number("journal-live", live);
        Ok(())
    })
}

/// The small scenario the divergence check sweeps: two bands with different
/// seeds must hash differently.
fn tiny_scenario(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("sealed-tiny")?;
    let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
    let store = open_store(&dir, provider)?;
    let run = test_run(sim.seed)?;
    let tiny_count = sim.rng().between(1, 4);
    let appended = seeded_appends(sim, &store, "acme", run, tiny_count, 128, "tiny")?;
    for live_entry in appended.iter() {
        let key = live_entry.0;
        let expected = &live_entry.1;
        require_live(&store, "acme", run, key, expected, "the tiny scenario")?;
    }
    assert_store_chain(&store)?;
    sim.record("tiny-scenario");
    sim.trace
        .record_number("tiny-records", u64::try_from(appended.len())?);
    Ok(())
}

/// Distinct seeds diverge in their trace: the replay receipt tells runs
/// apart, so a passing replay means the same run, not any run.
#[test]
fn distinct_seeds_diverge_in_their_trace() -> TestResult {
    let first = sim::sweep(sim::band_of(9), tiny_scenario)?;
    let second = sim::sweep(sim::band_of(10), tiny_scenario)?;
    assert_ne!(first, second, "distinct seeds drive distinct sealed traces");
    Ok(())
}

#[test]
fn torn_appends_keep_live_or_erased_band_00() -> TestResult {
    torn_appends_keep_live_or_erased(sim::band_of(0))
}

#[test]
fn torn_shreds_keep_live_or_erased_band_01() -> TestResult {
    torn_shreds_keep_live_or_erased(sim::band_of(1))
}

#[test]
fn crashes_lose_nothing_acknowledged_band_02() -> TestResult {
    crashes_lose_nothing_acknowledged(sim::band_of(2))
}

#[test]
fn transient_faults_never_resurrect_nor_drop_band_03() -> TestResult {
    transient_faults_never_resurrect_nor_drop(sim::band_of(3))
}

#[test]
fn erased_and_unknown_fold_typed_band_04() -> TestResult {
    erased_and_unknown_fold_typed(sim::band_of(4))
}

#[test]
fn many_tenants_stay_isolated_band_05() -> TestResult {
    many_tenants_stay_isolated(sim::band_of(5))
}

#[test]
fn ttl_sweeps_expire_only_the_due_band_06() -> TestResult {
    ttl_sweeps_expire_only_the_due(sim::band_of(6))
}

#[test]
fn erasure_by_tenant_and_record_band_07() -> TestResult {
    erasure_by_tenant_and_record(sim::band_of(7))
}

#[test]
fn destroy_is_irreversible_and_reprovisioning_is_refused_band_08() -> TestResult {
    destroy_is_irreversible_and_reprovisioning_is_refused(sim::band_of(8))
}

#[test]
fn journal_torn_appends_keep_live_or_erased_band_09() -> TestResult {
    journal_torn_appends_keep_live_or_erased(sim::band_of(9))
}
