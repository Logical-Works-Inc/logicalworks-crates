//! Offline RustSec advisory evaluation: lockfiles against a local checkout of
//! the advisory database (#799, #942). `audit` is the library entry and
//! `audit_files`, which `lgwks-deps advisories` calls, reads lockfiles from
//! disk for it. Both are support code, hidden from the rendered docs, so they
//! are named here and not linked.
//!
//! Matching agrees with Cargo's `VersionReq` on every requirement RustSec
//! loads, the pre-release gate included, so `patched = [">= 1.2.3"]` never
//! reads `2.0.0-alpha.1` as patched (ADR 0038). A requirement RustSec would
//! refuse to load is refused here too, typed, never skipped.

mod db;
mod parse;
mod semver;
#[cfg(test)]
mod sim_names;
mod types;

#[doc(hidden)]
pub use db::{audit, audit_files};
#[doc(hidden)]
pub use types::{
    AdvisoryError, AdvisoryKind, AdvisoryMatch, AdvisoryReport, DatabaseIdentity, Lockfile,
};

#[cfg(test)]
pub(crate) use db::{compute_db_digest, evaluate_lock};
#[cfg(test)]
pub(crate) use parse::parse_advisory;
#[cfg(test)]
pub(crate) use types::Advisory;
