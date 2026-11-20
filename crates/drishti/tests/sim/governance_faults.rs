//! Seeded governance faults: corrupt entries and wild queries must all fail
//! closed with typed errors, and the store must stay consistent.
//!
//! Same seed replays the same trace hash over error kinds.

use drishti::native_host::governance::{GovernanceError, GovernanceQuery, GovernanceStore};
use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};

use crate::rng::{SimRng, TraceHash};

/// Seeded possibly-corrupt entry.
fn corrupt_entry(random: &mut SimRng, index: usize) -> GovernanceEntry {
    let signal = if random.below(4) == 0 {
        String::new()
    } else {
        format!("signal-{index}-{}", random.below(3))
    };
    let recorded = match random.below(5) {
        0 => String::from("not a timestamp"),
        1 => String::from("2026-13-99T99:99:99Z"),
        _ => format!("2026-11-03T00:{:02}:00Z", random.below(60)),
    };
    let mut entry = GovernanceEntry::new(
        String::from("session"),
        format!("entry-{index}"),
        String::from("work"),
        signal,
        String::from("tick"),
        String::from("sim fault"),
        Attribution::new(
            AttributionKind::Widget,
            String::from("widget.sim"),
            None,
            None,
        ),
    );
    entry.set_recorded_at(recorded);
    entry.set_tenant_id(String::from(crate::fixtures::TENANT));
    entry
}

/// Run a built query (or hash why it was refused) and hash the rows it read.
pub(crate) fn mix_query(
    store: &GovernanceStore,
    query: Result<GovernanceQuery, GovernanceError>,
    trace: &mut TraceHash,
) {
    let outcome = query.and_then(|built| store.query(&built));
    match outcome {
        Ok(rows) => {
            trace.mix_u64(10);
            trace.mix_len(rows.len());
        }
        Err(error) => trace.mix_str(&format!("{error:?}")),
    }
}

/// Run one fault campaign from `seed` and hash the outcomes.
fn run_faults(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    for index in 0..200 {
        match store.append(&corrupt_entry(&mut random, index)) {
            Ok(()) => trace.mix_u64(1),
            Err(error) => trace.mix_str(&format!("{error:?}")),
        }
        // Wild queries: empty sessions, inverted and over-wide windows.
        let session = if random.below(6) == 0 {
            String::new()
        } else {
            String::from("session")
        };
        let from = random.below(200_000_000);
        let width = random.below(200_000_000);
        let to = if random.below(4) == 0 {
            from.saturating_sub(width).min(from)
        } else {
            from.saturating_add(width)
        };
        let tenant = String::from(crate::fixtures::TENANT);
        let query = GovernanceQuery::new(tenant, session, from, to, None, None);
        mix_query(&store, query, &mut trace);
    }
    trace.mix_u64(
        store
            .count(crate::fixtures::TENANT, "session")
            .map_err(|error| format!("{error}"))?,
    );
    Ok(trace.finish())
}

/// Same seed replays the same hash.
#[test]
fn sim_governance_faults_replay_identically() -> Result<(), String> {
    for seed in [41_u64, 53, 607] {
        if run_faults(seed)? != run_faults(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_faults(41)? == run_faults(42)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}

/// Run one append-count campaign from `seed`: seeded corrupt entries resolve
/// to success or a typed error, and the store count always equals the success
/// count.
fn run_counts(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut appended = 0_u64;
    for index in 0..200 {
        match store.append(&corrupt_entry(&mut random, index)) {
            Ok(()) => {
                appended = appended.saturating_add(1);
                trace.mix_u64(1);
            }
            Err(error) => trace.mix_str(&format!("{error:?}")),
        }
    }
    let counted = store
        .count(crate::fixtures::TENANT, "session")
        .map_err(|error| format!("{error}"))?;
    crate::fixtures::ensure(counted == appended, || {
        format!("store count must equal successes, counted {counted} appended {appended}")
    })?;
    trace.mix_u64(counted);
    Ok(trace.finish())
}

/// Same seed replays the same count hash with identical success totals.
#[test]
fn sim_governance_fault_counts_replay_identically() -> Result<(), String> {
    for seed in [43_u64, 59, 911] {
        if run_counts(seed)? != run_counts(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_counts(43)? == run_counts(44)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}
