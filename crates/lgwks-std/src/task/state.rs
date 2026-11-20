//! What the blocking pool's lock guards, and every transition on it.

use std::collections::VecDeque;

use super::{ThreadHandle, Work, join_pool_thread};

/// What the pool's lock guards.
///
/// Generic over the job so the deterministic simulation (`sim_pool`) drives
/// these same transitions with numbered jobs instead of closures. Every
/// transition is a method below; `Pool::submit` and `Pool::run` only add
/// the lock, the condvar, and running the job.
pub(super) struct PoolState<W = Work> {
    /// Jobs waiting for a thread, oldest first.
    pub(super) queue: VecDeque<W>,
    /// Threads alive, running or idle.
    pub(super) live: usize,
    /// Threads waiting for work that no submitter has claimed yet.
    pub(super) idle: usize,
    /// Wakeups claimed by submitters and not yet taken by a thread. Counted
    /// rather than inferred from the condvar, which may wake spuriously.
    pub(super) wakeups: usize,
    /// Set by `Pool::shutdown`: admission is closed, and a thread with no
    /// job leaves instead of parking.
    pub(super) draining: bool,
    /// The join handle of every thread this pool started and has not joined.
    ///
    /// Registered under the same lock that counted the start, so a shutdown
    /// can never observe a thread it cannot join. What the list holds is
    /// exactly:
    ///
    /// ```text
    /// handles.len() == live + (threads that have left the accounting and not
    ///                          yet returned)
    /// ```
    ///
    /// A thread decrements `live` under the lock and only then returns, so
    /// every entry beyond `live` is a real OS thread still executing its last
    /// instructions. **There is no constant bound on that second group.** Each
    /// departure frees a slot immediately while the thread keeps running, so
    /// a scheduler that deschedules every departing thread in turn admits the
    /// next one; at a ceiling of one the list can hold several handles with a
    /// single thread live. What is true is the identity above and what the
    /// reap does with it: every thread that *has* returned is joined at the
    /// next start or at a shutdown, so the list never holds a
    /// finished-but-unjoined thread past one of those, and it does not grow
    /// across idle cycles.
    pub(super) handles: Vec<ThreadHandle>,
}

/// What admitting a job asks of the submitter.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Admitted<W> {
    /// An idle thread was claimed: notify the condvar.
    Woke,
    /// No thread is idle and the pool is under its ceiling: start one with
    /// this job as its first, then report [`PoolState::started`] or
    /// [`PoolState::start_failed`]. The job never passes through the queue,
    /// so the new thread does not contend for the lock to reach it.
    Start(W),
    /// Every thread is busy at the ceiling: the job waits its turn.
    Queued,
}

/// Why admitting a job was refused. The job is handed back: it did not run.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Refused<W> {
    /// The wait queue is at its declared bound.
    AtCapacity(W),
    /// The pool no longer admits work.
    Draining(W),
}

/// What a parked thread does once it holds the lock again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Woken {
    /// It took a claimed wakeup: look for work.
    Resume,
    /// It waited out the keep-alive unclaimed: it has left the pool.
    Exit,
    /// A spurious wake: wait again.
    Wait,
}

