//! One answer per submission: the slot the worker completes and the future the
//! edge awaits.
//!
//! The slot is plain `std` synchronization — a mutex holding at most one
//! response and a waker for the async waiter, and a condvar for the blocking
//! one — so neither the host nor the edge names a driver. The worker sets the
//! response and wakes outside the lock; waiters never hold the lock across an
//! await. A slot is completed exactly once; a second completion is dropped and
//! logged, and the first stands.

use core::fmt;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
#[cfg(test)]
use std::sync::Condvar;
use std::sync::Mutex;

use super::CellError;
use crate::ecs::Response;

/// What a waiter left for the completing worker: how to reach it.
#[derive(Debug)]
struct ReplyInner {
    /// The answer, once the worker set it.
    settled: Option<Response>,
    /// Whether the slot was ever completed, so a taken answer is still settled.
    done: bool,
    /// The async waiter to wake, if it polled pending.
    waker: Option<Waker>,
}

/// One answer, awaited by at most one submitter and completed by one worker.
///
/// Liveness rests on two host disciplines, both enforced where the host is
/// built: every submitted job is answered exactly once (the worker drains its
/// mailbox before it exits, and a job dropped unanswered answers `503` from
/// its own drop), and no holder runs user code while holding the slot lock
/// (so the lock cannot be poisoned by a tenant panic; a poison is still
/// typed, never recovered).
#[derive(Debug)]
pub(crate) struct ReplySlot {
    /// The answer and the waiter's waker, behind one lock.
    inner: Mutex<ReplyInner>,
    /// Rung on completion for the blocking waiter. Simulation-gated: the
    /// shipped edge awaits through [`PendingReply`].
    #[cfg(test)]
    bell: Condvar,
}

impl ReplySlot {
    /// An empty slot, awaiting its answer.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(ReplyInner {
                settled: None,
                done: false,
                waker: None,
            }),
            #[cfg(test)]
            bell: Condvar::new(),
        }
    }

    /// Lock the slot, mapping a poison to its typed refusal.
    ///
    /// # Errors
    ///
    /// [`CellError::Poisoned`] when a previous holder panicked while holding
    /// the slot. No holder runs user code under the lock, so this names a
    /// bug in the host rather than a tenant.
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ReplyInner>, CellError> {
        match self.inner.lock() {
            Ok(guard) => Ok(guard),
            Err(_) => {
                lgwks_std::trace::debug!("cell: the reply slot lock is poisoned");
                Err(CellError::Poisoned { what: "reply slot" })
            }
        }
    }

    /// Whether the slot was ever completed.
    #[must_use]
    pub(crate) fn is_settled(&self) -> bool {
        match self.lock() {
            Ok(guard) => guard.done,
            Err(_) => false,
        }
    }

    /// Set the answer and wake the waiter, unless one is already set.
    pub(crate) fn complete(&self, response: Response) {
        let waker = match self.lock() {
            Ok(mut guard) => {
                if guard.done {
                    lgwks_std::trace::debug!("cell: a second completion for one reply is dropped");
                    return;
                }
                guard.settled = Some(response);
                guard.done = true;
                guard.waker.take()
            }
            Err(_) => return,
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        #[cfg(test)]
        self.bell.notify_all();
    }

    /// Take the answer if it arrived, leaving the slot empty.
    fn take(&self) -> Result<Option<Response>, CellError> {
        Ok(self.lock()?.settled.take())
    }

    /// Block until the answer arrives. Simulation-gated: the only waiter is
    /// the blocking answer.
    ///
    /// Waits on the completion itself; every submitted job is answered
    /// exactly once, so the wait ends on the answer.
    ///
    /// # Errors
    ///
    /// [`CellError::Poisoned`] when the slot lock is poisoned.
    #[cfg(test)]
    pub(crate) fn wait_blocking(&self) -> Result<Response, CellError> {
        let mut guard = self.lock()?;
        loop {
            if let Some(response) = guard.settled.take() {
                return Ok(response);
            }
            guard = match self.bell.wait(guard) {
                Ok(woke) => woke,
                Err(_) => {
                    lgwks_std::trace::warn!("cell: the reply slot broke under a blocking waiter");
                    return Err(CellError::Poisoned { what: "reply slot" });
                }
            };
        }
    }
}

impl fmt::Display for ReplySlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.is_settled() {
            true => f.write_str("a settled reply slot"),
            false => f.write_str("a reply slot awaiting its answer"),
        }
    }
}

/// The future the edge awaits for one submission's answer.
///
/// Dropping it cancels the wait; the worker still answers into the slot,
/// where a taken answer is simply never read.
#[derive(Debug)]
pub(crate) struct PendingReply {
    /// The slot the worker completes.
    slot: std::sync::Arc<ReplySlot>,
}

impl PendingReply {
    /// Await the answer held by `slot`.
    #[must_use]
    pub(crate) fn new(slot: std::sync::Arc<ReplySlot>) -> Self {
        Self { slot }
    }
}

impl Future for PendingReply {
    type Output = Result<Response, CellError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match self.slot.take() {
            Ok(Some(response)) => Poll::Ready(Ok(response)),
            Ok(None) => match self.slot.lock() {
                Ok(mut guard) => {
                    guard.waker = Some(context.waker().clone());
                    // A completion between the take and this store wakes the
                    // stored waker only if the worker saw it; re-check under
                    // the same lock so a completion cannot slip between.
                    match guard.settled.take() {
                        Some(response) => {
                            guard.waker = None;
                            Poll::Ready(Ok(response))
                        }
                        None => Poll::Pending,
                    }
                }
                Err(error) => Poll::Ready(Err(error)),
            },
            Err(error) => Poll::Ready(Err(error)),
        }
    }
}
