//! The numbers a measurement reports: nearest-rank percentiles and the
//! process's resident set.
//!
//! `crates/lgwks-bot/examples/support/measure.rs` holds the same nearest-rank
//! rule, but it cannot be included here: it keeps its percentiles private to a
//! `Summary` that prints a line and offers no maximum, so including it would
//! leave dead code behind and no way to hand the numbers to a JSON writer. The
//! rule is restated below and a reference model in the tests pins it.

use std::process::Command;
use std::time::Duration;

use lgwks_std::json::Serialize;

use crate::error::BenchError;

/// A duration in whole microseconds, saturating instead of wrapping.
pub(crate) fn micros(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_mul(1_000_000)
        .saturating_add(u64::from(duration.subsec_micros()))
}

/// A percentile of a sorted sample by nearest rank: the observed value, never
/// an interpolation. An empty sample reads `0`, and its [`Summary`] says so
/// with `samples: 0`.
fn percentile(sorted: &[u64], per_mille: usize) -> u64 {
    let Some(top) = sorted.last() else {
        return 0;
    };
    let rank = sorted.len().saturating_mul(per_mille).div_ceil(1_000);
    match sorted.get(rank.saturating_sub(1)) {
        Some(value) => *value,
        None => *top,
    }
}

/// The distribution of one measurement, in the unit its samples were taken in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub(crate) struct Summary {
    /// How many samples the percentiles are read from.
    pub(crate) samples: usize,
    /// Median.
    pub(crate) p50: u64,
    /// 95th.
    pub(crate) p95: u64,
    /// 99th.
    pub(crate) p99: u64,
    /// Largest.
    pub(crate) max: u64,
}

impl Summary {
    /// The percentiles of `samples`, sorted in place.
    pub(crate) fn of(samples: &mut [u64]) -> Self {
        samples.sort_unstable();
        Self {
            samples: samples.len(),
            p50: percentile(samples, 500),
            p95: percentile(samples, 950),
            p99: percentile(samples, 990),
            max: percentile(samples, 1_000),
        }
    }
}

/// The resident set of this process and where the figure came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Resident {
    /// Bytes.
    pub(crate) bytes: u64,
    /// What read it, so a reader knows what the number is.
    pub(crate) source: &'static str,
}

/// What reads the resident set: stable Rust has no in-process accessor for the
/// kernel's high-water mark, so this is the process's resident set sampled by
/// `ps` once the run has finished and its memory is still held.
const RSS_SOURCE: &str = "ps rss of this process, sampled after the run (no in-process peak)";

/// The resident set `ps` reported, in bytes.
///
/// # Errors
///
/// [`BenchError::MemoryReading`] when the text is not one whole number of
/// kibibytes or the product does not fit.
pub(crate) fn parse_ps(text: &str) -> Result<u64, BenchError> {
    let kibibytes: u64 = text.trim().parse().map_err(|source| {
        lgwks_std::trace::debug!(%source, "ps did not answer one whole number");
        BenchError::MemoryReading {
            reason: "ps did not answer one whole number",
        }
    })?;
    kibibytes
        .checked_mul(1_024)
        .ok_or(BenchError::MemoryReading {
            reason: "the resident set does not fit a u64",
        })
}

/// This process's resident set now.
///
/// # Errors
///
/// [`BenchError::Io`] when `ps` cannot run, and [`BenchError::MemoryReading`]
/// when it answers something else or answers zero.
pub(crate) fn resident() -> Result<Resident, BenchError> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p"])
        .arg(std::process::id().to_string())
        .output()
        .map_err(|source| BenchError::Io {
            context: "reading the resident set with ps",
            source,
        })?;
    let text = String::from_utf8(output.stdout).map_err(|source| {
        lgwks_std::trace::debug!(%source, "ps did not answer UTF-8");
        BenchError::MemoryReading {
            reason: "ps did not answer UTF-8",
        }
    })?;
    let bytes = parse_ps(&text)?;
    if bytes == 0 {
        lgwks_std::trace::debug!("ps answered a resident set of zero");
        return Err(BenchError::MemoryReading {
            reason: "ps answered a resident set of zero",
        });
    }
    Ok(Resident {
        bytes,
        source: RSS_SOURCE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lgwks_std::seeded::Seeded;

    /// A test's result.
    type TestResult = Result<(), String>;

    /// The reference: the least observed value with at least `ceil(p * n / 1000)`
    /// samples at or below it, found by counting, not by indexing. An empty
    /// sample has no value, which the summary reads as zero.
    fn reference(sorted: &[u64], per_mille: usize) -> u64 {
        let need = (sorted.len() * per_mille).div_ceil(1_000).max(1);
        for candidate in sorted {
            if sorted.iter().filter(|other| *other <= candidate).count() >= need {
                return *candidate;
            }
        }
        0
    }

    /// Seeded samples of every size from empty to a few hundred, duplicates
    /// included: the indexed rule and the counting rule never disagree.
    #[test]
    fn sim_percentiles_match_the_counting_reference() -> TestResult {
        let mut stream = Seeded::from_seed(9);
        for length in 0_usize..300 {
            let mut samples = Vec::new();
            for _ in 0..length {
                samples.push(stream.below(40).map_err(|error| error.to_string())?);
            }
            let summary = Summary::of(&mut samples);
            assert_eq!(summary.p50, reference(&samples, 500), "length {length}");
            assert_eq!(summary.p95, reference(&samples, 950), "length {length}");
            assert_eq!(summary.p99, reference(&samples, 990), "length {length}");
            assert_eq!(summary.max, reference(&samples, 1_000), "length {length}");
            assert_eq!(summary.samples, length);
        }
        Ok(())
    }

    /// Nothing sampled reads zero samples and zero percentiles, not a panic.
    #[test]
    fn an_empty_sample_says_zero_samples() {
        let summary = Summary::of(&mut []);
        assert_eq!(
            summary,
            Summary {
                samples: 0,
                p50: 0,
                p95: 0,
                p99: 0,
                max: 0
            }
        );
    }

    /// Microseconds saturate rather than wrap at the largest duration.
    #[test]
    fn micros_saturates_at_the_largest_duration() {
        assert_eq!(micros(Duration::from_micros(1_500)), 1_500);
        assert_eq!(micros(Duration::MAX), u64::MAX);
    }

    /// `ps` answers kibibytes with padding; anything else is refused typed.
    #[test]
    fn parse_ps_reads_kibibytes_and_refuses_the_rest() {
        assert_eq!(parse_ps("  2048\n").ok(), Some(2_097_152));
        for hostile in ["", "abc", "-5", "12 34", "18446744073709551615"] {
            assert!(
                matches!(parse_ps(hostile), Err(BenchError::MemoryReading { .. })),
                "{hostile:?}"
            );
        }
    }

    /// The live reading is a positive number from a real `ps`.
    #[test]
    fn the_resident_set_of_this_test_process_is_positive() -> TestResult {
        let reading = resident().map_err(|error| error.line())?;
        assert!(reading.bytes > 0);
        assert!(!reading.source.is_empty());
        Ok(())
    }
}
