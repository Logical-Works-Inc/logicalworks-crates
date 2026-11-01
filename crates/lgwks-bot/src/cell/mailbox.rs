//! One worker's bounded mailbox: submitters push, the worker pops.
//!
//! Blocking submitters wait for room on the room bell, the worker waits for
//! work on the work bell, and closing rings both, so no waiter parks past the
//! shutdown. The bound is declared at construction and never grows. No holder
//! runs user code under the lock, so a poisoned lock names a bug in this file
//! and is reported as one, never recovered.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};

use super::CellError;

/// A bounded queue between submitters and one worker.
#[derive(Debug)]
pub(super) struct Mailbox<J> {
    /// The queued jobs and the closed flag, behind one lock.
    inner: Mutex<Inner<J>>,
    /// Rung when a job is popped: room for a blocked submitter.
    room: Condvar,
    /// Rung when a job is pushed or the mailbox closes: work for the worker.
    work: Condvar,
    /// Jobs the mailbox holds before it refuses.
    bound: usize,
}

/// What a mailbox holds.
#[derive(Debug)]
struct Inner<J> {
    /// Queued jobs, in arrival order.
    queue: VecDeque<J>,
    /// Whether the host is shut down: no submitter is admitted, the worker
    /// drains what is queued and then exits.
    closed: bool,
    /// Whether the worker is parked on the work bell, so a push rings it only
    /// when someone is listening.
    parked: bool,
    /// Jobs of the batch the worker took last and may still be serving; they
    /// count against the bound with the queue until the next take.
    serving: usize,
}

/// Why a push did not queue. The job travels back so the caller decides what
/// its submitter hears.
#[derive(Debug)]
pub(super) enum PushRefused<J> {
    /// The mailbox is closed.
    Closed(J),
    /// The mailbox is full.
    Full(J),
    /// The mailbox lock is poisoned.
    Poisoned(J),
}

impl<J> Mailbox<J> {
    /// A mailbox holding at most `bound` jobs, and at least one.
    pub(super) fn new(bound: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                queue: VecDeque::new(),
                closed: false,
                parked: false,
                serving: 0,
            }),
            room: Condvar::new(),
            work: Condvar::new(),
            bound: bound.max(1),
        }
    }

    /// Lock the mailbox, or `None` when a holder panicked under it.
    fn lock(&self) -> Option<MutexGuard<'_, Inner<J>>> {
        match self.inner.lock() {
            Ok(guard) => Some(guard),
            Err(_) => {
                lgwks_std::trace::warn!("cell: the mailbox lock is poisoned");
                None
            }
        }
    }

    /// Queue `job` without waiting, refusing a full or closed mailbox.
    ///
    /// # Errors
    ///
    /// [`PushRefused`] carrying the job back.
    pub(super) fn try_push(&self, job: J) -> Result<(), PushRefused<J>> {
        let Some(mut inner) = self.lock() else {
            lgwks_std::trace::debug!(bound = self.bound, "cell: a push met a poisoned mailbox");
            return Err(PushRefused::Poisoned(job));
        };
        if inner.closed {
            lgwks_std::trace::debug!(bound = self.bound, "cell: a push met a closed mailbox");
            return Err(PushRefused::Closed(job));
        }
        let held = inner.queue.len().saturating_add(inner.serving);
        if held >= self.bound {
            lgwks_std::trace::debug!(held, bound = self.bound, "cell: a push met a full mailbox");
            return Err(PushRefused::Full(job));
        }
        inner.queue.push_back(job);
        let parked = inner.parked;
        drop(inner);
        if parked {
            self.work.notify_one();
        }
        Ok(())
    }

    /// Queue `job`, waiting for room, refusing only a closed mailbox.
    /// Simulation-gated: the only caller is the waiting submit.
    ///
    /// # Errors
    ///
    /// [`CellError::Closed`] when the host shut down while the submitter
    /// waited, [`CellError::Poisoned`] on a poisoned lock.
    #[cfg(test)]
    pub(super) fn push_blocking(&self, job: J) -> Result<(), CellError> {
        let Some(mut inner) = self.lock() else {
            lgwks_std::trace::debug!(
                bound = self.bound,
                "cell: a waiting push met a poisoned mailbox"
            );
            return Err(CellError::Poisoned { what: "mailbox" });
        };
        loop {
            if inner.closed {
                lgwks_std::trace::debug!(
                    bound = self.bound,
                    "cell: a waiting push met a closed mailbox"
                );
                return Err(CellError::Closed);
            }
            if inner.queue.len().saturating_add(inner.serving) < self.bound {
                inner.queue.push_back(job);
                drop(inner);
                self.work.notify_one();
                return Ok(());
            }
            inner = match self.room.wait(inner) {
                Ok(woke) => woke,
                Err(_) => {
                    lgwks_std::trace::warn!(
                        bound = self.bound,
                        "cell: the mailbox broke under a waiting push"
                    );
                    return Err(CellError::Poisoned { what: "mailbox" });
                }
            };
        }
    }

    /// Move every queued job into `batch`, which the worker hands back empty
    /// once it has served the last one, waiting while there are none; `false`
    /// once the closed mailbox has drained. One lock per batch rather than
    /// per job; the bound counts the batch in service with the queue, so at
    /// most `bound` jobs are ever admitted and unanswered.
    ///
    /// # Errors
    ///
    /// [`CellError::Poisoned`] on a poisoned lock.
    pub(super) fn take_batch(&self, batch: &mut VecDeque<J>) -> Result<bool, CellError> {
        let Some(mut inner) = self.lock() else {
            lgwks_std::trace::warn!(bound = self.bound, "cell: a worker met a poisoned mailbox");
            return Err(CellError::Poisoned { what: "mailbox" });
        };
        inner.serving = 0;
        self.room.notify_all();
        loop {
            if !inner.queue.is_empty() {
                core::mem::swap(&mut inner.queue, batch);
                inner.serving = batch.len();
                return Ok(true);
            }
            if inner.closed {
                return Ok(false);
            }
            inner.parked = true;
            inner = match self.work.wait(inner) {
                Ok(woke) => woke,
                Err(_) => {
                    lgwks_std::trace::warn!(
                        bound = self.bound,
                        "cell: the mailbox broke under a parked worker"
                    );
                    return Err(CellError::Poisoned { what: "mailbox" });
                }
            };
            inner.parked = false;
        }
    }

    /// Refuse every later submitter and wake every waiter; queued jobs still
    /// drain.
    pub(super) fn close(&self) {
        if let Some(mut inner) = self.lock() {
            inner.closed = true;
        }
        self.room.notify_all();
        self.work.notify_all();
    }
}
