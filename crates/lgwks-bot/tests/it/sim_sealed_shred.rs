//! Simulation family: repeated shreds on one live handle.
//!
//! NIST SP 800-88 crypto-erase is idempotent. On one live handle, the seed
//! picks which records are shredded and how many times; each is shredded
//! once, every repeat answers already-erased, and the untouched records stay
//! live with their exact bytes under a chain that still verifies. The
//! restart half of the property lives in `sim_sealed`'s
//! `destroy_is_irreversible_and_reprovisioning_is_refused`; this family pins
//! the same-handle half, where no reopen replays the destruction log.

use std::sync::Arc;
use std::time::Duration;

use lgwks_bot::sealed::keys::KeyProvider;
use lgwks_bot::task::{ShredOutcome, StoreError};

use crate::sealed_harness::{
    FaultingProvider, StepSpec, append_step, assert_store_chain, erased_step, open_store,
    refuse_seeded, require_live, seeded_appends, test_definition, test_key, test_run,
};
use crate::sim;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn repeated_shreds_on_one_handle_erase_once_band_10() -> TestResult {
    sim::assert_replays(sim::band_of(10), |sim| {
        let dir = sim.scratch("sealed-repeat-shred")?;
        let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
        let store = open_store(&dir, provider)?;
        let run = test_run(sim.seed)?;
        let count = sim.rng().between(2, 9);
        let appended = seeded_appends(sim, &store, "acme", run, count, 192, "repeat")?;
        let mut repeats_answered = 0_u64;
        for live_entry in &appended {
            let key = &live_entry.0;
            if sim.rng().below(2) == 0 {
                let bytes = &live_entry.1;
                require_live(&store, "acme", run, *key, bytes, "an unshredded record")?;
                continue;
            }
            let times = sim.rng().between(2, 5);
            for attempt in 0..times {
                match (attempt == 0, store.shred_record("acme", run, *key)?) {
                    (true, ShredOutcome::Shredded { .. }) => {}
                    (false, ShredOutcome::AlreadyErased { .. }) => {
                        repeats_answered = repeats_answered.saturating_add(1);
                    }
                    (_, other) => {
                        lgwks_std::trace::debug!(
                            seed = sim.seed,
                            attempt,
                            "repeated_shreds: a shred answered out of order"
                        );
                        return Err(refuse_seeded(
                            sim.seed,
                            format!("shred attempt {attempt} answered {other:?}"),
                        ));
                    }
                }
            }
            erased_step(&store, "acme", run, *key)?;
        }
        assert_store_chain(&store)?;
        sim.record("repeated-shreds-on-one-handle-erase-once");
        sim.trace
            .record_number("repeats-answered", repeats_answered);
        Ok(())
    })
}

