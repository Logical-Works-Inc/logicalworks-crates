//! Sequential per-tenant execution cells: one log, one state, one thread at a time.
//!
//! A [`Cell`] is the world's request system with its lock-striped partitions
//! removed (issue #790): every tenant owns a cell, a cell is a fold of its
//! tenant's command log, and a cell runs only on the worker thread that owns
//! it, so no `Mutex` or `RwLock` sits on the hot path. Ingress is a bounded
//! mailbox per worker; a tenant is routed by the FNV-1a hash the partitions
//! used, masked to the worker count, which is a power of two.
//!
//! The order of business for one message is fixed: the message is appended to
//! the tenant's command log, then applied. The log is a checkpoint of the
//! state plus the messages applied since it, so the state is always the
//! checkpoint folded with the tail. Every [`CHECKPOINT_EVERY`] messages or
//! [`CHECKPOINT_BYTES`] of tail the state is cloned into a new checkpoint and
//! the tail is cleared, which keeps the log bounded however long a tenant
//! lives: an event-sourced cell replays from its latest snapshot, never from
//! its first message.
//!
//! `apply` is synchronous: no clock, no I/O, no lock. A step budget travels
//! with every call, and a cell that spends past it is refused with a typed
//! refusal rather than stopped. A panic inside `apply` is caught at the cell
//! boundary: the half-mutated state is discarded, the poisoned message is
//! dropped from the tail, the state is rebuilt from the checkpoint and the
//! rest of the tail, the panic is counted and answered `500`, and no other
//! tenant is touched. No poisoned lock is ever recovered with `into_inner`.
//!
//! The log lives in the worker's memory. It survives a panic, which is what
//! a cell must contain; it does not survive the process, because a handler's
//! state carries no codec to write a checkpoint with.
//!
//! Effects are data. `apply` returns them and the host hands them to the
//! [`EffectRunner`] outside the cell. The world's feed delivers the reply
//! alone; the app SDK (#791) also serves fetch, ask, send, approval and
//! timers through its provider, their results re-entering as messages
//! through the mailbox.

use crate::cap::Cap;
use core::fmt;

mod host;
mod mailbox;
mod reply;
#[cfg(test)]
mod sim_cell;

pub(crate) use host::{CellHost, DynHost, HostCounts};
pub(crate) use reply::{PendingReply, ReplySlot};

/// The per-message step budget: how many steps one `apply` may spend.
/// A serve handler spends one; a future cell implementation spends per item.
pub(crate) const MAX_STEPS_PER_MESSAGE: u64 = 1_024;

/// The largest message the host admits, by [`Cell::weight`], in bytes.
/// The edge already caps bodies at 1 MiB, so this trips only on misuse.
pub(crate) const MAX_MESSAGE_BYTES: usize = 2 * 1024 * 1024;

/// Messages a tenant's tail holds before the state is checkpointed: a clone
/// of the state every 16 messages, against at most 16 retained per tenant.
pub(crate) const CHECKPOINT_EVERY: usize = 16;

/// Bytes of tail, by [`Cell::weight`], before the state is checkpointed.
pub(crate) const CHECKPOINT_BYTES: usize = 1024 * 1024;

/// How long an edge client is told to wait before retrying a full mailbox,
/// in milliseconds. The edge renders whole seconds, rounding up.
pub(crate) const RETRY_AFTER_MS: u64 = 200;

/// The default per-worker mailbox bound the request feed serves under.
pub(crate) const DEFAULT_MAILBOX_BOUND: usize = 1_024;

/// The most worker threads one host starts; the count is rounded up to a
/// power of two so the route is a mask, and capped here.
pub(crate) const MAX_WORKERS: usize = 64;

/// Steps left for one `apply` call. Spent explicitly by the cell; past zero
/// the spend is refused and the message is answered without running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Steps {
    /// Steps the call may still spend.
    remaining: u64,
}

impl Steps {
    /// A budget of `budget` steps.
    #[must_use]
    pub(crate) const fn new(budget: u64) -> Self {
        Self { remaining: budget }
    }

