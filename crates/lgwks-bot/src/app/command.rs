//! Where a command runs: a bounded pool of owned threads, off every worker.
//!
//! A command may block on a durable write, so it cannot run on the worker that
//! owns its tenant, which would stall that worker's other tenants. The runner hands
//! it here without waiting. Three bounds hold: how many threads run commands, how
//! many commands one host holds, and how many of those one tenant may hold, so a
//! noisy tenant is refused before it fills the pool (ADR 0025).
//!
//! A [`Ticket`] carries the request's reply slot, and with it the rule that a
//! dropped job answers: a ticket dropped on any path before its slot was handed on
//! answers `503` from its own `Drop`. Its permit is released the same way.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{Builder, JoinHandle};
use std::time::{Duration, Instant};

use super::{Principal, Provider};
use crate::cell::{EffectResult, Mailbox, PushRefused, ReplySlot, ReplyTag};
use crate::ecs::Response;

thread_local! {
    /// Which pool and place this thread serves, so a close that arrives on one of a
    /// pool's own threads can leave that one thread out of its join.
    static SERVING: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}

/// The most threads one pool starts.
const MAX_THREADS: usize = 256;

/// The most commands one host may hold, queued and running.
const MAX_WAITING: usize = 65_536;

/// The bounds commands run under.
///
/// The defaults (32 threads, 1,024 held, 256 per tenant, 30 s) are estimates sized
/// to a group commit of a few milliseconds; measure your own service and set these.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// use lgwks_bot::app::CommandLimits;
///
/// let limits = CommandLimits::new(8, 64, 16, Duration::from_secs(5));
/// assert_eq!(limits.deadline(), Duration::from_secs(5));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandLimits {
    /// Threads that run commands.
    threads: usize,
    /// Commands one host holds, queued and running.
    waiting: usize,
    /// Commands one tenant holds, queued and running.
    per_tenant: usize,
    /// How long a request may be held, and a command may queue.
    deadline: Duration,
}

impl CommandLimits {
    /// `threads` running commands, `waiting` held by the host and `per_tenant` by one
    /// tenant, each at least one and at most 256 threads and 65,536 held. A request
    /// held past `deadline` is answered `504`, and a command queued past it is not run.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use lgwks_bot::app::CommandLimits;
    ///
    /// // A zero never means "no threads": each bound is at least one.
    /// let least = CommandLimits::new(0, 0, 0, Duration::ZERO);
    /// assert_eq!(least, CommandLimits::new(1, 1, 1, Duration::ZERO));
    /// ```
    #[must_use]
    pub fn new(threads: usize, waiting: usize, per_tenant: usize, deadline: Duration) -> Self {
        let held = waiting.clamp(1, MAX_WAITING);
        Self {
            threads: threads.clamp(1, MAX_THREADS),
            waiting: held,
            per_tenant: per_tenant.clamp(1, held),
            deadline,
        }
    }

    /// How long a request may be held, and a command may queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use lgwks_bot::app::CommandLimits;
    ///
    /// assert_eq!(CommandLimits::default().deadline(), Duration::from_secs(30));
    /// ```
    #[must_use]
    pub const fn deadline(&self) -> Duration {
        self.deadline
    }
}

impl Default for CommandLimits {
    fn default() -> Self {
        Self::new(32, 1_024, 256, Duration::from_secs(30))
    }
}

/// Where an answer goes back in, so the pool stays clear of the app's state type.
pub(super) trait Feedback: Send + Sync {
    /// Answer `reply_to` for `tenant`, settling `slot` if nothing else can.
    fn feed(
        &self,
        tenant: &str,
        asker: Option<Principal>,
        reply_to: ReplyTag,
        result: EffectResult,
        slot: Option<Arc<ReplySlot>>,
    );
}

/// A command the runner hands to the pool.
pub(super) struct CommandRequest<'tenant> {
    /// Who it is for.
    pub(super) tenant: &'tenant str,
    /// Who asked for it.
    pub(super) asker: Option<Principal>,
    /// The command.
    pub(super) body: Box<[u8]>,
    /// Which `on` handler receives the answer.
    pub(super) reply_to: ReplyTag,
    /// The request it holds open, if it holds one.
    pub(super) slot: Option<Arc<ReplySlot>>,
}

/// How many commands are held, in all and per tenant.
#[derive(Default)]
struct Held {
    /// Commands held in all.
    total: usize,
    /// Commands held per tenant; a tenant holding none has no entry, so the table
    /// never outgrows the host's own bound.
    by_tenant: BTreeMap<Box<str>, usize>,
}

/// The two ceilings on what is held, charged before a command is queued.
struct Permits {
    /// What is held now.
    held: Mutex<Held>,
    /// Commands one host holds.
    waiting: usize,
    /// Commands one tenant holds.
    per_tenant: usize,
}

