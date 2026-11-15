//! Native host contract, rebuilt in Rust.
//!
//! Absorbed from `surfaces/native-host` (kernel#850 row 15). The kernel
//! implemented this in Swift; product repositories are pure Rust, so the
//! contract is rebuilt here: [`broker`] (fan-out, rate limit, registry,
//! stamping), [`capability`] (guard), [`pattern`] (matcher),
//! [`workspace_layout`](crate::canvas_protocol::workspace_layout) validation
//! reuse plus strict [`crate::canvas_protocol::workspace_layout::load_validated`]
//! wire loading,
//! [`session`] and [`governance`] stores, [`telemetry`] (ring buffer and tape
//! reader), and the [`command_bar`] intent pipeline. SwiftUI view code is
//! replaced by Drishti's own presentation; only portable logic is rebuilt.
//!
//! Three issue-driven additions beyond the Swift behavior, all tested:
//! tenant-scoped subscription delivery in [`broker`], the command-bar
//! capability gate on registration, and the broker audit sink that appends
//! every routed envelope to the governance log.

pub mod attest;
pub mod broker;
pub mod capability;
pub mod cbor;
pub mod command_bar;
pub mod governance;
pub mod pattern;
pub mod protocol;
pub mod session;
pub mod telemetry;
pub mod time;

/// Turn a failed condition into its typed error, so a validation step reads
/// as one `?` and never as a bare `return Err(..)`. The error is built only
/// when the condition fails.
pub(crate) fn ensure<E>(condition: bool, error: impl FnOnce() -> E) -> Result<(), E> {
    condition.then_some(()).ok_or_else(error)
}