/// GDPR Art. 17 and the TTL door of `sealed_shred`: on one handle holding two
/// tenants, the sweep at `now` shreds exactly the steps due at or before `now`
/// (step 0 of each tenant expires on `now` itself), then the tenant erasure
/// shreds only acme's still-live steps and globex's stay live with their bytes.
#[test]
fn sealed_shred_erase_and_sweep_take_only_their_own_steps_band_11() -> TestResult {
    sim::assert_replays(sim::band_of(11), |sim| {
        let dir = sim.scratch("sealed-erase-sweep")?;
        let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
        let store = open_store(&dir, provider)?;
        let definition = test_definition();
        let now = Duration::from_secs(sim.rng().between(10, 100).into());
        let mut live = Vec::new();
        let mut due = 0_u64;
        for (offset, tenant) in [(0_u64, "acme"), (1_u64, "globex")] {
            let run = test_run(sim.seed.wrapping_add(offset))?;
            let steps = sim.rng().between(2, 7);
            for step in 0..steps {
                let path = format!("ttl-{step}");
                let key = test_key(tenant, &path)?;
                let expires_at = if step == 0 {
                    now
                } else {
                    Duration::from_secs(sim.rng().between(1, 200).into())
                };
                append_step(
                    &store,
                    StepSpec {
                        tenant,
                        run,
                        key,
                        path: &path,
                        definition: &definition,
                        bytes: path.clone().into_bytes(),
                        expires_at: Some(expires_at),
                    },
                )?;
                if expires_at <= now {
                    due = due.saturating_add(1);
                } else {
                    live.push((tenant, run, key, path));
                }
            }
        }
        let swept = store.sweep_once(now)?;
        if swept != due {
            return Err(refuse_seeded(
                sim.seed,
                format!("the sweep at {now:?} shredded {swept}, {due} were due"),
            ));
        }
        let acme_live = u64::try_from(live.iter().filter(|entry| entry.0 == "acme").count())?;
        let erased = store.erase_tenant_records("acme")?;
        if erased != acme_live {
            return Err(refuse_seeded(
                sim.seed,
                format!("acme's erasure shredded {erased}, {acme_live} were live"),
            ));
        }
        for entry in &live {
            if entry.0 == "acme" {
                erased_step(&store, entry.0, entry.1, entry.2)?;
            } else {
                let bytes = entry.3.as_bytes();
                require_live(
                    &store,
                    entry.0,
                    entry.1,
                    entry.2,
                    bytes,
                    "globex's live step",
                )?;
            }
        }
        assert_store_chain(&store)?;
        sim.record("sealed-shred-erase-and-sweep-take-only-their-own-steps");
        sim.trace.record_number("due", due);
        Ok(())
    })
}

/// CWE-639 (authorization bypass through a user-controlled key) at the shred
/// fence of `sealed_impl`: a foreign tenant naming the owner's run and key is
/// refused as `ForeignTenant` and the step stays live; the owner then shreds it.
#[test]
fn sealed_impl_shred_fence_refuses_a_foreign_tenant_band_12() -> TestResult {
    sim::assert_replays(sim::band_of(12), |sim| {
        let dir = sim.scratch("sealed-shred-fence")?;
        let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&dir)?);
        let store = open_store(&dir, provider)?;
        let run = test_run(sim.seed)?;
        let path = format!("fenced-{}", sim.rng().between(0, 1000));
        let key = test_key("acme", &path)?;
        append_step(
            &store,
            StepSpec {
                tenant: "acme",
                run,
                key,
                path: &path,
                definition: &test_definition(),
                bytes: path.clone().into_bytes(),
                expires_at: None,
            },
        )?;
        match store.shred_record("globex", run, key) {
            Err(StoreError::ForeignTenant { .. }) => {}
            other => {
                return Err(refuse_seeded(
                    sim.seed,
                    format!("a foreign shred answered {other:?}"),
                ));
            }
        }
        require_live(
            &store,
            "acme",
            run,
            key,
            path.as_bytes(),
            "acme's step after a foreign shred",
        )?;
        match store.shred_record("acme", run, key)? {
            ShredOutcome::Shredded { .. } => {}
            other => {
                return Err(refuse_seeded(
                    sim.seed,
                    format!("the owner's shred answered {other:?}"),
                ));
            }
        }
        erased_step(&store, "acme", run, key)?;
        // The run is now wholly erased: the fence still holds on the burn set.
        match store.shred_record("acme", run, key)? {
            ShredOutcome::AlreadyErased { .. } => {}
            other => {
                return Err(refuse_seeded(
                    sim.seed,
                    format!("the owner's second shred answered {other:?}"),
                ));
            }
        }
        match store.shred_record("globex", run, key) {
            Err(StoreError::ForeignTenant { .. }) => {}
            other => {
                return Err(refuse_seeded(
                    sim.seed,
                    format!("a foreign shred of an erased run answered {other:?}"),
                ));
            }
        }
        sim.record("sealed-impl-shred-fence-refuses-a-foreign-tenant");
        Ok(())
    })
}
