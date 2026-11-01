//! What a 10-step durable run costs on the file store, end to end through
//! `Host::run`: admission, ten `remember` steps, ten group-committed records and
//! the report.
//!
//! ```text
//! cargo run -p lgwks_bot --features script,ephemeral --release --example durable_steps
//! ```
//!
//! Written against the host's public doors alone, so the same source measures a
//! build before and after a change to how the host reaches its store. Wall time
//! varies with the machine and the device's flush; the distribution's shape and
//! the before/after difference on one machine are the measurement.
//!
//! The output is written through `std::io::Write` rather than `println!` because
//! `clippy::print_stdout` is forbidden workspace-wide.

use std::error::Error;
use std::future::Future;
use std::io::Write;
use std::pin::Pin;
use std::time::Instant;

use lgwks_bot::script::{FlowError, Scope, remember};
use lgwks_bot::task::{Host, Task, task};

#[path = "support/measure.rs"]
mod measure;
#[path = "../tests/support/scratch.rs"]
mod scratch;

use scratch::Scratch;

/// How many runs are timed.
const RUNS: u32 = 200;

/// The steps of one run, each its own durable record.
const STEPS: [&str; 10] = ["s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9"];

/// The future the task body returns.
type StepsFuture = Pin<Box<dyn Future<Output = Result<u32, FlowError>>>>;

/// Ten durable steps, each adding its index to the input.
fn ten_steps(scope: Scope, input: u32) -> StepsFuture {
    Box::pin(async move {
        let mut total = input;
        for (index, step) in (0u32..).zip(STEPS) {
            total = remember(&scope, step, || async move {
                Ok::<_, FlowError>(total.saturating_add(index))
            })
            .await?;
        }
        Ok(total)
    })
}

/// Time [`RUNS`] fresh 10-step runs, each under its own run id.
fn main() -> Result<(), Box<dyn Error>> {
    let scratch = Scratch::new("durable-steps")?;
    let host = Host::builder("bench")?.run_store(scratch.path())?.build()?;
    let body: fn(Scope, u32) -> StepsFuture = ten_steps;
    let work: Task<fn(Scope, u32) -> StepsFuture> = task("ten", body)?;
    let mut samples = Vec::with_capacity(usize::try_from(RUNS)?);
    let started = Instant::now();
    for input in 0..RUNS {
        let at = Instant::now();
        let report = lgwks_bot::block_on(host.run(&work, input));
        samples.push(at.elapsed().as_micros());
        if !report.disposition().is_success() {
            let refusal = Err(format!("run {input} failed: {:?}", report.error()).into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "main: returning an error to the caller");
            return refusal;
        }
    }
    let elapsed = started.elapsed();
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "{}",
        measure::Summary::of(&mut samples).line(&format!("10-step durable run n={RUNS}"))
    )?;
    writeln!(out, "elapsed={elapsed:?}")?;
    Ok(())
}
