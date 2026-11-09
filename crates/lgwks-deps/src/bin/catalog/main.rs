//! The `catalog` binary: what the `catalog` lane runs, over the one implementation
//! `lgwks-deps catalog` also runs (ADR 0023).

use std::io::{self, Write};
use std::process::ExitCode;

#[path = "../../catalog_cmd.rs"]
mod catalog_cmd;

/// Runs the command with this process's arguments and reports its exit code.
fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let stderr = io::stderr();
    let mut err = stderr.lock();
    match catalog_cmd::handle_catalog(&arguments, &mut out, &mut err) {
        Ok(code) => code,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            if writeln!(err, "lgwks-deps catalog: {error}").is_err() {
                return ExitCode::from(2);
            }
            ExitCode::from(2)
        }
    }
}
