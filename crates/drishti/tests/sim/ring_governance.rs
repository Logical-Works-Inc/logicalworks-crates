//! Seeded governance interleaving: appends, queries, and tape reads across
//! sessions with duplicate, malformed, and unbounded faults injected.
//!
//! Same seed replays the same trace hash over counts, query lengths, and
//! error kinds.

use drishti::native_host::broker::Scope;
use drishti::native_host::governance::{GovernanceQuery, GovernanceStore};
use drishti::native_host::telemetry::TapeReader;

use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// Seeded timestamp within the first day.
fn stamp(random: &mut SimRng) -> String {
    let hour = random.below(24);
    let minute = random.below(60);
    format!("2026-11-03T{hour:02}:{minute:02}:00Z")
}

/// Run one interleaving from `seed` and hash the trace.
fn run_interleaving(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    let reader = TapeReader::new(store.clone());
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let sessions = ["alpha", "beta", "gamma"];
    let kinds = ["intent.find", "intent.pin", "forms.submitted", "tick"];
    for step in 0..250_u64 {
        let session = random
            .pick_str(&sessions)
            .ok_or_else(|| String::from("sessions empty"))?;
        let kind = random
            .pick_str(&kinds)
            .ok_or_else(|| String::from("kinds empty"))?;
        match random.below(10) {
            0..=5 => {
                // Append, sometimes duplicating or corrupting.
                let signal = if random.below(8) == 0 && step > 0 {
                    format!("signal-{}", random.below(step))
                } else {
                    format!("signal-{step}")
                };
                let recorded = if random.below(20) == 0 {
                    String::from("not a timestamp")
                } else {
                    stamp(&mut random)
                };
                let entry = crate::fixtures::governance_entry(&signal, session, kind, &recorded);
                match store.append(&entry) {
                    Ok(()) => trace.mix_u64(1),
                    Err(error) => trace.mix_str(&format!("{error:?}")),
                }
            }
            6..=7 => {
                // Bounded query with optional glob.
                let from = random.below(86_400_000);
                let width = random.below(86_400_000);
                let to = from.saturating_add(width).min(86_400_000);
                let glob = if random.below(2) == 0 {
                    Some(String::from("intent.*"))
                } else {
                    None
                };
                let tenant = String::from(crate::fixtures::TENANT);
                let query =
                    GovernanceQuery::new(tenant, String::from(session), from, to, None, glob);
                crate::governance_faults::mix_query(&store, query, &mut trace);
            }
            _ => {
                // Tape read with a sometimes absurd window.
                let window = if random.below(5) == 0 { 100.0 } else { 1.0 };
                let scope = Scope::new(crate::fixtures::TENANT, session);
                match reader.recent_entries(&scope, 10, window, 86_400_000) {
                    Ok(rows) => {
                        trace.mix_u64(20);
                        trace.mix_len(rows.len());
                    }
                    Err(error) => trace.mix_str(&format!("{error:?}")),
                }
            }
        }
    }
    for session in sessions {
        trace.mix_u64(
            store
                .count(crate::fixtures::TENANT, session)
                .map_err(|error| format!("{error}"))?,
        );
    }
    Ok(trace.finish())
}

/// Same seed replays the same hash.
#[test]
fn sim_governance_interleaving_replays_identically() -> Result<(), String> {
    for seed in [5_u64, 21, 333] {
        let (first, second) = (run_interleaving(seed)?, run_interleaving(seed)?);
        ensure(first == second, || format!("seed {seed} diverged"))?;
    }
    let (five, six) = (run_interleaving(5)?, run_interleaving(6)?);
    ensure(five != six, || String::from("distinct seeds must diverge"))
}

/// Run one tape-window campaign from `seed`: valid entries pre-populate two
/// sessions, then seeded tape reads with absurd windows always resolve, stay
/// within their limit, and keep per-session counts consistent with appends.
fn run_tape_windows(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    let reader = TapeReader::new(store.clone());
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let sessions = ["alpha", "beta"];
    let mut per_session = [0_usize, 0_usize];
    for step in 0..60 {
        let position = random.below_len(sessions.len());
        let session = *sessions
            .get(position)
            .ok_or_else(|| String::from("sessions empty"))?;
        let entry = crate::fixtures::governance_entry(
            &format!("tape-{step}"),
            session,
            "tick",
            &stamp(&mut random),
        );
        match store.append(&entry) {
            Ok(()) => {
                per_session[position] = per_session[position].saturating_add(1);
                trace.mix_u64(1);
            }
            Err(error) => trace.mix_str(&format!("{error:?}")),
        }
    }
    let windows = [1.0_f64, 100.0, 0.0, -1.0, f64::NAN];
    for _ in 0..200 {
        let session = random
            .pick_str(&sessions)
            .ok_or_else(|| String::from("sessions empty"))?;
        let window = windows
            .get(random.below_len(windows.len()))
            .ok_or_else(|| String::from("windows empty"))?;
        let limit = random.below_len(10);
        let scope = Scope::new(crate::fixtures::TENANT, session);
        match reader.recent_entries(&scope, limit, *window, 86_400_000) {
            Ok(rows) => {
                ensure(rows.len() <= limit, || {
                    String::from("tape read must honor its limit")
                })?;
                trace.mix_u64(10);
                trace.mix_len(rows.len());
            }
            Err(error) => trace.mix_str(&format!("{error:?}")),
        }
    }
    for (position, session) in sessions.iter().enumerate() {
        let counted = store
            .count(crate::fixtures::TENANT, session)
            .map_err(|error| format!("{error}"))?;
        let want = u64::try_from(per_session[position])
            .map_err(|error| format!("count overflow: {error}"))?;
        ensure(counted == want, || {
            format!("count must match appends for {session}")
        })?;
        trace.mix_u64(counted);
    }
    Ok(trace.finish())
}

/// Same seed replays the same tape-window hash.
#[test]
fn sim_ring_tape_window_replays_identically() -> Result<(), String> {
    for seed in [17_u64, 29, 1_111] {
        let (first, second) = (run_tape_windows(seed)?, run_tape_windows(seed)?);
        ensure(first == second, || format!("seed {seed} diverged"))?;
    }
    let (near, far) = (run_tape_windows(17)?, run_tape_windows(18)?);
    ensure(near != far, || String::from("distinct seeds must diverge"))
}
