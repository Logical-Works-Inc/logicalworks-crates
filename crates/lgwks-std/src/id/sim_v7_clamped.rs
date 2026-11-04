//! Seeded replay of the infallible version 7 rule (INV-ID-4, #851).
//!
//! One seed draws every clock reading (forward, the same millisecond, a step
//! backwards, a clock before 1970 read as zero, a reading past 48 bits) and
//! whether the OS gave entropy for each identifier. Whatever was drawn, every
//! identifier must be a version 7 value strictly greater, as a value and as
//! text, than the one before it, until the stamp is pinned at its 48-bit
//! ceiling; an identifier issued without entropy must carry the random field
//! of the one before it and nothing new. The same seed gives the same trace.

use std::collections::BTreeSet;
use std::thread;

use super::Uuid;
use super::v7::V7Generator;
use crate::seeded::Seeded;

/// The seeds the sweeps replay: the estate's one named table.
const SEEDS: [u64; 4] = crate::seeded::SWEEP_SEEDS;

/// Identifiers issued per seed.
const PER_SEED: usize = 16_384;

/// The mixing constant of the trace fold: the 64-bit FNV-1a prime.
const FNV_PRIME: u64 = 1_099_511_628_211;

/// The trace's starting value: the 64-bit FNV-1a offset basis.
const FNV_BASIS: u64 = 14_695_981_039_346_656_037;

/// The largest 48-bit stamp.
const MAX_MS: u64 = 0xFFFF_FFFF_FFFF;

/// The largest value of the 12-bit counter.
const COUNTER_MAX: u16 = 0x0FFF;

/// A clock reading well inside the 48-bit range: late 2023.
const BASE_MS: u64 = 1_700_000_000_000;

/// Threads that share the live process-wide generator.
const THREADS: usize = 100;

/// Identifiers each thread issues from the live generator.
const PER_THREAD: usize = 1_000;

/// The stamp of an issued value, read from its first six bytes.
fn stamp(id: Uuid) -> u64 {
    let [ts0, ts1, ts2, ts3, ts4, ts5, ..] = *id.as_bytes();
    u64::from_be_bytes([0, 0, ts0, ts1, ts2, ts3, ts4, ts5])
}

/// The 12-bit counter of an issued value.
fn counter(id: Uuid) -> u16 {
    let [_, _, _, _, _, _, high, low, ..] = *id.as_bytes();
    u16::from_be_bytes([high & 0x0f, low])
}

/// The variant byte and the random field: bytes 8 to 15.
fn random_field(id: Uuid) -> [u8; 8] {
    let [_, _, _, _, _, _, _, _, r0, r1, r2, r3, r4, r5, r6, r7] = *id.as_bytes();
    [r0, r1, r2, r3, r4, r5, r6, r7]
}

/// Ten bytes of entropy, one per stream word.
fn draw_entropy(stream: &mut Seeded) -> [u8; 10] {
    let [a0, a1, a2, a3, a4, a5, a6, a7] = stream.next_u64().to_le_bytes();
    let [b0, b1, ..] = stream.next_u64().to_le_bytes();
    [a0, a1, a2, a3, a4, a5, a6, a7, b0, b1]
}

/// The next clock reading: mostly forward, sometimes the same millisecond, a
/// step back, a clock before 1970 (read as zero), or a reading past 48 bits.
fn draw_clock(stream: &mut Seeded, now: u64) -> u64 {
    match stream.next_u64().rem_euclid(64) {
        0 => 0,
        1 => MAX_MS.saturating_add(stream.next_u64().rem_euclid(1_000).saturating_add(1)),
        2..=9 => now.saturating_sub(stream.next_u64().rem_euclid(5_000)),
        10..=31 => now,
        _ => now.saturating_add(stream.next_u64().rem_euclid(3)),
    }
}

