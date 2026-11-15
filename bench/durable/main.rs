//! `lgwks-bench-durable`: the `lgwks_bot` durable workflow engine on a ten-step
//! flow, measured as a user would see it.
//!
//! The flow is two calls to fixed-latency upstreams (2 ms and 20 ms, 20% of
//! attempts failing and retried under the crate's own policy), one approval and
//! seven local steps, every step a durable record in an on-disk run store. The
//! report is throughput, p50/p95/p99 latency, peak resident set and retries; with
//! `--recover` it is instead the time from `kill -9` of a process holding parked
//! runs to every one of them done again.
//!
//! The report goes to stdout, as text or, with `--json`, as one JSON document.
//! Progress and the failure line go to stderr. The exit status is 0, 1 for a run
//! failure, 2 for a usage failure and 3 when even the failure line could not be
//! written.

mod args;
mod error;
mod fleet;
mod flow;
mod measure;
mod recover;
mod report;
#[cfg(test)]
mod sim;
#[cfg(test)]
mod sim_chaos;
#[cfg(test)]
mod sim_edges;
#[cfg(test)]
mod sim_fools;
#[cfg(test)]
mod sim_kit;
mod stats;
mod workdir;
mod world;

use std::io::Write;
use std::process::ExitCode;

use args::{Args, Mode};
use error::{BenchError, EXIT_NO_STDERR};
use lgwks_bot::clock::Clock;
use workdir::Workdir;

/// A line on stderr.
fn progress(text: &str) -> Result<(), BenchError> {
    let mut err = std::io::stderr().lock();
    writeln!(err, "{text}").map_err(|source| BenchError::Io {
        context: "writing progress to stderr",
        source,
    })
}

/// The report on stdout: one document, nothing before it, nothing after.
fn emit(text: &str) -> Result<(), BenchError> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{text}")
        .and_then(|()| out.flush())
        .map_err(|source| BenchError::Io {
            context: "writing the report to stdout",
            source,
        })
}

/// Do what the command line asked.
fn run() -> Result<(), BenchError> {
    let args: Args = args::parse(std::env::args_os().skip(1))?;
    // The live binary waits real time: the one wall clock every wait is made on.
    let clock = Clock::wall();
    match args.mode {
        Mode::ParkChild => recover::park_child(
            &args,
            clock,
            std::io::stdout().lock(),
            std::io::stdin().lock(),
        ),
        Mode::Flow => {
            let work = Workdir::create(args.work_dir.as_deref())?;
            progress(&format!("measuring {} runs over two tenants", args.runs))?;
            let doc = measure::flow(&args, work.path(), clock)?;
            emit(&if args.json {
                report::json(&doc)?
            } else {
                report::flow_text(&doc)
            })
        }
        Mode::Recover => {
            let work = Workdir::create(args.work_dir.as_deref())?;
            progress(&format!(
                "recovering {} parked runs over {} rounds",
                args.runs, args.rounds
            ))?;
            let doc = recover::recover(&args, work.path(), clock, &mut progress)?;
            emit(&if args.json {
                report::json(&doc)?
            } else {
                report::recover_text(&doc)
            })
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let mut err = std::io::stderr().lock();
            if writeln!(err, "{}", error.line()).is_err() {
                return ExitCode::from(EXIT_NO_STDERR);
            }
            ExitCode::from(error.exit_code())
        }
    }
}
