//! The owner of every admitted run's task.
//!
//! A request's run is spawned here, never from the connection's own future.
//! The reason is the store, not speed: a run awaits its append through
//! `Awaiting` (`journal/owner/store.rs`), and dropping that future before the
//! owner answers marks the request abandoned and poisons the tenant's store
//! handle until a reopen (INV-BOT-15). A client that hangs up drops the
//! connection's future, so the run must not live in it. The set owns the task
//! instead: a finished task is reaped on the next spawn, a drain joins what is
//! left, and past the drain deadline the rest are aborted. The ceiling on how
//! many tasks the set holds is the serve's in-flight ceiling, held by each
//! run's own permit.

use std::future::Future;
use std::sync::{Mutex, MutexGuard};

use crate::rt::task::JoinSet;

/// The set of admitted runs, behind one short lock held only to spawn or reap.
pub(super) struct Runs {
    /// The tasks, reaped as they finish.
    set: Mutex<JoinSet<()>>,
}

impl Runs {
    /// An empty set.
    pub(super) fn new() -> Self {
        Self {
            set: Mutex::new(JoinSet::new()),
        }
    }

    /// The set, recovered if a panicking holder poisoned the lock: the set is
    /// a plain task list and is consistent after any single call returns.
    fn held(&self) -> MutexGuard<'_, JoinSet<()>> {
        match self.set.lock() {
            Ok(held) => held,
            Err(poisoned) => {
                lgwks_std::trace::debug!("serve: the run set lock was poisoned; recovered");
                poisoned.into_inner()
            }
        }
    }

    /// Own `run` until it finishes, reaping every task that already has.
    pub(super) fn spawn<F>(&self, run: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut held = self.held();
        while let Some(finished) = held.try_join_next() {
            if let Err(cause) = finished {
                lgwks_std::trace::debug!(error = ?cause, "serve: a run ended without finishing");
            }
        }
        held.spawn(run);
    }

    /// Take the set's tasks out, to join them without holding the lock.
    pub(super) fn take(&self) -> JoinSet<()> {
        std::mem::take(&mut *self.held())
    }
}

