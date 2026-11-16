//! Which arguments `cargo bench` hands the durable binary.
//!
//! Cargo runs a `harness = false` bench with its own `--bench` flag in front of
//! whatever the user typed after `--`. The binary takes no such flag, so the flag
//! cargo added is dropped and the rest goes through in order, untouched. Only
//! the first `--bench` is cargo's; a second is the user's and reaches the binary,
//! which refuses it as an unknown flag.

use std::ffi::OsString;

/// The arguments the durable binary is run with: `args` without the first `--bench`.
pub(crate) fn forwarded(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut dropped = false;
    args.into_iter()
        .filter(|arg| {
            let cargo_flag = !dropped && arg == "--bench";
            dropped |= cargo_flag;
            !cargo_flag
        })
        .collect()
}
