//! `lgwks`: one-file apps and the Rust they compile to.
//!
//! The storefront users install, so `cargo install lgwks_deps` gives `lgwks`.
//! Every subcommand takes `--json`: output for humans by default, the same
//! facts as JSON for agents.

use std::io::Write;
use std::process::ExitCode;

mod check;
#[path = "../shared/children.rs"]
mod children;
mod common;
mod expand;
mod id;
mod legacy;
mod new;
mod run;

use common::settle;

/// What `lgwks` was asked to do, with `--json` taken out.
struct Request {
    /// The subcommand: `new`, `check`, `expand`, `run`, `legacy` or `id`.
    command: Option<String>,
    /// The subcommand's own arguments.
    args: Vec<String>,
    /// Whether the facts print as one JSON object.
    json: bool,
}

/// Split `--json` out of the arguments, leaving the command and its own.
fn request(arguments: &[String]) -> Request {
    let mut command: Option<String> = None;
    let mut args = Vec::new();
    let mut json = false;
    for argument in arguments {
        if argument.as_str() == "--json" {
            json = true;
        } else if command.is_none() {
            command = Some(argument.clone());
        } else {
            args.push(argument.clone());
        }
    }
    Request {
        command,
        args,
        json,
    }
}

/// Route the request to its subcommand.
fn dispatch(
    request: Request,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<ExitCode> {
    match request.command.as_deref() {
        Some("new") => new::run(request.args.as_slice(), request.json, out, err),
        Some("check") => check::run(request.args.as_slice(), request.json, out, err),
        Some("expand") => expand::run(request.args.as_slice(), request.json, out, err),
        Some("run") => run::run(request.args.as_slice(), request.json, out, err),
        Some("legacy") => legacy::run(request.args.as_slice(), request.json, out, err),
        Some("id") => id::run(request.args.as_slice(), request.json, out, err),
        Some("-h" | "--help" | "help") => {
            write!(out, "{}", common::USAGE).map(|()| ExitCode::SUCCESS)
        }
        Some(other) => {
            writeln!(err, "lgwks: unknown command {other}")?;
            write!(err, "{}", common::USAGE).map(|()| ExitCode::from(2))
        }
        None => {
            writeln!(err, "lgwks: no command given")?;
            write!(err, "{}", common::USAGE).map(|()| ExitCode::from(2))
        }
    }
}

/// Locks both output handles once and runs the requested command.
fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let stderr = std::io::stderr();
    let mut err = stderr.lock();
    settle(dispatch(request(arguments.as_slice()), &mut out, &mut err))
}