/// Join every task in `set`, or abort what is left when `grace` passes.
pub(super) async fn join_or_abort(mut set: JoinSet<()>, grace: std::time::Duration) -> usize {
    let joined = crate::rt::time::timeout(grace, async {
        while let Some(finished) = set.join_next().await {
            if let Err(cause) = finished {
                lgwks_std::trace::debug!(error = ?cause, "serve: a run ended without finishing");
            }
        }
    })
    .await;
    if joined.is_err() {
        let left = set.len();
        lgwks_std::trace::debug!(left, "serve: the runs outlived the drain; aborting them");
        set.abort_all();
        return left;
    }
    0
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use lgwks_std::seeded::Seeded;

    use super::super::hosts::Hosts;
    use super::super::{CANCELLED_JOIN_GRACE, CancellationToken, Shared, drain_owned};
    use super::{Runs, join_or_abort};

    /// Requirement: a drain joins every admitted run and aborts none (#1008).
    ///
    /// Across seeded counts and yield depths, every spawned run finishes before
    /// the join returns, and the join reports zero aborted. Catches a drain that
    /// returns while a run whose client left is still writing.
    #[test]
    fn sim_drain_joins_every_seeded_run() -> Result<(), String> {
        for seed in 0..16_u64 {
            let mut draws = Seeded::from_seed(seed);
            let runs = Runs::new();
            let finished = Arc::new(AtomicUsize::new(0));
            let count = draws
                .below(64)
                .map_err(|refusal| format!("a count draw is refused: {refusal}"))?;
            let mut want = 0_usize;
            for _ in 0..count {
                let depth = draws
                    .below(8)
                    .map_err(|refusal| format!("a depth draw is refused: {refusal}"))?;
                let done = Arc::clone(&finished);
                crate::rt::runtime::block_on(async {
                    runs.spawn(async move {
                        for _ in 0..depth {
                            crate::rt::task::yield_now().await;
                        }
                        done.fetch_add(1, Ordering::SeqCst);
                    });
                });
                want = want.saturating_add(1);
            }
            let aborted =
                crate::rt::runtime::block_on(join_or_abort(runs.take(), Duration::from_secs(30)));
            if aborted != 0 || finished.load(Ordering::SeqCst) != want {
                return Err(format!(
                    "seed {seed}: {aborted} aborted, {} of {want} finished",
                    finished.load(Ordering::SeqCst)
                ));
            }
        }
        Ok(())
    }

    /// Requirement: a run that never ends is aborted past the grace, and counted
    /// (#1008).
    ///
    /// A drain that waits for ever on a stuck run would hang the process stop;
    /// this one returns once the bound passes and says how many it aborted.
    #[test]
    fn a_stuck_run_is_aborted_past_the_grace() {
        let runs = Runs::new();
        crate::rt::runtime::block_on(async {
            runs.spawn(std::future::pending());
        });
        let aborted =
            crate::rt::runtime::block_on(join_or_abort(runs.take(), Duration::from_millis(50)));
        assert_eq!(aborted, 1, "the stuck run was not reported aborted");
    }

    /// A drain whose server task `stuck` never ends, one run that ends only when
    /// the serve's token is cancelled, and `live` requests counted in flight:
    /// the pair `drain_owned` reports. Only a drain that passed its deadline
    /// cancels the token, so the run ends exactly then.
    fn drain(stuck: bool, grace: Duration, live: usize) -> (usize, usize) {
        let shared = Arc::new(Shared {
            routes: Vec::new(),
            room: Arc::new(lgwks_deps::tokio::sync::Semaphore::new(1)),
            token: CancellationToken::new(),
            hosts: Hosts::new(2),
            runs: Runs::new(),
            store_dir: std::path::PathBuf::new(),
            ephemeral_store: None,
            max_in_flight: 1,
            drained: AtomicUsize::new(0),
            live: AtomicUsize::new(live),
        });
        crate::rt::runtime::block_on(async {
            let mut owned = lgwks_deps::tokio::task::JoinSet::new();
            if stuck {
                owned.spawn(std::future::pending::<()>());
            }
            let token = shared.token.clone();
            shared.runs.spawn(async move {
                token.cancelled().await;
            });
            drain_owned(&mut owned, Arc::clone(&shared), grace).await
        })
    }

    /// Requirement: a drain reports what it cancelled, and a run its token
    /// reaches is joined, not aborted (#1008).
    ///
    /// Across seeded graces of 20 to 40 ms and live counts, a server that ended
    /// in time leaves the one run un-cancelled, so the drain aborts it and
    /// reports 1. A server stuck past the deadline cancels the token, the run
    /// ends and is joined inside the cancelled grace, and the report is the live
    /// count. Catches a drain that aborts a run its cancellation already ended,
    /// or reports fewer than were live.
    #[test]
    fn sim_drain_reports_what_it_cancelled() -> Result<(), String> {
        for seed in 0..8_u64 {
            let mut draws = Seeded::from_seed(seed);
            let grace = Duration::from_millis(
                draws
                    .below(20)
                    .map_err(|refusal| format!("a grace draw is refused: {refusal}"))?
                    .saturating_add(20),
            );
            let live = usize::try_from(
                draws
                    .below(6)
                    .map_err(|refusal| format!("a live draw is refused: {refusal}"))?,
            )
            .map_err(|refusal| format!("a live draw does not fit: {refusal}"))?;
            let ended_in_time = drain(false, grace, live);
            let past_deadline = drain(true, grace, live);
            if ended_in_time != (0, 1) || past_deadline != (0, live) {
                return Err(format!(
                    "seed {seed}: in time {ended_in_time:?} want (0, 1), past the deadline \
                     {past_deadline:?} want (0, {live})"
                ));
            }
        }
        Ok(())
    }

    /// The cancelled grace is long enough for a run to notice its cancellation.
    const _: () = assert!(CANCELLED_JOIN_GRACE.as_millis() >= 100);
}
