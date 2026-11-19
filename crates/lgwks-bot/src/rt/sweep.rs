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
//! The driver is generic over the kernel port
//! [`SweepTarget`](crate::sealed::sweep::SweepTarget): the kernel owns what a
//! sweep is and each store reports its own typed error, while this module owns
//! when the sweep runs. A sealed journal sweeps inline through its own
//! `sweep_sealed`, on the handle the caller already holds mutably for appends.
//!
//! # Bounds
//!
//! One task, one token, no queue. The wait is never shorter than
//! `SWEEP_FLOOR` (1 ms), so a zero interval cannot busy-spin the runtime. A
//! blocking pool past its job bound refuses the sweep with
//! [`lgwks_std::task::SpawnError`]; that sweep is shed and the next interval
//! tries again, so a saturated pool is never a growing backlog.
//!
//! [`JoinSet`]: crate::rt::task::JoinSet
//! [`CancellationToken`]: crate::rt::sync::CancellationToken

use std::time::Duration;

use lgwks_std::task::SpawnError;

use crate::clock::Clock;
use crate::rt::sync::CancellationToken;
use crate::rt::task::JoinSet;
use crate::rt::time::timeout;
use crate::sealed::sweep::SweepTarget;

/// The shortest wait between two sweeps: a smaller interval is raised to it.
const SWEEP_FLOOR: Duration = Duration::from_millis(1);

