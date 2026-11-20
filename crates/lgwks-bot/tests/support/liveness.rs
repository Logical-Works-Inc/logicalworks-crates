//! The parked-device instrument every liveness family needs, written once.
//!
//! `journal_liveness.rs`, `resume_liveness.rs` and `sim_store_scale.rs` prove the
//! same thing about two different stores: while a durable write is parked on its
//! device, an unrelated task on the same driver keeps turning. The instrument is
//! an independent watchdog thread that releases the device only once that task
//! has demonstrably progressed, and a second copy of it would be a second thing
//! to drift. It needs only `rt`, so the journal target builds without `script`.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::Poll;
use std::time::Duration;

use lgwks_bot::journal::StorageGate;

/// How many turns the unrelated task must get before the watchdog releases the
/// parked device. A blocking durable wait reaches at most one.
pub(crate) const PROGRESS_TURNS: u64 = 64;

/// How long the watchdog waits for that progress before it releases anyway, so a
/// genuinely wedged tree fails the tick assertion rather than hanging the suite.
const PROGRESS_BUDGET: Duration = Duration::from_secs(10);

/// The turns the unrelated task has taken, and the bell it rings on reaching
/// [`PROGRESS_TURNS`], so the watchdog waits on the event itself.
#[derive(Default)]
pub(crate) struct Ticks {
    /// Turns taken so far.
    count: AtomicU64,
    /// Whether the count has reached [`PROGRESS_TURNS`].
    progressed: Mutex<bool>,
    /// Rung once, when it has.
    bell: Condvar,
}

impl Ticks {
    /// Turns taken so far.
    pub(crate) fn load(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// One more turn; the turn that reaches [`PROGRESS_TURNS`] rings the bell.
    fn tick(&self) {
        let now = self.count.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        if now != PROGRESS_TURNS {
            return;
        }
        match self.progressed.lock() {
            Ok(mut progressed) => {
                *progressed = true;
                self.bell.notify_all();
            }
            Err(poisoned) => lgwks_std::trace::debug!(
                error = %poisoned,
                "liveness: the progress flag is poisoned, so the watchdog waits out its budget"
            ),
        }
    }

    /// Block until the bell rings or `budget` passes, whichever is first.
    fn await_progress(&self, budget: Duration) {
        let progressed = match self.progressed.lock() {
            Ok(progressed) => progressed,
            Err(poisoned) => {
                lgwks_std::trace::debug!(error = %poisoned, "liveness: the progress flag is poisoned");
                return;
            }
        };
        match self
            .bell
            .wait_timeout_while(progressed, budget, |progressed| !*progressed)
        {
            Ok((progressed, waited)) => lgwks_std::trace::debug!(
                progressed = *progressed,
                timed_out = waited.timed_out(),
                "liveness: the watchdog's wait ended"
            ),
            Err(poisoned) => lgwks_std::trace::debug!(
                error = %poisoned,
                "liveness: the progress flag was poisoned during the wait"
            ),
        }
    }
}

/// A parked device with the instruments a liveness test needs.
pub(crate) struct Parked {
    /// Where the parked append must be attempted, when the caller is parking a
    /// position-fenced write rather than a record append.
    ///
    /// A journal's liveness test needs the fence it is about to contend on; a
    /// record store's does not, because its append takes no caller-supplied
    /// position. One type serves both rather than two that differ by a field,
    /// because the part that is under test is identical: an independent thread
    /// releasing a parked device while an unrelated task keeps turning.
    tail: Option<lgwks_bot::journal::JournalPosition>,
    /// How many turns the unrelated task has taken.
    ticks: Arc<Ticks>,
    /// Set by the watchdog once it has released the device.
    released: Arc<AtomicBool>,
    /// The independent thread that releases the device.
    watchdog: std::thread::JoinHandle<u64>,
}

impl Parked {
    /// Park `gate`'s device and start the independent watchdog over it, with
    /// the fence a position-fenced write must continue from when there is one.
    ///
    /// One constructor rather than a copy per test, because the watchdog contract —
    /// an independent thread that releases the device once the unrelated task has
    /// progressed — is the thing under test, and a second copy is a second thing to
    /// drift. A record store's append takes no position, so it passes `None`; a
    /// journal's passes the tail it is about to contend on.
    ///
    /// # Errors
    ///
    /// Whatever the OS reports when the watchdog thread cannot start.
    pub(crate) fn at(
        tail: Option<lgwks_bot::journal::JournalPosition>,
        gate: StorageGate,
    ) -> Result<Self, std::io::Error> {
        let ticks = Arc::new(Ticks::default());
        let released = Arc::new(AtomicBool::new(false));
        let watchdog = spawn_watchdog(&ticks, gate, Arc::clone(&released))?;
        Ok(Self {
            tail,
            ticks,
            released,
            watchdog,
        })
    }

    /// The fence a parked append must continue from.
    ///
    /// # Errors
    ///
    /// When this `Parked` was opened with no fence: a caller asking for a fence
    /// it never supplied cannot be
    /// served a value, and a store whose append takes no position has none to
    /// hand back. `Result` rather than a panic because the two kinds of caller
    /// differ by construction and the test must not be the thing that decides
    /// which one it is.
    pub(crate) fn tail(&self) -> Result<lgwks_bot::journal::JournalPosition, String> {
        self.tail.ok_or_else(|| {
            "this Parked was opened without a fence; pass one to Parked::at".to_owned()
        })
    }

    /// The counter the unrelated task turns.
    #[must_use]
    pub(crate) fn ticks(&self) -> Arc<Ticks> {
        Arc::clone(&self.ticks)
    }

    /// Whether the watchdog has released the device.
    #[must_use]
    pub(crate) fn released(&self) -> &AtomicBool {
        &self.released
    }

    /// Wait for the watchdog and report the turns the runtime achieved.
    ///
    /// # Errors
    ///
    /// When the watchdog thread panicked, which would leave the release unheard.
    pub(crate) fn observed(self) -> Result<u64, String> {
        self.watchdog
            .join()
            .map_err(|panic| format!("the watchdog thread panicked: {panic:?}"))
    }
}

/// The unrelated ready task, polled on the same driver as the record: it ticks and
/// re-wakes itself so the driver keeps turning while the flush is parked.
pub(crate) fn heartbeat(ticks: Arc<Ticks>) -> impl Future<Output = ()> {
    std::future::poll_fn(move |cx| {
        ticks.tick();
        cx.waker().wake_by_ref();
        Poll::<()>::Pending
    })
}

/// An independent thread that releases the parked device once the unrelated task
/// has demonstrably progressed.
///
/// It is not the runtime and not the storage owner: nothing it does depends on the
/// future under test being polled. It gives up at `deadline` so a tree that is
/// genuinely wedged fails the tick assertion rather than hanging the suite, sets
/// `released` so a later phase can observe the release without asking the store,
/// and reports the tick count it saw. It waits on the heartbeat's bell, so it
/// wakes on the very turn that proves progress rather than on a timer.
fn spawn_watchdog(
    ticks: &Arc<Ticks>,
    gate: StorageGate,
    released: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<u64>, std::io::Error> {
    let ticks = Arc::clone(ticks);
    std::thread::Builder::new()
        .name("liveness-watchdog".to_owned())
        .spawn(move || {
            ticks.await_progress(PROGRESS_BUDGET);
            gate.release();
            released.store(true, Ordering::Relaxed);
            ticks.load()
        })
}
