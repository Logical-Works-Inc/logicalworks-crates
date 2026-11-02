//! p50/p95/p99 of a sealed append and read against the unsealed baseline.
//!
//! ```text
//! cargo run --locked --release -p lgwks_bot --features sealed \
//!     --example sealed_overhead -- [runs]
//! ```
//!
//! `sealed_append` is one sealed [`RunStore`](lgwks_bot::task::RunStore)
//! record append of a 1 KiB secret through the public entry point,
//! `sealed_read` is one [`RunStore::lookup`](lgwks_bot::task::RunStore::lookup)
//! of it back; `plain_append` and `plain_read` are the same records through an
//! unsealed `RunStore` (same frames, chain and fsync), so the gap is what
//! sealing costs over the store it seals. Latency is in microseconds
//! per call. Peak RSS is read externally (`/usr/bin/time -l`), not from
//! inside: a process id is an identity the OS reuses, which the estate's
//! std-first gate refuses.
//!
//! This is a measurement, not a gate: the `sealed` and `sim_sealed` suites
//! assert the invariants, and this prints the numbers a reader needs to
//! judge the AEAD overhead. It is an example rather than a test so it may
//! print.

use std::io::Write as _;
use std::sync::Arc;

use lgwks_bot::sealed::keys::SoftwareProvider;

#[path = "support/measure.rs"]
mod measure;

/// Runs each mechanism is measured over when the command line names no count.
const DEFAULT_RUNS: &str = "500";

/// The sealed payload size: one 1 KiB secret per record.
const PAYLOAD_BYTES: usize = 1024;

/// Summarise a sample as one report line, in microseconds.
fn report(label: &str, samples: &mut [u128]) -> std::io::Result<()> {
    let summary = measure::Summary::of(samples);
    let mut out = std::io::stdout().lock();
    let _written = writeln!(out, "{} n={}", summary.line(label), samples.len());
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use lgwks_bot::effect::RunId;
    use lgwks_bot::script::run_store::RunRecords;
    use lgwks_bot::script::{Scope, Tenant};
    use lgwks_bot::task::{DefinitionIdentity, RunStore, SealedAppend};

    let requested = std::env::args().nth(1);
    let runs: usize = match requested {
        Some(text) => text.parse()?,
        None => DEFAULT_RUNS.parse()?,
    };
    let body = vec![0x5A_u8; PAYLOAD_BYTES];
    let unique: [u8; 8] = lgwks_std::random::bytes()?;
    let hex: String = unique.iter().map(|byte| format!("{byte:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("lgwks-sealed-overhead-{hex}"));
    std::fs::create_dir_all(&dir)?;
    let provider: Arc<dyn lgwks_bot::sealed::keys::KeyProvider> =
        Arc::new(SoftwareProvider::open(&dir.join("keys"))?);
    let store = RunStore::open_sealed(dir.join("store.runstore"), provider)?;
    let definition = DefinitionIdentity::new(
        "sealed-overhead",
        1,
        lgwks_std::hash::blake3(b"sealed-overhead"),
        1,
    );
    let scope = Scope::root(Tenant::new("overhead")?);
    let mut append_samples = Vec::with_capacity(runs);
    let mut keys = Vec::with_capacity(runs);
    let mut id_runs = Vec::with_capacity(runs);
    for index in 0..runs {
        let run = RunId::from_hex(&format!("{:032x}", index.saturating_add(1)))?;
        let key = scope.enter(&format!("bench/{index}"))?.key();
        let started = std::time::Instant::now();
        store.append_with_expiry(SealedAppend::new(
            "overhead",
            run,
            key,
            &format!("bench/{index}"),
            &definition,
            body.clone(),
            None,
        ))?;
        append_samples.push(started.elapsed().as_micros());
        keys.push(key);
        id_runs.push(run);
    }
    store.verify_sealed_chain()?;
    report("sealed_append", &mut append_samples)?;
    let mut read_samples = Vec::with_capacity(runs);
    for (run, key) in id_runs.iter().copied().zip(keys.iter().copied()) {
        let started = std::time::Instant::now();
        match store.lookup("overhead", run, key)? {
            Some(back) => {
                read_samples.push(started.elapsed().as_micros());
                if back != body {
                    lgwks_std::trace::debug!(
                        run = run.id().to_hex(),
                        "sealed_overhead: the measured read returns different bytes"
                    );
                    return Err("the measured read returns different bytes".into());
                }
            }
            None => {
                lgwks_std::trace::debug!(
                    run = run.id().to_hex(),
                    "sealed_overhead: a freshly sealed record reads as absent"
                );
                return Err("a freshly sealed record never reads as absent".into());
            }
        }
    }
    report("sealed_read", &mut read_samples)?;
    // The unsealed baseline: the same records through an unsealed run store,
    // so the same frames, chain and fsync per append. The gap is exactly the
    // per-record key, its wrap and the AEAD.
    let plain = RunStore::open(dir.join("plain.runstore"))?;
    let mut write_samples = Vec::with_capacity(runs);
    for (index, (run, key)) in id_runs
        .iter()
        .copied()
        .zip(keys.iter().copied())
        .enumerate()
    {
        let started = std::time::Instant::now();
        RunRecords::append(
            &plain,
            "overhead",
            run,
            key,
            &format!("bench/{index}"),
            &definition,
            body.clone(),
        )?;
        write_samples.push(started.elapsed().as_micros());
    }
    report("plain_append", &mut write_samples)?;
    let mut read_plain_samples = Vec::with_capacity(runs);
    for (run, key) in id_runs.iter().copied().zip(keys.iter().copied()) {
        let started = std::time::Instant::now();
        let back = plain.lookup("overhead", run, key)?;
        read_plain_samples.push(started.elapsed().as_micros());
        if back.as_deref() != Some(body.as_slice()) {
            lgwks_std::trace::debug!(
                run = run.id().to_hex(),
                "sealed_overhead: the baseline misread its own record"
            );
            return Err("the baseline misread its own record".into());
        }
    }
    report("plain_read", &mut read_plain_samples)?;
    // Best effort: a directory the system refuses to remove is litter under
    // the temp root, not a wrong number.
    let _removed = std::fs::remove_dir_all(&dir);
    Ok(())
}
