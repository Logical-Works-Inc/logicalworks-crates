//! `lgwks tls`: install the agent clients' one TLS provider and say which won.
//!
//! The MCP, model and OTLP clients behind the `tls-ring` feature ship with no
//! provider of their own (ADR 0015), so a process names rustls on ring once
//! before the first handshake. This command makes that call the way an agent
//! harness does, twice, and reports whether the process agrees on ring: exit 0
//! when it does, 2 when another provider was installed first.

use std::io::Write;
use std::process::ExitCode;

use lgwks_deps::storefront::tls::install_ring;

/// Run `lgwks tls [--json]`; it takes no argument of its own.
pub(crate) fn run(
    args: &[String],
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<ExitCode> {
    if let Some(extra) = args.first() {
        writeln!(err, "lgwks tls: takes no argument, was given {extra}")?;
        return Ok(ExitCode::from(2));
    }
    // The second call is the one every later client makes; it must agree.
    let verdict = install_ring().and_then(|()| install_ring());
    let installed = verdict.is_ok();
    if let Err(refusal) = verdict {
        lgwks_std::trace::debug!(error = %refusal, "lgwks tls: the provider was refused");
        writeln!(err, "lgwks tls: {refusal}")?;
    }
    if json {
        writeln!(out, r#"{{"provider":"ring","installed":{installed}}}"#)?;
    } else if installed {
        writeln!(out, "tls: rustls on ring is this process's provider")?;
    }
    Ok(if installed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    })
}
