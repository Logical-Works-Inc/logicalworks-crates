//! The rig's percentiles against a sorted-array oracle (#1007).
//!
//! Every rig row rests on one percentile definition, `examples/support/
//! measure.rs`, which `edge-load` reads by path. The oracle is the
//! nearest-rank definition itself: the `p`th percentile of `n` samples is the
//! smallest sample `v` such that at least `p * n` of the samples are at or
//! below `v`. The oracle counts that on the sorted array, so it shares no index
//! arithmetic with the code under test, and a regression to the old
//! `(n - 1) * p` floor that `edge-load` used before #1007 is caught on the
//! sizes where the two disagree.
//!
//! Each seed draws a sample size from 1 to 4,096 and one of three shapes: a
//! narrow range full of ties, a heavy tail, or one value. The samples are
//! shuffled before the summary sorts them, as the driver merges its
//! connections in join order. Source: the nearest-rank method (Hyndman and Fan,
//! "Sample Quantiles in Statistical Packages", 1996, definition 1).

#[path = "../../examples/support/measure.rs"]
mod measure;

use crate::sim::{Band, Sim, assert_replays};

/// The seeds this family sweeps.
const BAND: Band = Band::new(0x1007_0000, 256);

/// The smallest sample with at least `per_mille / 1000` of `sorted` at or below it.
fn oracle(sorted: &[u128], per_mille: usize) -> Option<u128> {
    let need = sorted.len().checked_mul(per_mille)?;
    sorted
        .iter()
        .enumerate()
        .find(|&(at, value)| {
            let at_or_below = sorted
                .iter()
                .skip(at)
                .take_while(|&other| other == value)
                .count()
                .saturating_add(at);
            at_or_below.saturating_mul(1_000) >= need
        })
        .map(|(_, &value)| value)
}

/// One seed's samples: a size and a shape drawn from the run's stream.
fn samples(sim: &mut Sim) -> Vec<u128> {
    let size = sim.rng().between(1, 4_097);
    let shape = sim.rng().below(3);
    sim.trace.record_number("size", size);
    sim.trace.record_number("shape", shape);
    (0..size)
        .map(|_| match shape {
            0 => u128::from(sim.rng().between(100, 108)),
            1 => u128::from(sim.rng().between(1, 1_000)) << sim.rng().below(24),
            _ => 250,
        })
        .collect()
}

/// Shuffle `values` in place from the run's stream (Fisher and Yates).
fn shuffle(sim: &mut Sim, values: &mut [u128]) -> Result<(), Box<dyn std::error::Error>> {
    for last in (1..values.len()).rev() {
        let pick = usize::try_from(sim.rng().below(u32::try_from(last.saturating_add(1))?))?;
        values.swap(last, pick);
    }
    Ok(())
}

/// Every seed's p50, p95, p99, extremes and one drawn rank match the oracle,
/// the summary line names the same three numbers whatever order the samples
/// arrived in, and every seed replays to the same trace hash.
#[test]
fn sim_rig_percentiles_match_an_oracle() -> Result<(), Box<dyn std::error::Error>> {
    assert_replays(BAND, |sim| {
        let mut sorted = samples(sim);
        sorted.sort_unstable();
        let drawn = usize::try_from(sim.rng().below(1_001))?;
        for per_mille in [0, 1, 500, 950, 990, 999, 1_000, drawn] {
            let expected = oracle(&sorted, per_mille).ok_or("the oracle found no sample")?;
            let got = measure::percentile(&sorted, per_mille);
            assert_eq!(
                got,
                expected,
                "seed {}: per mille {per_mille} of {} samples",
                sim.seed,
                sorted.len()
            );
            sim.trace.record_number("rank", got);
        }
        let mut arrived = sorted.clone();
        shuffle(sim, &mut arrived)?;
        let line = measure::Summary::of(&mut arrived).line("latency");
        let [p50, p95, p99] = [500, 950, 990].map(|per_mille| oracle(&sorted, per_mille));
        let expected = format!(
            "latency: p50={}us p95={}us p99={}us",
            p50.ok_or("no p50")?,
            p95.ok_or("no p95")?,
            p99.ok_or("no p99")?
        );
        assert_eq!(
            line, expected,
            "seed {}: the summary of shuffled samples",
            sim.seed
        );
        assert_eq!(
            arrived, sorted,
            "seed {}: the summary sorts what it ranks",
            sim.seed
        );
        sim.record(&line);
        Ok(())
    })
}

/// The empty sample answers zero rather than an index past the end, and the
/// two definitions disagree where the old floor rounds down.
#[test]
fn the_nearest_rank_is_not_the_old_floor() {
    assert_eq!(measure::percentile(&[], 990), 0);
    let ten: Vec<u128> = (1..=10).collect();
    // Nearest rank: the 10th of 10 at p95. The old floor: index (9 * 950) / 1000 = 8, the 9th.
    assert_eq!(measure::percentile(&ten, 950), 10);
    assert_eq!(oracle(&ten, 950), Some(10));
}
