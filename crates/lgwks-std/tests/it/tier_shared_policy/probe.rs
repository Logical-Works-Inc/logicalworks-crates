//! The downstream consumer that runs the tiers and prints their report.
//!
//! It is a generated package rather than this test binary for one reason: the
//! workspace forbids printing from a test, and the per-tier p50/p95/p99 and peak
//! RSS *are* the deliverable of #154's reviewer note. Running them in a
//! consumer whose stdout is returned as a value keeps the report attached to
//! the assertion that checks it.
//!
//! The tiers, the fixtures and the comparison against the single-threaded
//! reference are deliberately the same code as in the parent module, so a
//! downstream run and an in-process run are two observations of one contract
//! rather than two contracts.

/// The consumer's `main.rs`, built and run by the parent test.
pub(super) const SHARED_POLICY_TIER_PROBE: &str = r##"
use lgwks_std::glob::{GlobDialect, GlobPattern, GlobScratch};
use lgwks_std::retry::RetryPolicy;
use lgwks_std::similarity::{CheckedEvidence, EditDistance};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const TIERS: [usize; 3] = [100, 1_000, 10_000];
const STACK_BYTES: usize = 64 * 1024;
const SHARED_PATTERN: &str = "*a**/b[0-9]?";
const FIXTURE_PATHS: [&str; 4] = ["a/x/b7z", "a/x/bzz", "abz", "a/b7z/deeper"];
const FIXTURE_TEXTS: [&str; 4] = [
    "kitten",
    "sitting",
    "an input well past the sixteen character budget",
    "",
];

/// The attempts the shared retry policy is driven across, one per caller
/// position modulo the window: a caller's attempt is a value this names rather
/// than a narrowing of its position.
const ATTEMPT_WINDOW: [u32; 40] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
    20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39,
];

/// The entry of `table` that `draw` names, wrapped into the table's length.
fn pick<T: Copy>(table: &[T], draw: usize, floor: T) -> T {
    let position = draw.rem_euclid(table.len().max(1));
    table
        .iter()
        .take(position.saturating_add(1))
        .fold(floor, |_, &entry| entry)
}

/// The caller's position read as the word a policy takes entropy in, mixed a
/// byte at a time so no bit of the position is dropped.
fn word_of(value: usize) -> u64 {
    let mut word = 0_u64;
    for byte in value.to_le_bytes() {
        word = word
            .wrapping_mul(31)
            .wrapping_add(u64::from(byte));
    }
    word
}

/// An elapsed duration in nanoseconds, read as seconds plus sub-second
/// nanoseconds so no narrowing of a `u128` nanosecond count is needed.
fn nanos_of(elapsed: std::time::Duration) -> u64 {
    elapsed
        .as_secs()
        .saturating_mul(1_000_000_000_u64)
        .saturating_add(u64::from(elapsed.subsec_nanos()))
}

