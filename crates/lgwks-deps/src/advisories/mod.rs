//! Offline RustSec advisory evaluation: a `Cargo.lock` against a local checkout
//! of the advisory database (#799). `lgwks-deps advisories` is the command.

mod cli;
mod db;
mod parse;
mod semver;
mod types;

pub use cli::run;
pub(crate) use db::audit_lockfile;
pub(crate) use types::{AdvisoryMatch, AdvisoryReport};

#[cfg(test)]
pub(crate) use db::{compute_db_digest, evaluate_lock};
#[cfg(test)]
pub(crate) use parse::parse_advisory;
#[cfg(test)]
pub(crate) use types::Advisory;
#[cfg(test)]
pub(crate) use types::{AdvisoryError, AdvisoryKind};
