//! The hashing a keyed `serve` request pays, as numbers (#1008).
//!
//! A keyed request hashes twice before its body runs: `InputDigest::of`, the
//! receipt's input digest, and the run identity `derive_run` derives from the
//! tenant and key. This times both on the shapes the rig sends (an empty body,
//! a short key) and on larger bodies, and prints p50/p95/p99 in nanoseconds per
//! call, so the cost reads against a request's CPU time rather than as an
//! adjective. `derive_run` is crate-private, so its bytes are mirrored here with
//! the same hasher, framing and domain label; the example measures the
//! primitive, and the rig measures the whole path.

use std::hint::black_box;
use std::time::Instant;

use lgwks_bot::effect::InputIdentity;
use lgwks_bot::task::InputDigest;
use lgwks_std::hash::Hasher;

/// Timed samples per shape.
const SAMPLES: usize = 2_048;

/// Calls inside one sample, so the clock's own cost is a small part of it.
const CALLS: usize = 256;

/// Body sizes in bytes: the rig's empty body, a small JSON, a page, a large post.
const BODY_SIZES: [usize; 4] = [0, 64, 1_024, 16_384];

/// The domain label `derive_run` frames first.
const RUN_DOMAIN: &[u8] = b"lgwks.bot.request-run.v1";

/// A request body as the serve hashes it.
struct Body(Vec<u8>);

impl InputIdentity for Body {
    /// The schema the serve's input binds its digest to.
    const SCHEMA_ID: &'static [u8] = b"lgwks.bot.schema.v1.serve-body";

    /// The body bytes, framed.
    fn write_identity(&self, hasher: &mut Hasher) {
        hasher.write_framed(self.0.as_slice());
    }
}

/// The run identity bytes `derive_run` takes from a tenant and a key.
fn run_head(tenant: &str, key: &str) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher
        .write_framed(RUN_DOMAIN)
        .write_framed(tenant.as_bytes())
        .write_framed(key.as_bytes());
    *hasher.finalize().as_bytes()
}

/// The `percent`th percentile of sorted `values`, or nothing when empty.
fn percentile(sorted: &[u64], percent: usize) -> Option<u64> {
    let last = sorted.len().checked_sub(1)?;
    let rank = last.checked_mul(percent)?.checked_div(100)?;
    sorted.get(rank).copied()
}

/// Nanoseconds per call over `SAMPLES` samples of `CALLS` calls of `work`:
/// the p50, p95 and p99 of the per-sample means.
fn measure(mut work: impl FnMut()) -> Result<(u64, u64, u64), String> {
    let calls = u64::try_from(CALLS).map_err(|cause| format!("CALLS does not fit u64: {cause}"))?;
    let mut means = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        for _ in 0..CALLS {
            work();
        }
        let nanos = u64::try_from(started.elapsed().as_nanos())
            .map_err(|cause| format!("a sample does not fit u64: {cause}"))?;
        let mean = nanos
            .checked_div(calls)
            .ok_or_else(|| String::from("a sample has no calls"))?;
        means.push(mean);
    }
    means.sort_unstable();
    let rank = |percent| {
        percentile(means.as_slice(), percent).ok_or_else(|| String::from("no samples to rank"))
    };
    Ok((rank(50)?, rank(95)?, rank(99)?))
}

/// One line of the report, through the locked stdout.
fn say(line: &str) -> Result<(), String> {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}").map_err(|error| format!("the report line is refused: {error}"))?;
    out.flush()
        .map_err(|error| format!("the report line is refused: {error}"))
}

fn main() -> Result<(), String> {
    say(&format!(
        "serve digest cost, ns per call: {SAMPLES} samples of {CALLS} calls"
    ))?;
    let tenant = "tenant-17";
    let key = "3f1-17-4096";
    let (p50, p95, p99) = measure(|| {
        black_box(run_head(black_box(tenant), black_box(key)));
    })?;
    say(&format!("derive_run     p50={p50} p95={p95} p99={p99}"))?;
    for size in BODY_SIZES {
        let body = Body(vec![b'x'; size]);
        let (p50, p95, p99) = measure(|| {
            black_box(InputDigest::of(black_box(&body)));
        })?;
        say(&format!(
            "InputDigest::of body={size}B p50={p50} p95={p95} p99={p99}"
        ))?;
    }
    // The bytes a stored receipt and run id hold for these inputs; the unit tests
    // in `task/request.rs` pin the same values, so a hash change cannot pass quietly.
    let empty = Body(Vec::new());
    say(&format!(
        "pin: run head({tenant}, {key}) = {}",
        lgwks_std::hex::encode(run_head(tenant, key))
    ))?;
    say(&format!(
        "pin: InputDigest::of(empty body) = {}",
        InputDigest::of(&empty)
    ))
}
