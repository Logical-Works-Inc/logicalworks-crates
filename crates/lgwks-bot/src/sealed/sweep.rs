//! The owned TTL driver: one [`JoinSet`], one [`CancellationToken`], and the
//! sealed store it sweeps.
//!
//! `Sweeper::spawn` places exactly one task, which loops: wait for the
//! interval or the token, whichever comes first, then sweep expired records
//! on the blocking pool through [`lgwks_std::task::try_spawn_blocking`] —
//! never on the awaiting task, so a slow device parks the pool and not the
//! runtime. `Sweeper::shutdown` cancels the token and joins the task, so
//! nothing outlives the sweeper that made it.
//!
//! The driver sweeps run stores: their handles clone across the loop, so the
//! background task never borrows the handle its caller still uses. A sealed
//! journal sweeps inline through its own `sweep_sealed`, on the handle the
//! caller already holds mutably for appends.
//!
//! A module of its own rather than a section of either store because the file
//! size law counts lines, not concerns: the driver owns its task, each store
//! owns its bytes, and neither reaches into the other except through
//! [`SweepTarget::sweep_once`].
//!
//! [`JoinSet`]: crate::rt::task::JoinSet
//! [`CancellationToken`]: crate::rt::sync::CancellationToken
//! [`SweepTarget::sweep_once`]: crate::sealed::sweep::SweepTarget::sweep_once

use std::time::Duration;

use crate::clock::Clock;
use crate::journal::JournalError;
use crate::rt::sync::CancellationToken;
use crate::rt::task::JoinSet;
use crate::task::StoreError;

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
    fn sweep_once(&self, now: Duration) -> Result<u64, Self::Error>;
}

/// Why a background sweep refused: the journal's or the run store's own
/// refusal, unchanged, so a sweep failure names the same fact the direct
/// call would have.
#[derive(Debug)]
#[non_exhaustive]
pub enum SweepError {
    /// The journal's sweep refused.
    Journal {
        /// The journal's own refusal.
        source: JournalError,
    },
    /// The run store's sweep refused.
    Store {
        /// The run store's own refusal.
        source: StoreError,
    },
}

impl core::fmt::Display for SweepError {
    /// The store's refusal, unchanged.
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::Journal { ref source } => {
                write!(formatter, "the TTL sweep's journal refused: {source}")
            }
            Self::Store { ref source } => {
                write!(formatter, "the TTL sweep's run store refused: {source}")
            }
        }
    }
}

impl std::error::Error for SweepError {
    /// The swept store's own error.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Journal { ref source } => Some(source),
            Self::Store { ref source } => Some(source),
        }
    }
}

impl From<JournalError> for SweepError {
    /// A journal refusal reaches the sweep's caller as itself.
    fn from(source: JournalError) -> Self {
        Self::Journal { source }
    }
}

impl From<StoreError> for SweepError {
    /// A run-store refusal reaches the sweep's caller as itself.
    fn from(source: StoreError) -> Self {
        Self::Store { source }
    }
}

/// The owned TTL driver: one [`JoinSet`], one [`CancellationToken`], and the
/// store it sweeps.
///
/// The one task this sweeper owns starts in [`Sweeper::spawn`] and ends in
/// [`Sweeper::shutdown`]. Dropping the set aborts whatever has not finished,
/// so the task cannot outlive the sweeper.
pub struct Sweeper {
    /// The one task this sweeper owns.
    set: JoinSet<()>,
    /// The token the loop listens to: cancelling it ends the wait at once,
    /// rather than at the next interval.
    token: CancellationToken,
}

impl core::fmt::Debug for Sweeper {
    /// The shape without the internals: the task set and the token carry no
    /// facts a log needs.
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("Sweeper").finish_non_exhaustive()
    }
}

impl Sweeper {
    /// Start sweeping `target` on `clock` every `interval`.
    ///
    /// An interval below one millisecond is treated as one millisecond: a
    /// zero interval would busy-spin the runtime it was meant to serve. Must
    /// be called in a runtime context — the owned task is placed on the
    /// driver, like every other task this crate starts.
    pub fn spawn<T>(target: T, clock: Clock, interval: Duration) -> Self
    where
        T: SweepTarget<Error = SweepError>,
    {
        let token = CancellationToken::new();
        let child = token.child_token();
        let mut set = JoinSet::new();
        let floor = Duration::from_millis(1).max(interval);
        set.spawn(async move {
            run_loop(target, clock, floor, child).await;
        });
        Self { set, token }
    }

    /// Stop the sweeper: cancel the token and join the owned task. The sweep
    /// in flight, if any, runs to its typed outcome on the blocking pool;
    /// nothing new starts after the cancel.
    pub async fn shutdown(mut self) {
        self.token.cancel();
        while let Some(outcome) = self.set.join_next().await {
            if let Err(error) = outcome {
                lgwks_std::trace::debug!(error = ?error, "sealed-sweep: the sweep task ended without joining");
            }
        }
    }
}

/// The sweeper's one task: wait, sweep, repeat until the token cancels.
async fn run_loop<T>(target: T, clock: Clock, interval: Duration, token: CancellationToken)
where
    T: SweepTarget<Error = SweepError>,
{
    loop {
        if token.is_cancelled() {
            break;
        }
        if crate::rt::time::timeout(interval, token.cancelled())
            .await
            .is_ok()
        {
            break;
        }
        let now = clock.now();
        let sweep = target.clone();
        let job = match lgwks_std::task::try_spawn_blocking(move || sweep.sweep_once(now)) {
            Ok(job) => job,
            Err(error) => {
                lgwks_std::trace::debug!(error = ?error, "sealed-sweep: the blocking pool refused the sweep");
                continue;
            }
        };
        match job.await {
            Ok(shredded) => {
                if shredded != 0 {
                    lgwks_std::trace::debug!(
                        shredded,
                        "sealed-sweep: the TTL sweep shredded expired records"
                    );
                }
            }
            Err(error) => {
                lgwks_std::trace::debug!(error = ?error, "sealed-sweep: the TTL sweep refused");
            }
        }
    }
}
