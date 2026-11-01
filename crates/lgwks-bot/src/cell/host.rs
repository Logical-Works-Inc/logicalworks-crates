//! The per-tenant execution host: one thread per core, one mailbox per worker.
//!
//! A [`CellHost`] owns `workers` threads started with [`Builder`] and joined
//! on shutdown, never detached. Each worker owns its tenants outright (their
//! states, checkpoints and tails) and its mailbox's receiving end, so no lock
//! sits between a message and its tenant. A tenant is routed by FNV-1a,
//! masked to the worker count, which is a power of two.
//!
//! Liveness rests on two rules. Shutdown closes every mailbox, the workers
//! answer every queued job and exit, and only then are the threads joined.
//! And a [`Job`] that is dropped unanswered on any path, a panic in the host
//! itself included, answers its submitter `503` from its own `Drop`, so no
//! edge future waits on a reply that will never come.

use std::collections::{BTreeMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{Builder, JoinHandle};

use super::mailbox::{Mailbox, PushRefused};
use super::reply::ReplySlot;
use super::{
    CHECKPOINT_BYTES, CHECKPOINT_EVERY, Cell, CellError, CellRefused, EffectRunner,
    MAX_MESSAGE_BYTES, MAX_STEPS_PER_MESSAGE, MAX_WORKERS, RETRY_AFTER_MS, Steps, SubmitRefused,
};
use crate::ecs::{MAX_TENANTS, Response};
use crate::journal::frame::SaturatingFrom;

/// FNV-1a offset basis over bytes.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a prime.
const FNV_PRIME: u64 = 0x0100_0000_01b3;

/// The counters one host reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct HostCounts {
    /// Messages the cell answered, including contained panics.
    pub(crate) settled: u64,
    /// Submissions refused before the cell: closed, full, or past the tenant
    /// ceiling.
    pub(crate) refused: u64,
    /// Tenants the host holds state for.
    pub(crate) tenants: usize,
    /// Panics the host contained and rebuilt from.
    pub(crate) panicked: u64,
}

/// One submission: a message and where its answer goes.
///
/// The message is `Some` until the worker moves it into the tenant's log. A
/// job dropped before its slot is settled answers `503` on the way out.
struct Job<M> {
    /// The message, taken into the tenant's tail by the worker.
    message: Option<M>,
    /// Where the answer goes.
    reply: Arc<ReplySlot>,
}

impl<M> Drop for Job<M> {
    fn drop(&mut self) {
        if !self.reply.is_settled() {
            lgwks_std::trace::debug!("cell: a job was dropped unanswered");
            self.reply.complete(Response::new(
                503,
                b"the cell host dropped the request".to_vec(),
            ));
        }
    }
}

/// The erased host behind [`RequestFeed`](crate::ecs::RequestFeed): the feed
/// stays non-generic over its handler while the host stays generic over its
/// cell.
pub(crate) trait DynHost<M>: Send + Sync {
    /// Queue `message` for `tenant` without waiting.
    ///
    /// # Errors
    ///
    /// [`SubmitRefused`] when the message, the mailbox or the host refuses.
    fn try_submit(
        &self,
        tenant: &str,
        message: M,
        reply: Arc<ReplySlot>,
    ) -> Result<(), SubmitRefused>;

    /// Queue `message` for `tenant`, waiting for room. Simulation-gated: the
    /// only caller is the blocking answer; the shipped path never waits.
    ///
    /// # Errors
    ///
    /// [`CellError::Closed`] when the host shut down, [`CellError::Poisoned`]
    /// on a poisoned mailbox lock.
    #[cfg(test)]
    fn submit_wait(&self, tenant: &str, message: M, reply: Arc<ReplySlot>)
    -> Result<(), CellError>;

    /// The counts the host has recorded.
    fn counts(&self) -> HostCounts;

    /// Refuse later submissions, answer every queued job and join every
    /// worker. Idempotent.
    fn close(&self);
}

