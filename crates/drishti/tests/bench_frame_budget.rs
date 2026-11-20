//! Frame-budget bench: 100 tweens through `frame()`, reporting
//! p50/p95/p99 over 200 samples with peak RSS.
//!
//! Counterpart of `motion-core/benches/frame_budget.rs` (kernel#850 row 15),
//! whose informational target was 0.5 ms on the bench machine. The pass or
//! fail verdict is counted and load-independent: every sample must evaluate
//! all 100 bindings, and every sample must produce the same bindings as the
//! first (the clock is deterministic). The wall-clock percentiles are a
//! bench receipt written to `target/bench-frame-budget.json` for the record;
//! no host load can fail the test because no timing is asserted.

use std::time::Instant;

use drishti::motion::{
    AccumulatorStore, Clock, Easing, MotionGraph, MotionGraphBuilder, NamedCurve, NodeKind,
    TweenComponent, frame, scrub::EmptyScrubMap,
};
use lgwks_std::id::Uuid;

/// Build the 100-tween graph from the kernel bench.
fn bench_graph() -> Result<MotionGraph, Box<dyn std::error::Error>> {
    let mut builder = MotionGraphBuilder::new();
    for _ in 0..100 {
        let tween = TweenComponent::new(0.0, 1.0, 0.0, 600.0, Easing::named(NamedCurve::CubicOut))?;
        builder = builder.add_node(
            Uuid::new_v4()?,
            String::from(".bench-target"),
            String::from("opacity"),
            NodeKind::Tween(tween),
        );
    }
    Ok(builder.build())
}

/// Nearest-rank percentile over sorted samples.
fn percentile(sorted: &[u128], rank: u128) -> u128 {
    // `u128` holds every length; refuse where the platform is wider.
    let Ok(count) = u128::try_from(sorted.len()) else {
        return 0;
    };
    // ceil(rank/100 * count), clamped into 1..=count, without floats.
    let scaled = rank
        .saturating_mul(count)
        .saturating_add(99)
        .wrapping_div(100);
    let position = scaled.clamp(1, count.max(1)).saturating_sub(1);
    // Unreachable: the position is below the sample count.
    let Ok(index) = usize::try_from(position) else {
        return 0;
    };
    // Only reachable with no samples; the test always holds 200.
    if let Some(sample) = sorted.get(index) {
        *sample
    } else {
        0
    }
}

/// Peak resident set size in bytes from Linux VmHWM.
#[cfg(target_os = "linux")]
fn peak_rss_bytes() -> Result<(u64, &'static str), Box<dyn std::error::Error>> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    for line in status.lines() {
        if let Some(kilobytes) = line.strip_prefix("VmHWM:") {
            let digits: String = kilobytes
                .chars()
                .filter(|char| char.is_ascii_digit())
                .collect();
            let value = digits.parse::<u64>()?;
            return Ok((value.saturating_mul(1_024), "linux VmHWM"));
        }
    }
    Err("linux VmHWM absent".into())
}

/// Peak resident set size is unmeasured off Linux.
#[cfg(not(target_os = "linux"))]
const fn peak_rss_bytes() -> (u64, &'static str) {
    (0, "non-linux: peak RSS unmeasured on this host")
}

/// 100-tween frames evaluate every binding, identically on every sample; the
/// timing percentiles are recorded as a receipt, never asserted.
#[test]
fn frame_bench_counts_every_binding_and_records_receipt() -> Result<(), Box<dyn std::error::Error>>
{
    let graph = bench_graph()?;
    let clock = Clock::deterministic(300.0);
    let scrubs = EmptyScrubMap::new();
    let mut samples = Vec::with_capacity(200);
    let mut first_values: Vec<Option<u32>> = Vec::new();
    for sample in 0..200_usize {
        let mut accumulators = AccumulatorStore::new();
        let started = Instant::now();
        let frame = frame(&graph, &clock, &mut accumulators, &scrubs);
        let elapsed = started.elapsed().as_nanos();
        if frame.bindings().len() != 100 {
            return Err("bench must evaluate 100 bindings".into());
        }
        let values: Vec<Option<u32>> = frame
            .bindings()
            .iter()
            .map(|binding| binding.value().as_scalar().map(f32::to_bits))
            .collect();
        if sample == 0 {
            first_values = values;
        } else if values != first_values {
            return Err(format!("sample {sample} diverged from sample 0").into());
        }
        samples.push(elapsed);
    }
    if samples.len() != 200 {
        return Err("bench must record 200 samples".into());
    }
    samples.sort_unstable();
    let p50 = percentile(&samples, 50);
    let p95 = percentile(&samples, 95);
    let p99 = percentile(&samples, 99);
    #[cfg(target_os = "linux")]
    let (rss, rss_source) = peak_rss_bytes()?;
    #[cfg(not(target_os = "linux"))]
    let (rss, rss_source) = peak_rss_bytes();
    let receipt = format!(
        "{{\"bench\":\"frame_100_tweens_nanos\",\"samples\":{},\"p50\":{p50},\"p95\":{p95},\"p99\":{p99},\"budget_nanos\":500000,\"peak_rss_bytes\":{rss},\"rss_source\":\"{rss_source}\"}}\n",
        samples.len()
    );
    // Cargo creates CARGO_TARGET_TMPDIR for integration tests under whatever
    // target dir the run uses, so the receipt lands even when CARGO_TARGET_DIR
    // points outside the checkout (the mutation gate's copy does).
    let receipt_path =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bench-frame-budget.json");
    std::fs::write(&receipt_path, receipt.as_bytes())?;
    Ok(())
}
