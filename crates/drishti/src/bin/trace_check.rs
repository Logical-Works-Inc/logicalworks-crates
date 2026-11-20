//! Trace checker CLI: Rust rewrite of `tools/check_trace.py`.
//!
//! Usage: `trace_check TRACE.jsonl SOURCE [--reduced]`. Validates headless
//! preview output against the analytic oracle in
//! [`drishti::preview::check_records`] and prints one JSON receipt. Traces
//! over 2 MiB are refused before parsing.
//!
//! Output runs through the shared [`drishti::cli`] convention: the binary
//! only implements `execute`, and every write error propagates as the typed
//! failure. Exit codes keep kernel parity: 0 checked, 1 rejected, 2 when the
//! report could not print.

use std::process::ExitCode;

use drishti::cli::{Failure, Streams, read_bounded};
use drishti::preview::check_records;

/// Maximum trace file bytes (2 MiB).
const TRACE_BYTES: u64 = 2_097_152;

/// Run the checker against the parsed arguments; returns the exit code.
fn execute(arguments: &[String], streams: &mut Streams<'_>) -> Result<u8, Failure> {
    let usage = "usage: trace_check TRACE.jsonl SOURCE [--reduced]";
    let mut parts = arguments.iter();
    let (trace_path, source_path, reduced) = match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some(trace), Some(source), None, _) => (trace, source, false),
        (Some(trace), Some(source), Some(flag), None) if flag == "--reduced" => {
            (trace, source, true)
        }
        _ => {
            let refusal = Err(usage.into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "execute: returning an error to the caller");
            return refusal;
        }
    };
    let trace_bytes = read_bounded(
        std::path::Path::new(trace_path),
        TRACE_BYTES,
        "oversized trace",
    )?;
    let trace_text = String::from_utf8(trace_bytes)?;
    let mut records = Vec::new();
    for line in trace_text.lines() {
        let record: lgwks_std::json::Value = lgwks_std::json::from_str(line)?;
        records.push(record);
    }
    let source_bytes = std::fs::read(source_path)?;
    let source_len = u64::try_from(source_bytes.len())?;
    let frames = check_records(&records, source_len, reduced)?;
    streams.tell(&format!(
        "{{\"scope\":\"headless_reducer_trace\",\"frames_checked\":{frames},\"native_verified\":false}}"
    ))?;
    Ok(0)
}

/// Checker entry point: exit code, never `process::exit`.
fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    drishti::cli::run("trace-check", |streams| execute(&arguments, streams))
}