/// Issues `PER_SEED` identifiers under `seed`'s clock and entropy and checks
/// every one against the rule; returns the trace of every issued value.
/// Without `ceiling_draws` a reading past 48 bits is replaced by the clock.
fn replay(seed: u64, ceiling_draws: bool) -> u64 {
    let mut stream = Seeded::from_seed(seed);
    let mut generator = V7Generator::new();
    let mut trace = FNV_BASIS;
    let mut clock = BASE_MS;
    let mut previous: Option<Uuid> = None;
    for step in 0..PER_SEED {
        let drawn = draw_clock(&mut stream, clock);
        let reading = if !ceiling_draws && drawn > MAX_MS {
            clock
        } else {
            drawn
        };
        if reading <= MAX_MS && reading > clock {
            clock = reading;
        }
        let entropy = (stream.next_u64().rem_euclid(3) != 0).then(|| draw_entropy(&mut stream));
        let id = generator.issue_clamped(reading, entropy);
        assert_eq!(id.version(), Some(7), "seed {seed} step {step}");
        if let Some(before) = previous {
            let pinned = stamp(before) == MAX_MS && counter(before) == COUNTER_MAX;
            if !pinned {
                assert!(id > before, "seed {seed} step {step}: {id} after {before}");
                assert!(
                    id.to_string() > before.to_string(),
                    "seed {seed} step {step}"
                );
            }
            if entropy.is_none() {
                assert_eq!(
                    random_field(id),
                    random_field(before),
                    "seed {seed} step {step}: no entropy, no new random field"
                );
            }
        }
        for byte in id.as_bytes() {
            trace = trace.wrapping_mul(FNV_PRIME).wrapping_add(u64::from(*byte));
        }
        previous = Some(id);
    }
    trace
}

#[test]
fn every_seed_issues_strictly_increasing_ids_whatever_the_clock_and_entropy_do() {
    for seed in SEEDS {
        replay(seed, false);
    }
}

#[test]
fn readings_past_forty_eight_bits_pin_the_stamp_and_never_refuse() {
    for seed in SEEDS {
        replay(seed, true);
    }
}

#[test]
fn the_same_seed_replays_to_the_same_trace_and_seeds_diverge() {
    let [first, second, ..] = SEEDS;
    assert_eq!(replay(first, true), replay(first, true));
    assert_ne!(replay(first, true), replay(second, true));
}

#[test]
fn a_first_id_without_entropy_is_the_clock_with_a_zero_counter_and_field() {
    let mut generator = V7Generator::new();
    let id = generator.issue_clamped(BASE_MS, None);
    assert_eq!((stamp(id), counter(id)), (BASE_MS, 0));
    assert_eq!(random_field(id), [0x80, 0, 0, 0, 0, 0, 0, 0]);
    let next = generator.issue_clamped(BASE_MS, None);
    assert_eq!((stamp(next), counter(next)), (BASE_MS, 1));
    assert!(next > id);
}

#[test]
fn at_the_ceiling_the_counter_climbs_to_its_maximum_and_holds() {
    let mut generator = V7Generator::new();
    let mut seen = Vec::new();
    for _ in 0..usize::from(COUNTER_MAX).saturating_add(64) {
        seen.push(generator.issue_clamped(MAX_MS.saturating_add(1), Some([0; 10])));
    }
    let counters: Vec<u16> = seen.iter().map(|id| counter(*id)).collect();
    assert!(seen.iter().all(|id| stamp(*id) == MAX_MS));
    assert_eq!(counters.first(), Some(&0));
    assert!(
        counters
            .windows(2)
            .all(|pair| matches!(pair, [earlier, later] if later >= earlier))
    );
    assert_eq!(counters.last(), Some(&COUNTER_MAX));
}

#[test]
fn a_hundred_threads_on_the_live_generator_never_repeat_and_each_sees_order() {
    let per_thread: Vec<Option<Vec<Uuid>>> = thread::scope(|scope| {
        // Every thread is spawned before the first is joined, so all of them
        // contend for the generator at once.
        let mut handles = Vec::with_capacity(THREADS);
        for _ in 0..THREADS {
            handles.push(scope.spawn(|| {
                (0..PER_THREAD)
                    .map(|_| Uuid::now_v7())
                    .collect::<Vec<Uuid>>()
            }));
        }
        let mut joined = Vec::with_capacity(THREADS);
        for handle in handles {
            joined.push(handle.join().ok());
        }
        joined
    });
    assert!(
        per_thread.iter().all(Option::is_some),
        "a thread issuing ids panicked"
    );
    let mut all = BTreeSet::new();
    for mine in per_thread.into_iter().flatten() {
        assert!(
            mine.windows(2)
                .all(|pair| matches!(pair, [earlier, later] if later > earlier)),
            "a thread saw its own ids out of order"
        );
        all.extend(mine);
    }
    assert_eq!(
        all.len(),
        THREADS.saturating_mul(PER_THREAD),
        "an id repeated"
    );
}