    /// Spend `steps`, refusing once the budget is gone.
    ///
    /// # Errors
    ///
    /// [`CellRefused::BudgetSpent`] when fewer than `steps` remain; nothing is
    /// spent on refusal.
    pub(crate) fn spend(&mut self, steps: u64) -> Result<(), CellRefused> {
        match self.remaining.checked_sub(steps) {
            Some(left) => {
                self.remaining = left;
                Ok(())
            }
            None => {
                lgwks_std::trace::debug!(
                    remaining = self.remaining,
                    wanted = steps,
                    "cell: the step budget is spent"
                );
                Err(CellRefused::BudgetSpent)
            }
        }
    }
}

/// Why a cell refused to apply a logged message. A refusal leaves the state
/// as it was, so the host drops the message from the tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum CellRefused {
    /// The step budget ran out before the message was applied.
    BudgetSpent,
}

impl fmt::Display for CellRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::BudgetSpent => f.write_str("the cell's step budget is spent"),
        }
    }
}

impl std::error::Error for CellRefused {}

/// What the cell host can fail with outside a submission.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum CellError {
    /// The host is closed; no message is admitted. Simulation-gated like the
    /// waiting submit, its only reporter.
    #[cfg(test)]
    Closed,
    /// A lock was poisoned. No holder runs user code while holding one, so
    /// this names a bug rather than a tenant, and nothing is retried under it.
    Poisoned {
        /// What the poisoned lock guards.
        what: &'static str,
    },
    /// A worker thread could not be started.
    Spawn(std::io::Error),
}

impl fmt::Display for CellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            #[cfg(test)]
            Self::Closed => f.write_str("the cell host is closed"),
            Self::Poisoned { what } => {
                write!(f, "the {what} lock is poisoned; the holder is a bug")
            }
            Self::Spawn(ref cause) => write!(f, "a cell worker could not start: {cause}"),
        }
    }
}

impl std::error::Error for CellError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Spawn(ref cause) => Some(cause),
            _ => None,
        }
    }
}

/// Why the host refused a submission before the cell saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum SubmitRefused {
    /// The tenant is not a name of 1..=64 bytes in `[A-Za-z0-9_-]`.
    Tenant,
    /// The host is closed.
    Closed,
    /// The tenant's worker mailbox is full. Retry after the named delay.
    Overloaded {
        /// Milliseconds the client should wait before retrying.
        retry_after_ms: u64,
    },
    /// The message is larger than [`MAX_MESSAGE_BYTES`].
    TooLarge {
        /// Bytes the message needed.
        bytes: usize,
        /// The ceiling that refused it.
        ceiling: usize,
    },
}

impl fmt::Display for SubmitRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Tenant => {
                f.write_str("the tenant is not a name of 1..=64 bytes in [A-Za-z0-9_-]")
            }
            Self::Closed => f.write_str("the cell host is closed"),
            Self::Overloaded { retry_after_ms } => write!(
                f,
                "the tenant mailbox is full; retry after {retry_after_ms}ms"
            ),
            Self::TooLarge { bytes, ceiling } => write!(
                f,
                "the message needs {bytes} bytes against a ceiling of {ceiling}"
            ),
        }
    }
}

impl std::error::Error for SubmitRefused {}

/// What a cell returns for the runner to deliver outside the cell.
///
/// A sum type rather than a bare reply so fetch, ask and send ride the same
/// seam: every variant after [`Effect::Reply`] carries its `reply_to` tag so
/// the runner can feed the provider's answer back through the mailbox.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Effect {
    /// Answer the waiting submitter with this response.
    Reply(crate::ecs::Response),
    /// Fetch `url` through the app's provider, under the `net` cap.
    Fetch {
        /// Where to fetch, clipped to 1 KiB.
        url: Box<str>,
        /// Which `on` handler receives the answer.
        reply_to: ReplyTag,
    },
    /// Ask a model `prompt` through the app's provider, under `app.ask`.
    Ask {
        /// The question, clipped to 4 KiB.
        prompt: Box<str>,
        /// Which `on` handler receives the answer.
        reply_to: ReplyTag,
    },
    /// Send `body` to `to` through the app's provider, under `notify`.
    Send {
        /// The recipient, clipped to 256 bytes.
        to: Box<str>,
        /// The payload, cut to 8 KiB.
        body: Box<[u8]>,
        /// Which `on` handler receives the receipt.
        reply_to: ReplyTag,
    },
    /// Wait for a human approval through the app's provider, under
    /// `app.approval`.
    AwaitApproval {
        /// What waits, clipped to 4 KiB.
        what: Box<str>,
        /// Which `on` handler receives the decision.
        reply_to: ReplyTag,
    },
    /// Wake after `millis` milliseconds, as data the provider schedules.
    After {
        /// The named delay, clamped to one hour.
        millis: u64,
        /// Which `on` handler receives the wake.
        reply_to: ReplyTag,
    },
}