impl Permits {
    /// Charge one command to `tenant`, or `None` past either ceiling.
    fn admit(permits: &Arc<Self>, tenant: &str) -> Option<Permit> {
        let Ok(mut held) = permits.held.lock() else {
            lgwks_std::trace::warn!("app: the command permits lock is poisoned");
            return None;
        };
        let mine = match held.by_tenant.get(tenant) {
            Some(count) => *count,
            None => 0,
        };
        if held.total >= permits.waiting || mine >= permits.per_tenant {
            lgwks_std::trace::debug!(
                tenant,
                mine,
                total = held.total,
                "app: a command is past its ceiling"
            );
            return None;
        }
        held.total = held.total.saturating_add(1);
        held.by_tenant.insert(tenant.into(), mine.saturating_add(1));
        drop(held);
        Some(Permit {
            permits: Arc::clone(permits),
            tenant: tenant.into(),
        })
    }
}

/// One held command's charge, given back when it is dropped.
struct Permit {
    /// The ceilings it was charged against.
    permits: Arc<Permits>,
    /// The tenant it was charged to.
    tenant: Box<str>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let Ok(mut held) = self.permits.held.lock() else {
            lgwks_std::trace::warn!("app: the command permits lock is poisoned");
            return;
        };
        held.total = held.total.saturating_sub(1);
        match held.by_tenant.get(&self.tenant).copied() {
            Some(count) if count > 1 => {
                held.by_tenant
                    .insert(self.tenant.clone(), count.saturating_sub(1));
            }
            Some(_) | None => {
                held.by_tenant.remove(&self.tenant);
            }
        }
    }
}

/// One queued command, and the duty to answer whoever waits on it.
struct Ticket {
    /// Who it is for.
    tenant: Box<str>,
    /// Who asked for it, taken when the answer is fed back.
    asker: Option<Principal>,
    /// The command.
    body: Box<[u8]>,
    /// Which `on` handler receives the answer.
    reply_to: ReplyTag,
    /// The request it holds open, taken when the answer is fed back.
    slot: Option<Arc<ReplySlot>>,
    /// When it was queued, against the deadline.
    queued: Instant,
    /// Its charge against the ceilings, given back before its waiter hears, so a
    /// client that has its answer finds its room already free.
    permit: Option<Permit>,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        drop(self.permit.take());
        if let Some(slot) = self.slot.take()
            && !slot.is_settled()
        {
            lgwks_std::trace::debug!(tenant = &*self.tenant, "app: a command was dropped unrun");
            slot.complete(Response::new(
                503,
                b"the command did not run; retry with the request's key".to_vec(),
            ));
        }
    }
}

/// What every thread of one pool shares.
struct PoolCore {
    /// The queue, bounded by the host's ceiling.
    queue: Mailbox<Ticket>,
    /// Who serves commands.
    provider: Arc<dyn Provider>,
    /// Where answers go back in.
    feed: Arc<dyn Feedback>,
    /// How long a command may queue.
    deadline: Duration,
    /// Whether the pool is stopping, so a queued command is not started.
    closing: AtomicBool,
}

impl PoolCore {
    /// Run commands until the queue closes and drains.
    fn serve(&self) {
        loop {
            match self.queue.take_one() {
                Ok(Some(ticket)) => self.run_one(ticket),
                Ok(None) => return,
                Err(error) => {
                    lgwks_std::trace::warn!(%error, "app: a command thread stops on a broken queue");
                    return;
                }
            }
        }
    }

    /// Run one command, unless the pool is stopping or it queued too long, and feed
    /// its answer back. A panicking service is contained here, as a cell's panic is.
    fn run_one(&self, mut ticket: Ticket) {
        if self.closing.load(Ordering::Acquire) {
            return;
        }
        let result = if ticket.queued.elapsed() >= self.deadline {
            lgwks_std::trace::debug!(
                tenant = &*ticket.tenant,
                "app: a command queued past its deadline"
            );
            EffectResult::Unavailable
        } else {
            let served = catch_unwind(AssertUnwindSafe(|| {
                self.provider
                    .command(&ticket.tenant, ticket.asker.as_ref(), &ticket.body)
            }));
            match served {
                Ok(Some(bytes)) => EffectResult::Value(bytes.into_boxed_slice()),
                Ok(None) => EffectResult::Unavailable,
                Err(_) => {
                    lgwks_std::trace::warn!(
                        tenant = &*ticket.tenant,
                        "app: a command service panicked"
                    );
                    EffectResult::Unavailable
                }
            }
        };
        drop(ticket.permit.take());
        let (asker, slot) = (ticket.asker.take(), ticket.slot.take());
        self.feed
            .feed(&ticket.tenant, asker, ticket.reply_to.clone(), result, slot);
    }
}

