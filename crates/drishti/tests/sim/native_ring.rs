//! Telemetry-core sims: the ring buffer, the latency tracker and the tape
//! reader's window are compared with plain reference models, with capacities
//! and hours driven to zero, huge and not-a-number.

use drishti::native_host::broker::Scope;
use drishti::native_host::governance::{GovernanceError, GovernanceStore};
use drishti::native_host::telemetry::{
    LatencyTracker, MAX_RING_CAPACITY, RingBuffer, TapeReader, TelemetryError,
};

use crate::fixtures::governance_entry_in;
use crate::motion_kit::{describe, ensure};
use crate::rng::SimRng;

/// Milliseconds in an hour.
const HOUR_MS: u64 = 3_600_000;

/// Bug caught: a ring that returns items out of order after wrapping, loses
/// the running total, or reports a count above its capacity.
#[test]
fn sim_ring_buffer_matches_a_list_model() -> Result<(), String> {
    for seed in 0..200_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(400_000));
        let capacity = random.below_len(20).saturating_add(1);
        let mut ring: RingBuffer<String> = RingBuffer::new(capacity).map_err(describe)?;
        let mut model: Vec<String> = Vec::new();
        let pushes = random.below_len(200);
        for index in 0..pushes {
            let item = format!("item-{index}-{}", random.below(1_000));
            ring.push(item.clone());
            model.push(item);
            let held = model.len().min(capacity);
            let want = model[model.len().saturating_sub(held)..].to_vec();
            ensure(
                ring.snapshot() == want
                    && ring.count() == held
                    && ring.total_pushed()
                        == u64::try_from(index.saturating_add(1)).map_err(describe)?,
                &format!("seed {seed} push {index}: the ring drifted from the model"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a zero capacity accepted (a ring that cannot hold anything),
/// or a huge capacity that reserves its memory up front.
#[test]
fn sim_ring_buffer_capacity_edges() -> Result<(), String> {
    ensure(
        RingBuffer::<u8>::new(0).err() == Some(TelemetryError::InvalidCapacity(0)),
        "a zero capacity must be the typed refusal",
    )?;
    let mut huge: RingBuffer<u64> = RingBuffer::new(100_000_000).map_err(describe)?;
    for value in 0..1_000_u64 {
        huge.push(value);
    }
    ensure(
        huge.count() == 1_000
            && huge.total_pushed() == 1_000
            && huge.snapshot().first() == Some(&0)
            && huge.snapshot().last() == Some(&999),
        "a huge capacity must hold only what was pushed",
    )?;
    let mut single: RingBuffer<u64> = RingBuffer::new(1).map_err(describe)?;
    ensure(
        single.snapshot().is_empty(),
        "an empty ring snapshots empty",
    )?;
    for value in 0..50_u64 {
        single.push(value);
    }
    ensure(
        single.snapshot() == [49] && single.count() == 1 && single.total_pushed() == 50,
        "a one-slot ring keeps only the newest",
    )?;
    ensure(
        MAX_RING_CAPACITY == 5_000,
        "the documented tap bound is 5,000",
    )
}

/// Bug caught: a p99 off by one at small sample counts, taken over samples
/// the ring has already overwritten, or a panic on an empty tracker.
#[test]
fn sim_latency_p99_matches_the_sorted_reference() -> Result<(), String> {
    ensure(
        LatencyTracker::new().p99_microseconds() == 0
            && LatencyTracker::with_capacity(0).p99_microseconds() == 0,
        "an empty tracker reports zero",
    )?;
    for seed in 0..300_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(410_000));
        let capacity = random.below_len(60).saturating_add(1);
        let mut tracker = LatencyTracker::with_capacity(capacity);
        let mut samples: Vec<u64> = Vec::new();
        let total = random.below_len(150).saturating_add(1);
        for _ in 0..total {
            let sample = if random.below(20) == 0 {
                u64::MAX
            } else {
                random.below(5_000_000)
            };
            tracker.record(sample);
            samples.push(sample);
        }
        let mut window = samples[samples.len().saturating_sub(capacity)..].to_vec();
        window.sort_unstable();
        let index = window
            .len()
            .saturating_sub(1)
            .saturating_mul(99)
            .wrapping_div(100);
        let want = window[index].wrapping_div(1_000);
        ensure(
            tracker.p99_microseconds() == want,
            &format!(
                "seed {seed}: p99 over {} of {total} samples read {}, reference {want}",
                window.len(),
                tracker.p99_microseconds()
            ),
        )?;
    }
    let mut zero_ring = LatencyTracker::with_capacity(0);
    zero_ring.record(9_000);
    zero_ring.record(7_000);
    ensure(
        zero_ring.p99_microseconds() == 7,
        "a zero sample capacity is treated as one and keeps the newest",
    )
}

/// The oldest age in milliseconds a window request may reach, by the
/// documented rule: not-a-number, infinite, zero and negative requests read
/// nothing older than the clock; the rest clamp to 24 hours, fractions kept.
fn window_limit_ms(requested: f64) -> f64 {
    if !requested.is_finite() || requested <= 0.0 {
        0.0
    } else {
        requested.min(24.0) * 3_600_000.0
    }
}

/// Bug caught: a tape window that honours not-a-number or infinite hours,
/// reads past 24 hours, drops the fraction of an hour (half an hour reading
/// nothing), or underflows when the clock is near the epoch.
#[test]
fn sim_tape_window_clamps_hours_and_clock() -> Result<(), String> {
    let store = GovernanceStore::open_in_memory().map_err(describe)?;
    let day = 24 * HOUR_MS;
    let mut stamps = Vec::new();
    for index in 0..48_u64 {
        let minute = index.saturating_mul(30);
        let signal = format!("tick-{index}");
        let stamp = format!(
            "1970-01-01T{:02}:{:02}:00Z",
            minute.wrapping_div(60),
            minute.wrapping_rem(60)
        );
        store
            .append(&governance_entry_in(
                "tenant-r", &signal, "session", "tick", &stamp,
            ))
            .map_err(describe)?;
        stamps.push((signal, minute.saturating_mul(60_000)));
    }
    let reader = TapeReader::new(store);
    let scope = Scope::new("tenant-r", "session");
    let requests = [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -5.0,
        -0.0,
        0.0,
        f64::MIN_POSITIVE,
        0.5,
        0.999,
        1.0,
        1.9,
        2.0,
        12.5,
        23.99,
        24.0,
        24.5,
        1e9,
        1e300,
        f64::MAX,
    ];
    let clocks = [0_u64, 1_000, HOUR_MS, 5 * HOUR_MS, day, day * 2];
    for requested in requests {
        for now in clocks {
            let limit = window_limit_ms(requested);
            let mut want: Vec<&String> = Vec::new();
            for entry in &stamps {
                if entry.1 > now {
                    continue;
                }
                let age = u32::try_from(now.saturating_sub(entry.1)).map_err(describe)?;
                if f64::from(age) <= limit {
                    want.push(&entry.0);
                }
            }
            let got: Vec<String> = reader
                .recent_entries(&scope, usize::MAX, requested, now)
                .map_err(describe)?
                .iter()
                .map(|row| String::from(row.signal_id()))
                .collect();
            ensure(
                got.iter().eq(want.iter().copied()),
                &format!(
                    "hours {requested:e} at {now}: read {} rows, reference {}",
                    got.len(),
                    want.len()
                ),
            )?;
        }
    }
    ensure(
        reader.recent_entries(&Scope::new("", "session"), 5, 1.0, day)
            == Err(GovernanceError::Unbounded)
            && reader.recent_entries(&Scope::new("tenant-r", ""), 5, 1.0, day)
                == Err(GovernanceError::Unbounded),
        "an empty tenant or session is the typed unbounded refusal",
    )
}