fn peak_rss_bytes() -> Option<u64> {
    if let Ok(statm) = std::fs::read_to_string("/proc/self/statm")
        && let Some(pages) = statm.split_whitespace().nth(1)
        && let Ok(pages) = pages.parse::<u64>()
        && let Some(bytes) = pages.checked_mul(4096)
    {
        return Some(bytes);
    }
    let pid = std::process::id().to_string();
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()?.split_whitespace().next()?.parse::<u64>().ok()?.checked_mul(1024)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CallerAnswer {
    matched: bool,
    score_bits: u64,
    accepted: bool,
    refusal: String,
    delay_nanos: u128,
}

fn answer(
    pattern: &GlobPattern,
    evidence: &CheckedEvidence<str>,
    retry: &RetryPolicy,
    scratch: &mut GlobScratch,
    index: usize,
) -> CallerAnswer {
    let path = pick(&FIXTURE_PATHS, index, "");
    let text = pick(&FIXTURE_TEXTS, index, "");
    let verdict = evidence.verdict(text, text);
    let (refusal, score_bits, accepted) = match verdict.as_ref() {
        Ok(inner) => {
            let reason = match inner.refusals().first().and_then(|outcome| outcome.reason()) {
                Some(reason) => reason.to_string(),
                None => String::new(),
            };
            let bits = match inner.score() {
                Some(score) => score.to_bits(),
                None => 0,
            };
            (reason, bits, inner.is_accepted())
        }
        Err(reason) => (reason.to_string(), 0, false),
    };
    CallerAnswer {
        matched: pattern.is_match_with(path, scratch),
        score_bits,
        accepted,
        refusal,
        delay_nanos: retry
            .delay(pick(&ATTEMPT_WINDOW, index, 0), word_of(index))
            .as_nanos(),
    }
}

struct Shared {
    pattern: RwLock<GlobPattern>,
    evidence: RwLock<CheckedEvidence<str>>,
    retry: RwLock<RetryPolicy>,
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    match lock.read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn percentiles(samples: &[u64]) -> (u64, u64, u64) {
    if samples.is_empty() {
        return (0, 0, 0);
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let count = sorted.len();
    let rank = |percent: usize| -> u64 {
        let index = percent
            .saturating_mul(count)
            .div_ceil(100)
            .saturating_sub(1)
            .min(count.saturating_sub(1));
        pick(&sorted, index, 0)
    };
    (rank(50), rank(95), rank(99))
}

fn main() {
    let Ok(pattern) = GlobPattern::compile_with_dialect(SHARED_PATTERN, GlobDialect::Legacy) else {
        println!("compile-failed 1");
        return;
    };
    let Ok(evidence) = CheckedEvidence::new(
        vec![Box::new(EditDistance::new(16)), Box::new(EditDistance::new(16))],
        vec![0.5, 0.5],
        0.5,
    ) else {
        println!("evidence-failed 1");
        return;
    };
    let retry = RetryPolicy::new(u32::MAX, Duration::from_millis(100), Duration::MAX);
    let shared = Arc::new(Shared {
        pattern: RwLock::new(pattern),
        evidence: RwLock::new(evidence),
        retry: RwLock::new(retry),
    });
    let clock: fn() -> Instant = Instant::now;

    let mut reference = Vec::with_capacity(TIERS[2]);
    {
        let mut scratch = GlobScratch::new();
        for index in 0..TIERS[2] {
            let pattern = read(&shared.pattern);
            let evidence = read(&shared.evidence);
            let retry = read(&shared.retry);
            reference.push(answer(&pattern, &evidence, &retry, &mut scratch, index));
        }
    }

    // A resident-size figure the platform cannot report is not printed at all:
    // the caller reads a missing measurement as absent and refuses the tier,
    // which is what an unread figure means.
    if let Some(baseline) = peak_rss_bytes() {
        println!("baseline-rss {baseline}");
    }

    for tier in TIERS {
        let mut answers: Vec<CallerAnswer> = Vec::with_capacity(tier);
        let mut durations: Vec<u64> = Vec::with_capacity(tier);
        let mut reached = 0usize;
        for index in 0..tier {
            let owned = Arc::clone(&shared);
            let built = std::thread::Builder::new()
                .name(format!("tier-{index}"))
                .stack_size(STACK_BYTES)
                .spawn(move || {
                    let mut scratch = GlobScratch::new();
                    let pattern = read(&owned.pattern);
                    let evidence = read(&owned.evidence);
                    let retry = read(&owned.retry);
                    let start = clock();
                    let value = answer(&pattern, &evidence, &retry, &mut scratch, index);
                    (value, nanos_of(start.elapsed()))
                });
            match built {
                Ok(joined) => {
                    let (value, nanos) = match joined.join() {
                        Ok(observed) => observed,
                        Err(_panicked) => (
                            CallerAnswer {
                                matched: false,
                                score_bits: 0,
                                accepted: false,
                                refusal: String::from("caller panicked"),
                                delay_nanos: 0,
                            },
                            0,
                        ),
                    };
                    answers.push(value);
                    durations.push(nanos);
                    reached += 1;
                }
                Err(error) => println!("tier{tier}-spawn-refusal {index} {:?}", error.kind()),
            }
        }
        let divergences = answers
            .iter()
            .zip(reference.iter())
            .filter(|(answer, expected)| *answer != *expected)
            .count();
        let (p50, p95, p99) = percentiles(&durations);
        let rss = peak_rss_bytes();
        println!("tier{tier}-reached {reached}");
        println!("tier{tier}-divergences {divergences}");
        println!("tier{tier}-p50 {p50}");
        println!("tier{tier}-p95 {p95}");
        println!("tier{tier}-p99 {p99}");
        if let Some(rss) = rss {
            println!("tier{tier}-rss {rss}");
        }
    }

    // The drain check: after every tier has joined, one more call on the same
    // shared values must still equal the reference.
    let mut after_drain = 0usize;
    {
        let mut scratch = GlobScratch::new();
        for (index, expected) in reference.iter().enumerate() {
            let pattern = read(&shared.pattern);
            let evidence = read(&shared.evidence);
            let retry = read(&shared.retry);
            if answer(&pattern, &evidence, &retry, &mut scratch, index) != *expected {
                after_drain += 1;
            }
        }
    }
    println!("after-drain-divergences {after_drain}");
}
"##;
