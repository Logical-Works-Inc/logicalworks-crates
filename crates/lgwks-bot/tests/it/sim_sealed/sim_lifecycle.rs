//! The lifecycle families of the sealed store: a destroyed key never comes
//! back, and a sealed journal cut at any byte replays its committed prefix.

use std::sync::Arc;

use lgwks_bot::sealed::keys::KeyProvider;

use super::TestResult;
use crate::sealed_harness::{
    FaultingProvider, assert_store_chain, erased_step, open_store, seeded_appends, test_run,
};
use crate::sim::{self, Band};

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
                lgwks_bot::task::ShredOutcome::Shredded { .. } => {}
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
                lgwks_bot::task::ShredOutcome::AlreadyErased { .. } => {}
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
        use lgwks_bot::sealed::conformance::mint_ids;
        let destroyed: Vec<[u8; 16]> = mint_ids(fresh.as_ref(), "acme", appended.len())?
            .iter()
            .map(lgwks_bot::sealed::keys::KeyId::id)
            .collect();
        for key in mint_ids(fresh.as_ref(), "acme", appended.len())? {
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
            let kept = file_bytes
                .get(..cut.min(file_bytes.len()))
                .ok_or("the cut lies inside the file")?;
            std::fs::write(&path, kept)?;
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

#[test]
fn destroy_is_irreversible_and_reprovisioning_is_refused_band_08() -> TestResult {
    destroy_is_irreversible_and_reprovisioning_is_refused(sim::band_of(8))
}

#[test]
fn journal_torn_appends_keep_live_or_erased_band_09() -> TestResult {
    journal_torn_appends_keep_live_or_erased(sim::band_of(9))
}