/// One tenant's cell: its state and the command log it is the fold of.
struct TenantCell<S, M> {
    /// The live state: `checkpoint` folded with `tail`.
    state: S,
    /// The state as of the last checkpoint.
    checkpoint: S,
    /// Messages applied since the checkpoint, in order.
    tail: Vec<M>,
    /// The tail's total weight.
    tail_bytes: usize,
}

/// What every worker of one host shares: the cell, the runner and the
/// counters. No mailbox or join handle is here, so a worker never keeps its
/// host alive.
struct HostCore<C> {
    /// The cell every worker applies.
    cell: C,
    /// The runner every worker delivers through.
    runner: Arc<dyn EffectRunner>,
    /// Whether the host is closed.
    closed: AtomicBool,
    /// Messages the cell answered, including contained panics.
    settled: AtomicU64,
    /// Submissions refused before the cell.
    refused: AtomicU64,
    /// Contained panics rebuilt from the log.
    panicked: AtomicU64,
    /// Tenants held across every worker, charged before a tenant is made.
    tenants: AtomicU64,
}

/// The per-tenant execution host for one [`Cell`].
///
/// The host owns the mailboxes and the join handles; the workers own their
/// tenants. Dropping the host shuts the workers down and joins them, so no
/// thread outlives its host.
pub(crate) struct CellHost<C: Cell> {
    /// The cell, runner and counters the workers serve under.
    core: Arc<HostCore<C>>,
    /// One bounded mailbox per worker.
    boxes: Vec<Arc<Mailbox<Job<C::Message>>>>,
    /// Workers minus one: the route mask.
    mask: u64,
    /// Worker threads, taken and joined on shutdown.
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl<C: Cell> CellHost<C> {
    /// Start `workers` threads serving `cell`, each behind a mailbox of
    /// `mailbox_bound` jobs. `workers` is capped at [`MAX_WORKERS`] and rounded
    /// up to a power of two.
    ///
    /// # Errors
    ///
    /// [`CellError::Spawn`] when a worker thread cannot be started; the ones
    /// already started are shut down and joined first.
    pub(crate) fn start(
        cell: C,
        runner: Arc<dyn EffectRunner>,
        workers: usize,
        mailbox_bound: usize,
    ) -> Result<Self, CellError> {
        let count = workers.clamp(1, MAX_WORKERS).next_power_of_two();
        let host = Self::with_boxes(cell, runner, count, mailbox_bound, false);
        for (worker, mailbox) in host.boxes.iter().enumerate() {
            let driver = Worker {
                core: Arc::clone(&host.core),
                mailbox: Arc::clone(mailbox),
                table: BTreeMap::new(),
            };
            let spawned = Builder::new()
                .name(format!("lgwks-cell-{worker:02}"))
                .spawn(move || driver.serve());
            match spawned {
                Ok(handle) => host.held_workers()?.push(handle),
                Err(error) => {
                    lgwks_std::trace::warn!(%error, worker, "cell: a worker thread could not start");
                    host.shutdown();
                    return Err(CellError::Spawn(error));
                }
            }
        }
        Ok(host)
    }

    /// A host that refuses every submission: the degradation when no worker
    /// could start, warned at the call site.
    #[must_use]
    pub(crate) fn closed(cell: C, runner: Arc<dyn EffectRunner>) -> Self {
        Self::with_boxes(cell, runner, 0, 1, true)
    }

    /// A host with `count` empty mailboxes and no worker yet.
    fn with_boxes(
        cell: C,
        runner: Arc<dyn EffectRunner>,
        count: usize,
        mailbox_bound: usize,
        closed: bool,
    ) -> Self {
        Self {
            core: Arc::new(HostCore {
                cell,
                runner,
                closed: AtomicBool::new(closed),
                settled: AtomicU64::new(0),
                refused: AtomicU64::new(0),
                panicked: AtomicU64::new(0),
                tenants: AtomicU64::new(0),
            }),
            boxes: (0..count)
                .map(|_| Arc::new(Mailbox::new(mailbox_bound)))
                .collect(),
            mask: u64::saturating_from(count.saturating_sub(1)),
            workers: Mutex::new(Vec::with_capacity(count)),
        }
    }

    /// The join handles, or a typed refusal when their lock is poisoned.
    fn held_workers(&self) -> Result<std::sync::MutexGuard<'_, Vec<JoinHandle<()>>>, CellError> {
        match self.workers.lock() {
            Ok(held) => Ok(held),
            Err(_) => {
                lgwks_std::trace::warn!("cell: the worker table lock is poisoned");
                Err(CellError::Poisoned {
                    what: "worker table",
                })
            }
        }
    }

