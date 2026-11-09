//! A recorded instant replays bit-identically: seeded, fooled and raced.
//!
//! Source: #845, a downstream consumer needs a clock value it can record and restore
//! exactly, seed-driven under simulation. One seed drives the anchor and every
//! advance, the oracle is a model of nanoseconds in a `u128`, never the clock's own
//! output, and the failure each sim would catch is named on it.

use std::time::Duration;

use lgwks_bot::kernel::clock::{Clock, ClockError, EpochClock, RecordedInstant};
use lgwks_bot::kernel::journal::Durable;

use crate::sim::piped::Outcome;

/// A SplitMix64 step, so one seed drives every choice in a sim.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut mixed = *state;
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    mixed ^ (mixed >> 31)
}

/// The bytes of a whole seeded run: each instant as it would be journaled.
fn trace(seed: u64) -> Outcome<Vec<u8>> {
    let clock = EpochClock::seeded(seed);
    let mut state = seed;
    let mut bytes = clock.now().to_record()?;
    for _ in 0..16 {
        let step = Duration::from_nanos(next(&mut state) >> 20);
        bytes.extend_from_slice(&clock.advance(step)?.to_record()?);
    }
    Ok(bytes)
}

/// Catches a reading drawn from a real clock: two runs of one seed would differ.
#[test]
fn sim_one_seed_gives_one_trace_and_seeds_differ() -> Outcome<()> {
    let mut seeds = 7_u64;
    for _ in 0..2_000 {
        let seed = next(&mut seeds);
        assert_eq!(trace(seed)?, trace(seed)?, "seed {seed}");
        assert_ne!(trace(seed)?, trace(seed.wrapping_add(2))?, "seed {seed}");
    }
    Ok(())
}

/// Catches an archive that depends on the host: this pins the bytes themselves.
#[test]
fn sim_the_archive_of_an_instant_is_its_little_endian_count() -> Outcome<()> {
    let bytes = RecordedInstant::from_unix_nanos(0x0102_0304_0506_0708).to_record()?;
    assert_eq!(bytes, [8, 7, 6, 5, 4, 3, 2, 1]);
    Ok(())
}

/// Catches restore drifting from record: the restored clock reads the value exactly,
/// and both timelines agree with a model of the same advances.
#[test]
fn sim_a_restored_clock_continues_where_the_record_stopped() -> Outcome<()> {
    let mut seeds = 11_u64;
    for _ in 0..2_000 {
        let seed = next(&mut seeds);
        let mut state = seed;
        let first = EpochClock::seeded(seed);
        let mut model = u128::from(seed >> 1);
        for _ in 0..8 {
            let step = next(&mut state) >> 24;
            model = model.saturating_add(u128::from(step));
            first.advance(Duration::from_nanos(step))?;
        }
        let held = u128::from(first.now().unix_nanos());
        assert_eq!(held, model, "seed {seed}");
        let recorded = first.now().to_record()?;
        let second = EpochClock::restored(RecordedInstant::from_record(&recorded)?);
        assert_eq!(second.now(), first.now(), "seed {seed}");
        let step = Duration::from_nanos(next(&mut state) >> 24);
        assert_eq!(second.advance(step), first.advance(step), "seed {seed}");
    }
    Ok(())
}

/// The weird-bytes fool: truncated and empty records are refused, never read as zero.
#[test]
fn sim_short_records_are_refused() {
    for length in 0..8 {
        let bytes = vec![0xA5_u8; length];
        assert!(
            RecordedInstant::from_record(&bytes).is_err(),
            "{length} bytes must not decode"
        );
    }
}

/// The giant fool: a clock at the ceiling reads the ceiling and then refuses.
#[test]
fn sim_the_ceiling_saturates_and_then_refuses() -> Outcome<()> {
    let clock = EpochClock::restored(RecordedInstant::from_unix_nanos(u64::MAX));
    assert_eq!(clock.now().unix_nanos(), u64::MAX);
    let at_end = EpochClock::seeded(u64::MAX);
    at_end.advance(Clock::elapsed_ceiling())?;
    assert_eq!(
        at_end.now().unix_nanos(),
        u64::MAX,
        "saturates, never wraps"
    );
    assert!(matches!(
        at_end.advance(Duration::from_nanos(1)),
        Err(ClockError::OutOfRange { .. })
    ));
    Ok(())
}

/// The clock fool: a system clock behind the last record cannot order new records
/// before it, and a live clock's readings never run backwards.
#[test]
fn sim_a_restart_never_reads_before_the_last_record() -> Result<(), ClockError> {
    let future = RecordedInstant::from_unix_nanos(u64::MAX - 1_000_000_000_000_000);
    let clock = EpochClock::system_after(future)?;
    let first = clock.now();
    assert!(first >= future);
    assert!(clock.now() >= first);
    Ok(())
}

/// The fat-finger fool: crossed readings give zero, not a wrapped duration.
#[test]
fn sim_crossed_readings_give_a_zero_duration() {
    let earlier = RecordedInstant::from_unix_nanos(5);
    let later = RecordedInstant::from_unix_nanos(9);
    assert_eq!(earlier.saturating_duration_since(later), Duration::ZERO);
    assert_eq!(
        later.saturating_duration_since(earlier),
        Duration::from_nanos(4)
    );
    assert_eq!(later.to_string(), "0.000000009s since the Unix epoch");
}

/// The neighbour: two tenants on one seed share a value and never a timeline.
#[test]
fn sim_two_tenants_never_share_a_timeline() -> Result<(), ClockError> {
    let acme = EpochClock::seeded(40);
    let globex = EpochClock::seeded(40);
    let before = globex.now();
    acme.advance(Duration::from_secs(1))?;
    assert_eq!(globex.now(), before);
    assert_eq!(
        acme.now().saturating_duration_since(before),
        Duration::from_secs(1)
    );
    Ok(())
}

/// The double-clicker at scale: racing advances on one timeline lose none, at 100 and
/// 1,000 threads. Ten thousand and above are capped by this host's thread limit.
#[test]
fn sim_racing_advances_lose_nothing() {
    for threads in [100_u64, 1_000] {
        let clock = EpochClock::seeded(0);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                let shared = clock.clone();
                scope.spawn(move || {
                    for _ in 0..16 {
                        let before = shared.now();
                        let landed = shared.advance(Duration::from_nanos(3));
                        assert!(matches!(landed, Ok(at) if at > before));
                    }
                });
            }
        });
        assert_eq!(clock.now().unix_nanos(), threads * 16 * 3);
    }
}