/// The longest `reply_to` tag an effect may carry, in bytes.
pub(crate) const MAX_TAG_BYTES: usize = 64;

/// The longest `Fetch::url` an effect may carry, in bytes.
pub(crate) const MAX_URL_BYTES: usize = 1_024;

/// The longest `Ask` prompt or approval subject, in bytes.
pub(crate) const MAX_PROMPT_BYTES: usize = 4_096;

/// The longest `Send::to` recipient, in bytes.
pub(crate) const MAX_SEND_TO_BYTES: usize = 256;

/// The longest `Send::body` payload, in bytes.
pub(crate) const MAX_SEND_BODY_BYTES: usize = 8_192;

/// The longest `After::millis` delay, in milliseconds.
pub(crate) const MAX_AFTER_MILLIS: u64 = 3_600_000;

/// The longest prefix of `text` that is at most `ceiling` bytes and ends on a
/// character boundary.
pub(crate) fn clip(text: &str, ceiling: usize) -> Box<str> {
    let mut end = 0;
    for (at, character) in text.char_indices() {
        let next = at.saturating_add(character.len_utf8());
        if next > ceiling {
            break;
        }
        end = next;
    }
    match text.get(..end) {
        Some(kept) => kept.into(),
        None => Box::default(),
    }
}

/// The name of the `on` handler an effect's answer goes to, at most 64 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReplyTag(Box<str>);

/// What a provider answered, or the cap whose absence refused the dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EffectResult {
    /// The provider's bytes, bounded by the effect that asked.
    Value(Box<[u8]>),
    /// The dispatch was refused: the app holds no such cap.
    Refused(Cap),
    /// The app's provider does not serve this effect, or failed to.
    Unavailable,
}

/// What an `on` handler receives: which tag fired and what it carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnInput {
    /// Which effect answered.
    tag: ReplyTag,
    /// The provider's answer or the refusal.
    result: EffectResult,
}

impl ReplyTag {
    /// A tag naming an `on` handler, clipped to 64 bytes.
    #[must_use]
    pub fn new(tag: &str) -> Self {
        Self(clip(tag, MAX_TAG_BYTES))
    }

    /// The tag text.
    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Effect {
    /// Answer the waiting submitter with `status` and `body`.
    #[must_use]
    pub fn reply(status: u16, body: Vec<u8>) -> Self {
        let mut body = body;
        body.truncate(crate::rt::edge::MAX_BODY_BYTES);
        Self::Reply(crate::ecs::Response::new(status, body))
    }

    /// Which `on` handler this effect answers to, if it answers one.
    #[must_use]
    pub(crate) fn reply_to(&self) -> Option<&ReplyTag> {
        match *self {
            Self::Reply(_) => None,
            Self::Fetch { ref reply_to, .. }
            | Self::Ask { ref reply_to, .. }
            | Self::Send { ref reply_to, .. }
            | Self::AwaitApproval { ref reply_to, .. }
            | Self::After { ref reply_to, .. } => Some(reply_to),
        }
    }

    /// The cap the runner must hold to dispatch this effect; `None` needs none.
    #[must_use]
    pub(crate) fn required_cap(&self) -> Option<Cap> {
        match *self {
            Self::Reply(_) | Self::After { .. } => None,
            Self::Fetch { .. } => Some(Cap::net()),
            Self::Ask { .. } => Some(Cap::new("app.ask")),
            Self::Send { .. } => Some(Cap::notify()),
            Self::AwaitApproval { .. } => Some(Cap::new("app.approval")),
        }
    }

    /// Truncate every bounded field to its ceiling.
    #[must_use]
    pub(crate) fn bound(self) -> Self {
        match self {
            Self::Reply(response) => Self::Reply(response),
            Self::Fetch { url, reply_to } => Self::Fetch {
                url: clip(&url, MAX_URL_BYTES),
                reply_to,
            },
            Self::Ask { prompt, reply_to } => Self::Ask {
                prompt: clip(&prompt, MAX_PROMPT_BYTES),
                reply_to,
            },
            Self::Send { to, body, reply_to } => {
                let mut body = body.into_vec();
                body.truncate(MAX_SEND_BODY_BYTES);
                Self::Send {
                    to: clip(&to, MAX_SEND_TO_BYTES),
                    body: body.into_boxed_slice(),
                    reply_to,
                }
            }
            Self::AwaitApproval { what, reply_to } => Self::AwaitApproval {
                what: clip(&what, MAX_PROMPT_BYTES),
                reply_to,
            },
            Self::After { millis, reply_to } => Self::After {
                millis: millis.min(MAX_AFTER_MILLIS),
                reply_to,
            },
        }
    }
}

impl OnInput {
    /// What an `on` handler receives.
    #[must_use]
    pub(crate) fn new(tag: ReplyTag, result: EffectResult) -> Self {
        Self { tag, result }
    }