    /// The mailbox `tenant` routes to: FNV-1a over its bytes, masked.
    fn mailbox_of(&self, tenant: &str) -> Option<&Arc<Mailbox<Job<C::Message>>>> {
        let mut hash = FNV_OFFSET;
        for byte in tenant.bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME);
        }
        self.boxes.get(usize::saturating_from(hash & self.mask))
    }

    /// Refuse the submission, logged and counted.
    fn refuse<T>(&self, tenant: &str, refusal: SubmitRefused) -> Result<T, SubmitRefused> {
        lgwks_std::trace::debug!(tenant, %refusal, "cell: a submission was refused");
        self.core.refused.fetch_add(1, Ordering::Relaxed);
        Err(refusal)
    }

    /// The mailbox of an open host that `tenant` routes to, or the refusal.
    fn route(&self, tenant: &str) -> Result<&Arc<Mailbox<Job<C::Message>>>, SubmitRefused> {
        if self.core.closed.load(Ordering::Acquire) {
            return self.refuse(tenant, SubmitRefused::Closed);
        }
        match self.mailbox_of(tenant) {
            Some(mailbox) => Ok(mailbox),
            None => self.refuse(tenant, SubmitRefused::Closed),
        }
    }

    /// Refuse later submissions, answer every queued job and join every
    /// worker. Idempotent: a second call finds no handle left to join.
    fn shutdown(&self) {
        self.core.closed.store(true, Ordering::Release);
        for mailbox in &self.boxes {
            mailbox.close();
        }
        let handles = match self.held_workers() {
            Ok(mut held) => core::mem::take(&mut *held),
            Err(_) => Vec::new(),
        };
        for handle in handles {
            if handle.join().is_err() {
                lgwks_std::trace::warn!("cell: a worker panicked on the way out");
            }
        }
    }
}

impl<C: Cell> DynHost<C::Message> for CellHost<C> {
    fn try_submit(
        &self,
        tenant: &str,
        message: C::Message,
        reply: Arc<ReplySlot>,
    ) -> Result<(), SubmitRefused> {
        let bytes = C::weight(&message);
        if bytes > MAX_MESSAGE_BYTES {
            return self.refuse(
                tenant,
                SubmitRefused::TooLarge {
                    bytes,
                    ceiling: MAX_MESSAGE_BYTES,
                },
            );
        }
        let mailbox = self.route(tenant)?;
        let job = Job {
            message: Some(message),
            reply,
        };
        // A refused job is answered by the edge from the refusal, so its own
        // drop must not answer first: settle it quietly with the same status.
        match mailbox.try_push(job) {
            Ok(()) => Ok(()),
            Err(PushRefused::Full(job)) => {
                job.reply.complete(Response::new(503, Vec::new()));
                self.refuse(
                    tenant,
                    SubmitRefused::Overloaded {
                        retry_after_ms: RETRY_AFTER_MS,
                    },
                )
            }
            Err(PushRefused::Closed(job) | PushRefused::Poisoned(job)) => {
                job.reply.complete(Response::new(503, Vec::new()));
                self.refuse(tenant, SubmitRefused::Closed)
            }
        }
    }

