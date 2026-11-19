//! The TTL sweep port: what a sealed store offers the driver that runs it.
//!
//! The kernel owns what a sweep is, [`SweepTarget`]; the driver that decides
//! when it runs is above the kernel and names this trait, never the reverse.
//!
//! [`SweepTarget`]: crate::sealed::sweep::SweepTarget

use std::time::Duration;

/// What the sweeper sweeps: a sealed store that shreds its expired records
/// on the injected clock.
///
/// Cloneable because the background loop clones the handle into each sweep
/// rather than holding it across the wait: a sweep that parked the shared
/// handle would serialize every caller behind the device.
pub trait SweepTarget: Clone + Send + Sync + 'static {
    /// The store's own refusal, which the sweep reports unchanged.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Shred every live record expired at `now`, on the injected clock, and
    /// report how many shredded.
    ///
    /// # Errors
    ///
    /// The store's own refusal when it cannot shred an expired record.
    fn sweep_once(&self, now: Duration) -> Result<u64, Self::Error>;
}
