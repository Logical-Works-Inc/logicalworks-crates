//! The rustc workspace wrapper of the macOS `it` group rows (`ci.yml`
//! `tests_macos`, compiled there with `rustc` alone, no dependencies).
//!
//! Cargo runs it as `it-group-rustc <rustc> <arguments>` for workspace crates
//! only. With `LWC_IT_GROUP` set, it adds `--cfg lwc_it_group` and
//! `--cfg lwc_it_group="<group>"`, the cfg that selects one group of
//! `lgwks_bot`'s `it` modules. Set in `RUSTFLAGS`, that cfg changed every
//! dependency's fingerprint per row, so only the row that saved the shared
//! rust-cache entry reused it: run 41837734726 compiled 16 crates in g3 and
//! 202 to 351 in each other row.

use std::env;
use std::ffi::OsString;
use std::io::Write;
use std::process::{Command, ExitCode};

/// Writes one line to a locked stderr. A failed write leaves nothing more to
/// report to, so the exit code alone carries the failure.
fn report(message: &str) {
    let stderr = std::io::stderr();
    let mut handle = stderr.lock();
    if writeln!(handle, "it-group-rustc: {message}").is_err() {
        return;
    }
}

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let Some(rustc) = arguments.next() else {
        report("cargo passes the rustc path first, and none came");
        return ExitCode::from(2);
    };
    let mut command = Command::new(&rustc);
    command.args(arguments);
    if let Some(group) = env::var_os("LWC_IT_GROUP").filter(|group| !group.is_empty()) {
        let mut value = OsString::from("lwc_it_group=\"");
        value.push(&group);
        value.push("\"");
        command.args(["--cfg", "lwc_it_group", "--cfg"]).arg(value);
    }
    let status = match command.status() {
        Ok(status) => status,
        Err(error) => {
            report(&format!("{} did not start: {error}", rustc.to_string_lossy()));
            return ExitCode::FAILURE;
        }
    };
    // A signal ends rustc with no code; a code past a byte is reported as 1.
    match status.code().map(u8::try_from) {
        Some(Ok(code)) => ExitCode::from(code),
        Some(Err(_)) | None => ExitCode::FAILURE,
    }
}
