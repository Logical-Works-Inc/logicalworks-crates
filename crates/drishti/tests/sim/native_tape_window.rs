//! Tape-window sims: the replay window is a number of hours that may carry a
//! fraction, and the rows a read returns are exactly the rows inside it, as
//! judged by a reference that does its arithmetic on whole milliseconds.

use drishti::native_host::broker::Scope;
use drishti::native_host::governance::GovernanceStore;
use drishti::native_host::telemetry::TapeReader;

use crate::fixtures::governance_entry_in;
use crate::motion_kit::{describe, ensure};
use crate::native_governance::stamp;
use crate::rng::SimRng;

/// Milliseconds in the first day; the tape clock reads its end.
const DAY_MS: u64 = 86_400_000;
/// Milliseconds in one eighth of an hour (exact in binary floating point).
const EIGHTH_HOUR_MS: u64 = 450_000;

/// Bug caught: a window of fractional hours truncated to whole hours, so a
/// replay of the last half hour reads nothing (and 1.9 hours reads one hour),
/// hiding recent activity from the one reader meant to show it.
#[test]
fn sim_tape_fractional_hour_windows_match_the_reference() -> Result<(), String> {
    for seed in 0..25_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(420_000));
        let store = GovernanceStore::open_in_memory().map_err(describe)?;
        let reader = TapeReader::new(store.clone());
        let mut minutes = Vec::new();
        for index in 0..90_u64 {
            let minute = if index < 70 {
                1_439_u64.saturating_sub(random.below(300))
            } else {
                random.below(1_440)
            };
            store
                .append(&governance_entry_in(
                    "tenant-t",
                    &format!("row-{index}"),
                    "session",
                    "tick",
                    &stamp(minute),
                ))
                .map_err(describe)?;
            minutes.push((index, minute.saturating_mul(60_000)));
        }
        let scope = Scope::new("tenant-t", "session");
        for _ in 0..12 {
            let eighths = random.below(200);
            let hours = f64::from(u32::try_from(eighths).map_err(describe)?) / 8.0;
            let window_ms = eighths.min(192).saturating_mul(EIGHTH_HOUR_MS);
            let from_ms = DAY_MS.saturating_sub(window_ms);
            let mut kept: Vec<&(u64, u64)> = minutes
                .iter()
                .filter(|row| row.1 >= from_ms && row.1 <= DAY_MS)
                .collect();
            kept.sort_by_key(|row| row.1);
            let want: Vec<String> = kept.iter().map(|row| format!("row-{}", row.0)).collect();
            let got: Vec<String> = reader
                .recent_entries(&scope, 1_000, hours, DAY_MS)
                .map_err(describe)?
                .iter()
                .map(|row| String::from(row.signal_id()))
                .collect();
            ensure(
                got == want,
                &format!(
                    "seed {seed}: {hours} hours read {} rows, the {window_ms} ms window holds {}",
                    got.len(),
                    want.len()
                ),
            )?;
        }
        for hours in [f64::NAN, -3.0, 0.0] {
            let got = reader
                .recent_entries(&scope, 1_000, hours, DAY_MS)
                .map_err(describe)?;
            ensure(
                got.iter().all(|row| row.timestamp_ms() == DAY_MS),
                &format!("seed {seed}: a window of {hours} hours must be empty of earlier rows"),
            )?;
        }
    }
    Ok(())
}