    #[cfg(test)]
    fn submit_wait(
        &self,
        tenant: &str,
        message: C::Message,
        reply: Arc<ReplySlot>,
    ) -> Result<(), CellError> {
        let mailbox = self.route(tenant).or(Err(CellError::Closed))?;
        let job = Job {
            message: Some(message),
            reply,
        };
        mailbox.push_blocking(job).inspect_err(|error| {
            lgwks_std::trace::debug!(%error, tenant, "cell: the waiting submit was refused");
            self.core.refused.fetch_add(1, Ordering::Relaxed);
        })
    }

    fn counts(&self) -> HostCounts {
        HostCounts {
            settled: self.core.settled.load(Ordering::Relaxed),
            refused: self.core.refused.load(Ordering::Relaxed),
            tenants: usize::saturating_from(self.core.tenants.load(Ordering::Acquire)),
            panicked: self.core.panicked.load(Ordering::Relaxed),
        }
    }

    fn close(&self) {
        self.shutdown();
    }
}

impl<C: Cell> Drop for CellHost<C> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// One worker thread: its mailbox and the tenants it owns.
struct Worker<C: Cell> {
    /// The cell, runner and counters.
    core: Arc<HostCore<C>>,
    /// This worker's mailbox.
    mailbox: Arc<Mailbox<Job<C::Message>>>,
    /// This worker's tenants, owned outright.
    table: BTreeMap<Box<str>, TenantCell<C::State, C::Message>>,
}

impl<C: Cell> Worker<C> {
    /// Serve the mailbox, a batch at a time, until it is closed and drained.
    fn serve(mut self) {
        let mut batch = VecDeque::new();
        loop {
            match self.mailbox.take_batch(&mut batch) {
                Ok(true) => {
                    while let Some(job) = batch.pop_front() {
                        self.serve_one(job);
                    }
                }
                Ok(false) => return,
                Err(error) => {
                    lgwks_std::trace::warn!(%error, "cell: a worker stops on a broken mailbox");
                    return;
                }
            }
        }
    }

    /// Log, apply and answer one job.
    fn serve_one(&mut self, mut job: Job<C::Message>) {
        let Some(message) = job.message.take() else {
            return;
        };
        let answer = match cell_of::<C>(&self.core, &mut self.table, C::tenant(&message)) {
            Some(cell) => apply_logged(&self.core, cell, message),
            None => {
                self.core.refused.fetch_add(1, Ordering::Relaxed);
                Response::new(503, b"tenant ceiling reached".to_vec())
            }
        };
        job.reply.complete(answer);
    }
}

/// The tenant's cell in `table`, made on its first message while the host is
/// under [`MAX_TENANTS`]; `None` past the ceiling.
fn cell_of<'table, C: Cell>(
    core: &HostCore<C>,
    table: &'table mut BTreeMap<Box<str>, TenantCell<C::State, C::Message>>,
    tenant: &str,
) -> Option<&'table mut TenantCell<C::State, C::Message>> {
    if !table.contains_key(tenant) {
        let held = core.tenants.fetch_add(1, Ordering::AcqRel);
        if held >= u64::saturating_from(MAX_TENANTS) {
            core.tenants.fetch_sub(1, Ordering::AcqRel);
            lgwks_std::trace::warn!(tenant, "cell: the tenant ceiling refused a tenant");
            return None;
        }
        table.insert(
            tenant.into(),
            TenantCell {
                state: C::State::default(),
                checkpoint: C::State::default(),
                tail: Vec::new(),
                tail_bytes: 0,
            },
        );
    }
    table.get_mut(tenant)
}

