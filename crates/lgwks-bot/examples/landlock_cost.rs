//! p50/p95/p99 of one Landlock-confined spawn under 1,000 rules, beside the same
//! spawn under a few (#1114).
//!
//! ```text
//! cargo run --locked --release -p lgwks_bot --features "rt time sync process" \
//!     --example landlock_cost -- "$(mktemp -d)" [runs]
//! ```
//!
//! The first argument is an empty directory the run fills with 1,000
//! directories of one file each. Each run is one `Supervisor::run_process` of
//! the shared probe (`tests/support/landlock_probe.rs`, the one
//! `sim_landlock` asserts), timed from the request to the reaped exit in
//! microseconds. The two rule sets alternate, so a host that slows mid-run
//! slows both. Two lines are printed through the shared instrument
//! (`support/measure.rs`), each named by its rule count.
//!
//! This is a measurement, not a gate: `sim_landlock`'s
//! `a_thousand_rule_allowlist_is_applied_by_the_kernel` asserts the kernel
//! applies the list. It is an example rather than a test so it may read the
//! clock and print. Landlock is Linux only, so elsewhere it refuses at once.

#[cfg(target_os = "linux")]
#[path = "support/measure.rs"]
mod measure;

#[cfg(target_os = "linux")]
#[path = "../tests/support/landlock_probe.rs"]
mod landlock_probe;

/// Rules in the large allowlist: a 500-entry home folder walked from `/`, with
/// margin.
#[cfg(target_os = "linux")]
const RULES: usize = 1_000;

/// Runs of each rule set when the command line names no count.
#[cfg(target_os = "linux")]
const DEFAULT_RUNS: usize = 200;

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write as _;
    use std::time::Instant;

    use lgwks_bot::rt::process::{Confinement, ProcessSpec};
    use lgwks_bot::rt::runtime::Runtime;
    use lgwks_bot::rt::supervise::Supervisor;

    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().ok_or("name an empty scratch directory")?);
    let runs = match args.next() {
        Some(text) => text.parse()?,
        None => DEFAULT_RUNS,
    };
    let probe = landlock_probe::probe(&root, RULES)?;
    let labels = [
        format!("landlock_{}_rules", probe.full.rules().len()),
        format!("landlock_{}_rules", probe.few.rules().len()),
    ];
    let specs = [probe.full, probe.few].map(|rules| {
        let mut spec = ProcessSpec::new("/bin/sh");
        spec.arg("-c").arg(&probe.script);
        spec.confinement(Confinement::Landlock(rules));
        spec
    });
    let runtime = Runtime::new()?;
    let mut supervisor = Supervisor::new(1);
    let mut samples = [Vec::with_capacity(runs), Vec::with_capacity(runs)];
    for _ in 0..runs {
        for (spec, taken) in specs.iter().zip(samples.iter_mut()) {
            let started = Instant::now();
            let run = runtime.block_on(supervisor.run_process(spec))?;
            taken.push(started.elapsed().as_micros());
            if run.exit_code() != Some(0) {
                let code = run.exit_code();
                lgwks_std::trace::error!(
                    ?code,
                    "a confined probe failed, so the kernel did not apply the list as granted"
                );
                return Err(format!("a confined probe exited {code:?}").into());
            }
        }
    }
    let mut out = std::io::stdout().lock();
    for (label, mut taken) in labels.iter().zip(samples) {
        let line = measure::Summary::of(&mut taken).line(label);
        writeln!(out, "{line} n={}", taken.len())?;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    Err("Landlock confines a child on Linux only".into())
}