impl<W> PoolState<W> {
    /// An empty pool.
    pub(super) const fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            live: 0,
            idle: 0,
            wakeups: 0,
            draining: false,
            handles: Vec::new(),
        }
    }

    /// Admit `work`, or hand it back when the pool takes no more.
    pub(super) fn admit(
        &mut self,
        work: W,
        queue_limit: Option<usize>,
        ceiling: usize,
    ) -> Result<Admitted<W>, Refused<W>> {
        if self.draining {
            #[cfg(feature = "trace")]
            crate::trace::debug!(
                live = self.live,
                queued = self.queue.len(),
                "admit: the pool is shutting down and admits nothing"
            );
            return Err(Refused::Draining(work));
        }
        if queue_limit.is_some_and(|limit| self.queue.len() >= limit) {
            #[cfg(feature = "trace")]
            crate::trace::debug!(queued = self.queue.len(), limit = ?queue_limit, "admit: the wait queue is at its bound");
            return Err(Refused::AtCapacity(work));
        }
        if self.idle == 0 && self.live < ceiling {
            return Ok(Admitted::Start(work));
        }
        self.queue.push_back(work);
        if self.idle > 0 {
            self.idle = self.idle.saturating_sub(1);
            self.wakeups = self.wakeups.saturating_add(1);
            return Ok(Admitted::Woke);
        }
        Ok(Admitted::Queued)
    }

    /// The thread [`Admitted::Start`] asked for is running.
    pub(super) const fn started(&mut self) {
        self.live = self.live.saturating_add(1);
    }

    /// The thread [`Admitted::Start`] asked for could not start, and `work`
    /// was its first job. With a live thread the job is queued for it
    /// (`None`): no thread is idle, since admission chose to start one, and
    /// the lock has been held since. With none, nothing would ever run it, so
    /// it is handed back.
    pub(super) fn start_failed(&mut self, work: W) -> Option<W> {
        if self.live > 0 {
            self.queue.push_back(work);
            return None;
        }
        Some(work)
    }

    /// The next job for a thread that is free.
    pub(super) fn take(&mut self) -> Option<W> {
        self.queue.pop_front()
    }

    /// A free thread found no job: park it, or — while the pool drains —
    /// retire it instead. Returns `true` when the thread leaves the pool.
    pub(super) const fn park_or_retire(&mut self) -> bool {
        if self.draining {
            self.live = self.live.saturating_sub(1);
            return true;
        }
        self.idle = self.idle.saturating_add(1);
        false
    }

    /// A parked thread holds the lock again, its wait having `timed_out` or
    /// not.
    pub(super) const fn woken(&mut self, timed_out: bool) -> Woken {
        if self.wakeups > 0 {
            // The submitter already moved this thread out of `idle`.
            self.wakeups = self.wakeups.saturating_sub(1);
            return Woken::Resume;
        }
        if timed_out {
            self.idle = self.idle.saturating_sub(1);
            self.live = self.live.saturating_sub(1);
            return Woken::Exit;
        }
        Woken::Wait
    }

    /// A parked thread woken without a claimed wakeup: while the pool drains
    /// it takes a waiting job if there is one, and otherwise leaves. Its
    /// `idle` was never claimed by a submitter, so it retires that itself.
    pub(super) fn drain_wake(&mut self) -> Woken {
        if !self.draining {
            return Woken::Wait;
        }
        self.idle = self.idle.saturating_sub(1);
        if self.queue.is_empty() {
            self.live = self.live.saturating_sub(1);
            return Woken::Exit;
        }
        Woken::Resume
    }

    /// Close admission: every later submit is refused, and a thread with no
    /// job leaves instead of parking.
    pub(super) const fn begin_drain(&mut self) {
        self.draining = true;
    }

    /// Join every thread that has already returned.
    ///
    /// A thread that exits on its keep-alive is still held by its handle until
    /// something joins it, so a start is where the exits of earlier bursts are
    /// released — the property the list's doc rests on. `is_finished` never
    /// blocks and a returned thread's `join` returns at once, so this is a reap
    /// and not a wait: a thread still in its last instructions keeps its handle
    /// and is reaped by a later start or by a shutdown.
    pub(super) fn reap_finished_threads(&mut self) {
        // The count exists only where its one reader, the trace event, is built.
        #[cfg(feature = "trace")]
        let before = self.handles.len();
        let mut kept = Vec::with_capacity(self.handles.len());
        for handle in std::mem::take(&mut self.handles) {
            if handle.is_finished() {
                join_pool_thread(handle);
            } else {
                kept.push(handle);
            }
        }
        self.handles = kept;
        #[cfg(feature = "trace")]
        {
            let reaped = before.saturating_sub(self.handles.len());
            if reaped > 0 {
                crate::trace::debug!(
                    reaped,
                    live = self.live,
                    "submit: joined the pool threads that had already left"
                );
            }
        }
    }
}