/// Append `message` to the tenant's log, apply it, and settle its answer.
fn apply_logged<C: Cell>(
    core: &HostCore<C>,
    cell: &mut TenantCell<C::State, C::Message>,
    message: C::Message,
) -> Response {
    cell.tail_bytes = cell.tail_bytes.saturating_add(C::weight(&message));
    cell.tail.push(message.clone());
    let applied = catch_unwind(AssertUnwindSafe(|| {
        let mut budget = Steps::new(MAX_STEPS_PER_MESSAGE);
        core.cell.apply(&mut cell.state, message, &mut budget)
    }));
    match applied {
        Ok(Ok(effects)) => {
            core.settled.fetch_add(1, Ordering::Relaxed);
            let answer = match core.runner.run(effects) {
                Some(response) => response,
                None => {
                    lgwks_std::trace::debug!("cell: the cell returned no reply effect");
                    Response::new(500, b"the cell returned no answer".to_vec())
                }
            };
            checkpoint_if_due::<C>(cell);
            answer
        }
        Ok(Err(CellRefused::BudgetSpent)) => {
            drop_last::<C>(cell);
            core.refused.fetch_add(1, Ordering::Relaxed);
            Response::new(503, b"the cell spent its step budget".to_vec())
        }
        Err(_) => {
            drop_last::<C>(cell);
            rebuild(core, cell);
            core.panicked.fetch_add(1, Ordering::Relaxed);
            core.settled.fetch_add(1, Ordering::Relaxed);
            Response::new(
                500,
                b"the tenant cell panicked and was rebuilt; the message was not applied".to_vec(),
            )
        }
    }
}

/// Drop the newest message from the tail: it was refused or it panicked, so
/// the state it would have made is not the tenant's.
fn drop_last<C: Cell>(cell: &mut TenantCell<C::State, C::Message>) {
    if let Some(last) = cell.tail.pop() {
        cell.tail_bytes = cell.tail_bytes.saturating_sub(C::weight(&last));
    }
}

/// Make the live state the new checkpoint once the tail is long or heavy
/// enough, and clear the tail. A clone that panics leaves the old checkpoint
/// and tail in place, which still fold to the live state.
fn checkpoint_if_due<C: Cell>(cell: &mut TenantCell<C::State, C::Message>) {
    if cell.tail.len() < CHECKPOINT_EVERY && cell.tail_bytes < CHECKPOINT_BYTES {
        return;
    }
    match catch_unwind(AssertUnwindSafe(|| cell.state.clone())) {
        Ok(checkpoint) => {
            cell.checkpoint = checkpoint;
            cell.tail.clear();
            cell.tail_bytes = 0;
        }
        Err(_) => lgwks_std::trace::warn!("cell: a state clone panicked; the checkpoint stands"),
    }
}

/// Rebuild the live state as the checkpoint folded with the tail.
///
/// Replayed effects are discarded: the live run already delivered them. A
/// message that panics in replay (a handler that is not a pure function of
/// its state and message) is dropped from the tail and the fold starts again,
/// so the loop ends within one pass per message. A checkpoint whose clone
/// panics leaves the tenant on its default state, the one state that needs
/// no clone, and says so.
fn rebuild<C: Cell>(core: &HostCore<C>, cell: &mut TenantCell<C::State, C::Message>) {
    loop {
        let Ok(mut state) = catch_unwind(AssertUnwindSafe(|| cell.checkpoint.clone())) else {
            lgwks_std::trace::warn!(
                "cell: the checkpoint clone panicked; the tenant restarts empty"
            );
            cell.state = C::State::default();
            cell.checkpoint = C::State::default();
            cell.tail.clear();
            cell.tail_bytes = 0;
            return;
        };
        let mut failed = None;
        for (at, message) in cell.tail.iter().enumerate() {
            let replayed = catch_unwind(AssertUnwindSafe(|| {
                let mut budget = Steps::new(MAX_STEPS_PER_MESSAGE);
                core.cell.apply(&mut state, message.clone(), &mut budget)
            }));
            if replayed.is_err() {
                failed = Some(at);
                break;
            }
        }
        match failed {
            None => {
                cell.state = state;
                return;
            }
            Some(at) => {
                lgwks_std::trace::warn!(
                    at,
                    "cell: a logged message panicked in replay; it is dropped"
                );
                let after = cell.tail.split_off(at.saturating_add(1));
                if let Some(removed) = cell.tail.pop() {
                    cell.tail_bytes = cell.tail_bytes.saturating_sub(C::weight(&removed));
                }
                cell.tail.extend(after);
            }
        }
    }
}
