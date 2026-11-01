//! Parking ten thousand approvals for one tenant: what a park costs and what
//! the process holds at the end.
//!
//! One host over one file store parks `PARKS` runs, each with its own payload,
//! and every park ends `Parked` with no task held: the run after the park is
//! the report, not a second body. The harness times each park and prints the
//! nearest-rank p50/p95/p99 over the observed samples, plus the process's
//! peak resident set, so a ceiling change has numbers to answer to.
//!
//! ```text
//! cargo run -p lgwks_bot --features script,ephemeral --release --example approval_scale
//! ```
//!
//! The output is written through `std::io::Write` rather than `println!` because
//! `clippy::print_stdout` is forbidden workspace-wide with no example-only
//! carve-out; an example that writes explicitly says the same thing fallibly.

use std::io::Write as _;
use std::time::Instant;

use lgwks_bot::script::Scope;
use lgwks_bot::task::{Disposition, Host, task};

#[path = "support/measure.rs"]
mod measure;
#[path = "../tests/support/scratch.rs"]
mod scratch;

use measure::Summary;
use scratch::Scratch;

/// How many approvals one tenant parks.
const PARKS: usize = 10_000;

lgwks_bot::script! {
    /// Park the payload for a human decision.
    flow ship(payload: Vec<u8>) -> Vec<u8>:
        let approved = await approval ship within 30s:
            payload
        give back approved.payload().to_vec()
}

/// The peak resident set in KiB, on Linux, or the current set where the
/// platform names no peak.
///
/// `VmHWM` is the process's own high-water mark as the kernel accounts it,
/// which is what a ceiling is measured against.
#[cfg(target_os = "linux")]
fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.split_whitespace().next()?.parse::<u64>().ok();
        }
    }
    None
}

/// No reading where the platform has no `/proc/self/status`.
///
/// macOS and the BSDs give the peak through `getrusage`, which this crate does
/// not reach without an FFI edge, so the harness reports no number rather than
/// a guess; `/usr/bin/time -l` reads the same peak from outside the process.
#[cfg(not(target_os = "linux"))]
fn peak_rss_kib() -> Option<u64> {
    None
}

/// Report one parked run, or refuse the whole harness on the first surprise.
fn check_parked(
    index: usize,
    report: &lgwks_bot::task::Report<Vec<u8>>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(report.disposition(), Disposition::Parked) {
        lgwks_std::trace::debug!(index, "check_parked: the run did not end parked");
        return Err(format!("park {index} did not end parked: {:?}", report.error()).into());
    }
    Ok(())
}

/// Park `PARKS` approvals for one tenant and print what each park cost.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::io::stdout().lock();
    let scratch = Scratch::new("approval-scale").map_err(|error| format!("{error}"))?;
    let host = Host::builder("scale")
        .map_err(|error| format!("{error}"))?
        .run_store(scratch.path())
        .map_err(|error| format!("{error}"))?
        .build()
        .map_err(|error| format!("{error}"))?;
    let shipping = task("ship", |scope: Scope, payload: Vec<u8>| async move {
        ship(&scope, payload).await
    })
    .map_err(|error| format!("{error}"))?;
    let mut samples: Vec<u128> = Vec::with_capacity(PARKS);
    for index in 0..PARKS {
        let payload = format!("scale-{index:05}").into_bytes();
        let started = Instant::now();
        let report = lgwks_bot::block_on(host.run(&shipping, payload));
        samples.push(started.elapsed().as_micros());
        check_parked(index, &report)?;
    }
    let summary = Summary::of(&mut samples);
    writeln!(out, "parks: {PARKS}")?;
    writeln!(out, "{}", summary.line("park"))?;
    match peak_rss_kib() {
        Some(kib) => writeln!(out, "peak_rss_kib: {kib}")?,
        None => writeln!(out, "peak_rss_kib: unavailable")?,
    }
    Ok(())
}