/// The owned TTL driver: one [`JoinSet`], one [`CancellationToken`], and the
/// store it sweeps.
///
/// The one task this sweeper owns starts in [`Sweeper::spawn`] and ends in
/// [`Sweeper::shutdown`]. Dropping the set aborts whatever has not finished,
/// so the task cannot outlive the sweeper.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use std::sync::Arc;
/// use std::time::Duration;
/// use lgwks_bot::clock::Clock;
/// use lgwks_bot::effect::RunId;
/// use lgwks_bot::rt::runtime::block_on;
/// use lgwks_bot::rt::sweep::Sweeper;
/// use lgwks_bot::rt::task::yield_now;
/// use lgwks_bot::rt::time::timeout;
/// use lgwks_bot::sealed::keys::{KeyProvider, SoftwareProvider};
/// use lgwks_bot::script::{Scope, Tenant};
/// use lgwks_bot::task::{DefinitionIdentity, RunStore, SealedAppend, SealedStepRead};
///
/// let dir = std::env::temp_dir().join("lgwks-doc-sweeper-struct");
/// std::fs::create_dir_all(&dir)?;
/// let provider: Arc<dyn KeyProvider> = Arc::new(SoftwareProvider::open(&dir)?);
/// let store = RunStore::open_sealed(dir.join("store.runstore"), provider)?;
/// let definition = DefinitionIdentity::new("doc", 1, lgwks_std::hash::blake3(b"doc"), 1);
/// let run = RunId::from_hex("00000000000000000000000000000002")?;
/// let key = Scope::root(Tenant::new("acme")?).enter("session")?.key();
/// store.append_with_expiry(SealedAppend::new(
///     "acme", run, key, "session", &definition, b"token".to_vec(),
///     Some(Duration::from_secs(5)),
/// ))?;
///
/// // The clock is virtual and moved past the expiry, never slept to.
/// let clock = Clock::virtual_at(Duration::from_secs(1));
/// clock.advance(Duration::from_secs(10))?;
/// let watched = store.clone();
/// let swept = block_on(async move {
///     let sweeper = Sweeper::spawn(watched.clone(), clock, Duration::from_millis(1));
///     let seen = timeout(Duration::from_secs(30), async {
///         while !matches!(watched.read_sealed("acme", run, key), Ok(SealedStepRead::Erased { .. })) {
///             yield_now().await;
///         }
///     })
///     .await;
///     sweeper.shutdown().await;
///     seen.is_ok()
/// });
/// assert!(swept, "the due record was never shredded");
/// drop(store);
/// std::fs::remove_dir_all(&dir)?;
/// # Ok(()) }
/// ```
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
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use lgwks_bot::clock::Clock;
    /// use lgwks_bot::rt::runtime::block_on;
    /// use lgwks_bot::rt::sweep::Sweeper;
    /// use lgwks_bot::sealed::keys::{KeyProvider, SoftwareProvider};
    /// use lgwks_bot::task::RunStore;
    ///
    /// let dir = std::env::temp_dir().join("lgwks-doc-sweeper-spawn");
    /// std::fs::create_dir_all(&dir)?;
    /// let provider: Arc<dyn KeyProvider> = Arc::new(SoftwareProvider::open(&dir)?);
    /// let store = RunStore::open_sealed(dir.join("store.runstore"), provider)?;
    /// let clock = Clock::virtual_at(Duration::from_secs(1));
    /// block_on(async move {
    ///     // A zero interval is raised to the one-millisecond floor, so the
    ///     // sweeper does not spin the runtime.
    ///     let sweeper = Sweeper::spawn(store, clock, Duration::ZERO);
    ///     sweeper.shutdown().await;
    /// });
    /// std::fs::remove_dir_all(&dir)?;
    /// # Ok(()) }
    /// ```
    pub fn spawn<T>(target: T, clock: Clock, interval: Duration) -> Self
    where
        T: SweepTarget,
    {
        let token = CancellationToken::new();
        let child = token.child_token();
        let mut set = JoinSet::new();
        let floor = SWEEP_FLOOR.max(interval);
        set.spawn(async move {
            run_loop(target, clock, floor, child).await;
        });
        Self { set, token }
    }

    /// Stop the sweeper: cancel the token and join the owned task. The sweep
    /// in flight, if any, runs to its typed outcome on the blocking pool;
    /// nothing new starts after the cancel.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use lgwks_bot::clock::Clock;
    /// use lgwks_bot::rt::runtime::block_on;
    /// use lgwks_bot::rt::sweep::Sweeper;
    /// use lgwks_bot::rt::time::timeout;
    /// use lgwks_bot::sealed::keys::{KeyProvider, SoftwareProvider};
    /// use lgwks_bot::task::RunStore;
    ///
    /// let dir = std::env::temp_dir().join("lgwks-doc-sweeper-shutdown");
    /// std::fs::create_dir_all(&dir)?;
    /// let provider: Arc<dyn KeyProvider> = Arc::new(SoftwareProvider::open(&dir)?);
    /// let store = RunStore::open_sealed(dir.join("store.runstore"), provider)?;
    /// let clock = Clock::virtual_at(Duration::from_secs(1));
    /// let joined = block_on(async move {
    ///     // The wait is one hour: shutdown ends it at once, inside the bound.
    ///     let sweeper = Sweeper::spawn(store, clock, Duration::from_secs(3600));
    ///     timeout(Duration::from_secs(30), sweeper.shutdown()).await
    /// });
    /// assert!(joined.is_ok(), "shutdown waited out the interval");
    /// std::fs::remove_dir_all(&dir)?;
    /// # Ok(()) }
    /// ```
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
    T: SweepTarget,
{
    loop {
        if token.is_cancelled() {
            break;
        }
        if timeout(interval, token.cancelled()).await.is_ok() {
            break;
        }
        let now = clock.now();
        let sweep = target.clone();
        match lgwks_std::task::try_spawn_blocking(move || sweep.sweep_once(now)) {
            Ok(job) => match job.await {
                Ok(shredded) => {
                    lgwks_std::trace::debug!(shredded, "sealed-sweep: the TTL sweep ran");
                }
                Err(error) => {
                    lgwks_std::trace::debug!(error = ?error, "sealed-sweep: the TTL sweep refused");
                }
            },
            Err(error) => {
                let refused: SpawnError = error;
                lgwks_std::trace::debug!(error = ?refused, "sealed-sweep: the blocking pool refused the sweep");
            }
        }
    }
}
