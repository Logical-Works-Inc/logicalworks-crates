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

use lgwks_bot::sealed::keys::KeyProvider;
use lgwks_bot::sealed::runstore::ShredOutcome;

use crate::sealed_harness::{
    FaultingProvider, assert_store_chain, erased_step, open_store, refuse_seeded, require_live,
    seeded_appends, test_run,
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
