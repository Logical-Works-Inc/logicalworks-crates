//! `cargo bench --bench durable -- <args>`: the durable-workflow report, run through
//! the built `lgwks-bench-durable` binary as a user would run it.
//!
//! The target has no harness of its own. It starts the binary cargo built beside
//! it, gives it every argument after the `--` except the `--bench` flag cargo
//! adds, shares this process's stdin, stdout and stderr with it, and exits with
//! the binary's exit code. A binary that cannot start, or that ends without an
//! exit code, is a nonzero exit with the reason on stderr.

#[path = "durable/forward.rs"]
mod forward;

use std::io::Write;
use std::process::{Command, ExitCode, ExitStatus};

/// The exit status when even the failure line could not be written.
const EXIT_NO_STDERR: u8 = 3;

/// The exit status of a bench that could not run the binary to an exit code.
const EXIT_NOT_RUN: u8 = 1;

/// The exit code of the finished binary, or the reason it has none.
fn exit_of(status: ExitStatus) -> ExitCode {
    let Some(code) = status.code() else {
        return fail("lgwks-bench-durable ended without an exit code (killed by a signal)");
    };
    match u8::try_from(code) {
        Ok(byte) => ExitCode::from(byte),
        Err(source) => fail(&format!(
            "lgwks-bench-durable exited with {code}, which is no exit status: {source}"
        )),
    }
}

/// Say why the bench did not run, and exit nonzero.
fn fail(reason: &str) -> ExitCode {
    let mut err = std::io::stderr().lock();
    if writeln!(err, "bench durable: {reason}").is_err() {
        return ExitCode::from(EXIT_NO_STDERR);
    }
    ExitCode::from(EXIT_NOT_RUN)
}

fn main() -> ExitCode {
    let args = forward::forwarded(std::env::args_os().skip(1));
    match Command::new(env!("CARGO_BIN_EXE_lgwks-bench-durable"))
        .args(args)
        .status()
    {
        Ok(status) => exit_of(status),
        Err(source) => fail(&format!("could not start lgwks-bench-durable: {source}")),
    }
}