    /// Which effect answered.
    #[must_use]
    pub(crate) fn tag(&self) -> &ReplyTag {
        &self.tag
    }

    /// The provider's answer or the refusal.
    #[must_use]
    pub fn result(&self) -> &EffectResult {
        &self.result
    }
}

/// Delivers a cell's effects outside the cell, after `apply` returned.
///
/// The runner never runs inside `apply`: the host calls it between the apply
/// and the next message, and never for a message replayed in a rebuild, so a
/// logged message's effects are delivered exactly once.
pub(crate) trait EffectRunner: Send + Sync {
    /// Deliver `effects` for `tenant`; the reply they carry, if any.
    fn run(&self, tenant: &str, effects: Vec<Effect>) -> Option<crate::ecs::Response>;
}

/// The first [`Effect::Reply`] among `effects`; every other effect goes to
/// `other`. A second reply for one message is dropped and logged, and the
/// first stands.
pub(crate) fn first_reply(
    effects: Vec<Effect>,
    mut other: impl FnMut(Effect),
) -> Option<crate::ecs::Response> {
    let mut answer = None;
    for effect in effects {
        match effect {
            Effect::Reply(response) if answer.is_none() => answer = Some(response),
            Effect::Reply(_) => {
                lgwks_std::trace::debug!("cell: a second reply effect for one message is dropped");
            }
            effect => other(effect),
        }
    }
    answer
}

/// The world feed's runner: the first [`Effect::Reply`] is the answer.
///
/// A message whose cell returned no reply effect is answered `500` by the
/// host rather than left hanging.
#[derive(Debug, Default)]
pub(crate) struct ReplyRunner;

impl EffectRunner for ReplyRunner {
    fn run(&self, tenant: &str, effects: Vec<Effect>) -> Option<crate::ecs::Response> {
        first_reply(effects, |effect| {
            lgwks_std::trace::debug!(
                tenant,
                ?effect,
                "cell: the reply-only runner drops an effect"
            );
        })
    }
}

/// A sequential per-tenant execution core: synchronous, no clock, no I/O, no
/// lock.
///
/// The host owns every tenant outright on its worker threads and calls
/// `apply` with only the tenant's own state, so an implementation cannot read
/// a neighbour however it is written. Messages reach `apply` in log order and
/// the state is their fold, so replaying the tail onto the checkpoint
/// rebuilds the same state.
pub(crate) trait Cell: Send + Sync + 'static {
    /// The tenant's state: the fold of its log, cloned into each checkpoint.
    type State: Clone + Default + Send + 'static;
    /// One logged command, kept in the tail until the next checkpoint.
    type Message: Clone + Send + 'static;

    /// The tenant `message` is for: the key of the state it folds into.
    fn tenant(message: &Self::Message) -> &str;

    /// The bytes `message` holds, charged against [`MAX_MESSAGE_BYTES`] at
    /// submit and against [`CHECKPOINT_BYTES`] in the tail.
    fn weight(message: &Self::Message) -> usize;

    /// Fold one logged message into the tenant's state.
    ///
    /// Synchronous and total over the budget: no clock, no I/O, no lock, and
    /// a spend past `budget` is refused rather than taken.
    ///
    /// # Errors
    ///
    /// [`CellRefused`] when the message is past the budget; the tenant's state
    /// is left as it was.
    fn apply(
        &self,
        state: &mut Self::State,
        message: Self::Message,
        budget: &mut Steps,
    ) -> Result<Vec<Effect>, CellRefused>;
}
