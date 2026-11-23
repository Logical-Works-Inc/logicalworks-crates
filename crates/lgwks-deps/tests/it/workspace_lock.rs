//! The workspace's own `Cargo.lock`, read with the shipped lock reader.
//!
//! Sources: rustc's future-incompatibility report (`cargo report
//! future-incompatibilities`), rust-lang/rust#74840 (static of uninhabited
//! type) and rust-lang/rust#127909 (re-export of a private `extern crate`).
//! Requirement proved: INV-SEC-AUDIT-2, the lock resolves no package rustc
//! will refuse (crates#1122, zed#67338).

use std::error::Error;
use std::path::Path;

use lgwks_deps::lock::parse;

/// A shared test result.
type TestResult = Result<(), Box<dyn Error>>;

/// Packages rustc reports as future-incompatible, by name and version.
const FUTURE_INCOMPATIBLE: [(&str, &str); 2] = [("block", "0.1.6"), ("proc-macro-error2", "2.0.1")];

#[test]
/// gpui 0.2.2 from crates.io pulled both packages; the Zed main pin dropped
/// them, and this fails the suite if either comes back.
fn the_workspace_lock_resolves_no_future_incompatible_crate() -> TestResult {
    let lock = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let resolved = parse(&std::fs::read_to_string(lock)?)?;
    let flagged: Vec<String> = resolved
        .iter()
        .filter(|package| {
            FUTURE_INCOMPATIBLE
                .iter()
                .any(|&(name, version)| package.name() == name && package.version() == version)
        })
        .map(|package| format!("{} {}", package.name(), package.version()))
        .collect();
    assert_eq!(
        flagged,
        Vec::<String>::new(),
        "future-incompatible crates resolved"
    );
    Ok(())
}