/// The pool of one host: its threads, its queue and its ceilings.
///
/// Spawned only for an app granted `app.command`, and joined when the host closes,
/// so no thread outlives its host. A service that never returns holds that join, as
/// it would hold a worker were it served inline; ADR 0025 says so.
pub(super) struct CommandPool {
    /// What the threads share.
    core: Arc<PoolCore>,
    /// The ceilings charged before a command queues.
    permits: Arc<Permits>,
    /// The threads, taken and joined on close.
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl CommandPool {
    /// Start `limits`' threads serving `provider`.
    ///
    /// # Errors
    ///
    /// The cause when a thread cannot start; those already started are joined first.
    pub(super) fn start(
        limits: &CommandLimits,
        provider: Arc<dyn Provider>,
        feed: Arc<dyn Feedback>,
    ) -> Result<Self, std::io::Error> {
        let pool = Self {
            core: Arc::new(PoolCore {
                queue: Mailbox::new(limits.waiting),
                provider,
                feed,
                deadline: limits.deadline,
                closing: AtomicBool::new(false),
            }),
            permits: Arc::new(Permits {
                held: Mutex::new(Held::default()),
                waiting: limits.waiting,
                per_tenant: limits.per_tenant,
            }),
            threads: Mutex::new(Vec::with_capacity(limits.threads)),
        };
        let mut started = Vec::with_capacity(limits.threads);
        for thread in 0..limits.threads {
            let core = Arc::clone(&pool.core);
            let spawned = Builder::new()
                .name(format!("lgwks-command-{thread:03}"))
                .spawn(move || {
                    SERVING.set(Some((Arc::as_ptr(&core).addr(), thread)));
                    core.serve();
                });
            match spawned {
                Ok(handle) => started.push(handle),
                Err(cause) => {
                    lgwks_std::trace::warn!(%cause, thread, "app: a command thread could not start");
                    pool.keep(started);
                    pool.close();
                    return Err(cause);
                }
            }
        }
        pool.keep(started);
        Ok(pool)
    }

    /// Hold `handles` for the close that joins them.
    fn keep(&self, handles: Vec<JoinHandle<()>>) {
        match self.threads.lock() {
            Ok(mut held) => held.extend(handles),
            Err(_) => lgwks_std::trace::warn!("app: the command thread table is poisoned"),
        }
    }

    /// Queue `request` without waiting; it comes back when a ceiling refuses it.
    ///
    /// # Errors
    ///
    /// The request itself, past the host's or the tenant's ceiling.
    pub(super) fn hand<'tenant>(
        &self,
        request: CommandRequest<'tenant>,
    ) -> Result<(), CommandRequest<'tenant>> {
        let Some(permit) = Permits::admit(&self.permits, request.tenant) else {
            lgwks_std::trace::debug!(tenant = %request.tenant, "app: a command ceiling refused this request");
            return Err(request);
        };
        let ticket = Ticket {
            tenant: request.tenant.into(),
            asker: request.asker,
            body: request.body,
            reply_to: request.reply_to,
            slot: request.slot,
            queued: Instant::now(),
            permit: Some(permit),
        };
        // A permit is charged first, so the queue fills only as the host closes or
        // its lock breaks. The refused ticket answers its waiter as it drops.
        match self.core.queue.try_push(ticket) {
            Ok(()) => {}
            Err(PushRefused::Closed(refused)) | Err(PushRefused::Poisoned(refused)) => {
                lgwks_std::trace::debug!(
                    tenant = request.tenant,
                    "app: a closed pool refused a command"
                );
                drop(refused);
            }
            Err(PushRefused::Full(refused)) => {
                lgwks_std::trace::warn!(
                    tenant = request.tenant,
                    "app: the command queue filled under its permits"
                );
                drop(refused);
            }
        }
        Ok(())
    }

    /// Whether a close has begun, for a sim that must act after it did.
    #[cfg(test)]
    pub(super) fn is_closing(&self) -> bool {
        self.core.closing.load(Ordering::Acquire)
    }

    /// Start no more commands, answer what is queued and join the threads. Idempotent.
    ///
    /// A thread holding the last handle on the host can arrive here itself, so it
    /// joins everyone but itself; it exits as soon as this returns.
    pub(super) fn close(&self) {
        self.core.closing.store(true, Ordering::Release);
        self.core.queue.close();
        let handles = match self.threads.lock() {
            Ok(mut held) => core::mem::take(&mut *held),
            Err(_) => {
                lgwks_std::trace::warn!("app: the command thread table is poisoned");
                Vec::new()
            }
        };
        let pool = Arc::as_ptr(&self.core).addr();
        for (place, handle) in handles.into_iter().enumerate() {
            if SERVING.get() != Some((pool, place)) && handle.join().is_err() {
                lgwks_std::trace::warn!("app: a command thread panicked on the way out");
            }
        }
    }
}
