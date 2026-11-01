//! Requests into the world: the HTTP edge's events (ADR 0006 R4).
//!
//! A [`RequestFeed`] is the world's request system and every tenant's state.
//! The edge hands it each request and writes back the [`Response`] it settles.
//! Underneath, every tenant owns a sequential cell on a thread-per-core host
//! (issue #790): the request is appended to the tenant's command log first,
//! then applied, and a panic rebuilds that one tenant from its log without the
//! request that panicked. Tenancy is the feed's, not the handler's: the host
//! keeps one state per tenant and hands a request only its own tenant's, so no
//! handler can read a neighbour's state however it is written. The tenants a
//! feed holds are bounded ([`MAX_TENANTS`]); a request for one more is
//! answered `503` without reaching the handler.
//!
//! A handler is synchronous and runs only on the worker that owns its tenant,
//! holding no lock: a request waits at most for the requests already queued
//! for its own worker, in a bounded mailbox. The edge submits without waiting
//! and awaits the reply; a full mailbox is a `503` with `Retry-After`.
//!
//! The feed names no driver (ADR 0006 decision 4): workers are `std` threads
//! and a reply is awaited through a `std` waker.

use core::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

#[cfg(test)]
use super::super::cell::CellError;
use super::super::cell::{
    Cell, CellHost, CellRefused, DEFAULT_MAILBOX_BOUND, DynHost, Effect, HostCounts, PendingReply,
    ReplyRunner, ReplySlot, Steps, SubmitRefused,
};

#[cfg(test)]
mod sim_requests;

/// The most tenants one feed keeps state for.
pub(crate) const MAX_TENANTS: usize = 65_536;

/// The longest tenant name a request may carry, in bytes.
pub(crate) const MAX_TENANT_BYTES: usize = 64;

/// Whether `tenant` names a tenant: 1..=64 bytes of `[A-Za-z0-9_-]`.
pub(crate) fn valid_tenant(tenant: &str) -> bool {
    !tenant.is_empty()
        && tenant.len() <= MAX_TENANT_BYTES
        && tenant
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// One request, as the world's request system receives it: a view of what
/// the edge read, so handing one to a handler copies nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request<'a> {
    /// Who the request is for; validated by [`Request::new`].
    tenant: &'a str,
    /// The HTTP method, upper case.
    method: &'a str,
    /// The path below the tenant prefix, starting with `/`.
    path: &'a str,
    /// The body, read whole by the edge under its own ceiling.
    body: &'a [u8],
}

/// Why the simulation door did not answer a request. Simulation-gated like
/// [`RequestFeed::answer`]: the shipped edge reports the same cases through
/// [`SubmitRefused`].
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum RequestRefused {
    /// The tenant is empty, longer than [`MAX_TENANT_BYTES`], or holds a byte
    /// outside `[A-Za-z0-9_-]`.
    Tenant,
    /// The feed was closed.
    Closed,
    /// A host lock is poisoned: no holder runs user code under one, so this
    /// names a bug in the host.
    Poisoned,
}

#[cfg(test)]
impl fmt::Display for RequestRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Tenant => {
                f.write_str("the tenant is not a name of 1..=64 bytes in [A-Za-z0-9_-]")
            }
            Self::Closed => f.write_str("the request feed is closed"),
            Self::Poisoned => f.write_str("a host lock is poisoned; the holder is a bug"),
        }
    }
}

#[cfg(test)]
impl std::error::Error for RequestRefused {}

impl<'a> Request<'a> {
    /// A request for `tenant`, as the simulations build one; the edge builds
    /// its requests through [`RequestFeed::try_submit`].
    ///
    /// # Errors
    ///
    /// [`RequestRefused::Tenant`] when `tenant` is not a valid tenant name.
    #[cfg(test)]
    pub(crate) fn new(
        tenant: &'a str,
        method: &'a str,
        path: &'a str,
        body: &'a [u8],
    ) -> Result<Self, RequestRefused> {
        if !valid_tenant(tenant) {
            lgwks_std::trace::debug!(tenant, "request: refused tenant name");
            return Err(RequestRefused::Tenant);
        }
        Ok(Self {
            tenant,
            method,
            path,
            body,
        })
    }

    /// Who the request is for.
    #[must_use]
    pub const fn tenant(&self) -> &'a str {
        self.tenant
    }

    /// The verb as the client sent it on the request line, `GET` or `POST`
    /// for most handlers; the edge passes it through unchanged.
    #[must_use]
    pub const fn method(&self) -> &'a str {
        self.method
    }

    /// The path below the tenant prefix.
    #[must_use]
    pub const fn path(&self) -> &'a str {
        self.path
    }

    /// Every byte the client sent after the head, read whole by the edge up to
    /// its 1 MiB ceiling; empty when the framing declared none.
    #[must_use]
    pub const fn body(&self) -> &'a [u8] {
        self.body
    }
}

/// The world's answer to one request: the settled effect the edge writes back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The HTTP status.
    status: u16,
    /// The body.
    body: Vec<u8>,
}

impl Response {
    /// An answer with `status` and `body`.
    #[must_use]
    pub const fn new(status: u16, body: Vec<u8>) -> Self {
        Self { status, body }
    }

    /// The HTTP status.
    #[must_use]
    pub(crate) const fn status(&self) -> u16 {
        self.status
    }

    /// The body, owned.
    #[must_use]
    pub(crate) fn into_body(self) -> Vec<u8> {
        self.body
    }
}

/// The counts a feed has recorded. Each is read on its own, so under load
/// they are a moment's reading, not one consistent cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct RequestCounts {
    /// Requests the handler answered, including contained panics.
    pub settled: u64,
    /// Requests refused before the handler: the feed closed, the tenant
    /// ceiling reached, a full mailbox, or a spent step budget.
    pub refused: u64,
    /// Tenants the feed holds state for.
    pub tenants: usize,
    /// Handler panics the host contained and rebuilt the tenant from its log.
    pub panicked: u64,
}

/// One logged request: what the tenant's tail keeps and the cell applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnedRequest {
    /// Who the request is for, validated at submit.
    tenant: Box<str>,
    /// The HTTP method.
    method: Box<str>,
    /// The path below the tenant prefix.
    path: Box<str>,
    /// The body the edge read.
    body: Box<[u8]>,
}

impl OwnedRequest {
    /// The owned copy of the parts the edge read.
    fn new(tenant: &str, method: &str, path: &str, body: &[u8]) -> Self {
        Self {
            tenant: tenant.into(),
            method: method.into(),
            path: path.into(),
            body: body.into(),
        }
    }

    /// The handler's view of this request.
    fn view(&self) -> Request<'_> {
        Request {
            tenant: &self.tenant,
            method: &self.method,
            path: &self.path,
            body: &self.body,
        }
    }
}

/// The request world's cell: `serve` applied to the tenant's state.
struct ServeCell<F, S> {
    /// The handler.
    serve: F,
    /// The state type, named through a function pointer so the cell is `Send`
    /// and `Sync` whatever the state is: states live on one worker thread.
    state: PhantomData<fn() -> S>,
}

impl<F, S> Cell for ServeCell<F, S>
where
    F: Fn(&Request<'_>, &mut S) -> Response + Send + Sync + 'static,
    S: Clone + Default + Send + 'static,
{
    type State = S;
    /// Shared, so keeping a request in the tail is a reference count, not a
    /// copy of its bytes.
    type Message = Arc<OwnedRequest>;

    fn tenant(message: &Arc<OwnedRequest>) -> &str {
        &message.tenant
    }

    fn weight(message: &Arc<OwnedRequest>) -> usize {
        message
            .tenant
            .len()
            .saturating_add(message.method.len())
            .saturating_add(message.path.len())
            .saturating_add(message.body.len())
    }

    fn apply(
        &self,
        state: &mut S,
        message: Arc<OwnedRequest>,
        budget: &mut Steps,
    ) -> Result<Vec<Effect>, CellRefused> {
        budget.spend(1)?;
        Ok(vec![Effect::Reply((self.serve)(&message.view(), state))])
    }
}

/// The world's request system and every tenant's state.
///
/// Cloning is a handle: the edge and the world share one feed.
#[derive(Clone)]
pub struct RequestFeed {
    /// The erased cell host: every tenant's log, state and worker.
    host: Arc<dyn DynHost<Arc<OwnedRequest>>>,
}

/// Worker threads for this host: the machine's parallelism, which the host
/// caps and rounds up to a power of two.
fn auto_workers() -> usize {
    match std::thread::available_parallelism() {
        Ok(threads) => threads.get(),
        Err(error) => {
            lgwks_std::trace::warn!(%error, "request: parallelism is unreadable; serving on four workers");
            4
        }
    }
}

impl RequestFeed {
    /// A feed answering every request with `serve`, given the state of the
    /// request's own tenant, created by `S::default()` on that tenant's first
    /// request. `S` is `Clone` because each tenant's log checkpoints its state
    /// by cloning it, which is what lets a panic rebuild the state exactly.
    #[must_use]
    pub fn new<S: Clone + Default + Send + 'static>(
        serve: impl Fn(&Request<'_>, &mut S) -> Response + Send + Sync + 'static,
    ) -> Self {
        Self::with_mailbox_bound(serve, DEFAULT_MAILBOX_BOUND)
    }

    /// A feed like [`RequestFeed::new`] whose per-worker mailbox holds at most
    /// `mailbox_bound` queued requests (at least one) before the edge answers
    /// `503`: a smaller bound sheds load sooner, a larger one queues more.
    /// Crate-private until a caller outside the crate sizes its own mailbox;
    /// the edge's backpressure test is the caller today.
    #[must_use]
    pub(crate) fn with_mailbox_bound<S: Clone + Default + Send + 'static>(
        serve: impl Fn(&Request<'_>, &mut S) -> Response + Send + Sync + 'static,
        mailbox_bound: usize,
    ) -> Self {
        let cell = ServeCell {
            serve,
            state: PhantomData,
        };
        match CellHost::start(cell, Arc::new(ReplyRunner), auto_workers(), mailbox_bound) {
            Ok(host) => Self {
                host: Arc::new(host),
            },
            Err(error) => {
                lgwks_std::trace::warn!(%error, "request: no worker could start; the feed refuses everything");
                let cell: ServeCell<_, S> = ServeCell {
                    serve: |_: &Request<'_>, _: &mut S| Response::new(503, Vec::new()),
                    state: PhantomData,
                };
                Self {
                    host: Arc::new(CellHost::closed(cell, Arc::new(ReplyRunner))),
                }
            }
        }
    }

    /// Answer `request` from its own tenant's state, waiting for room and for
    /// the reply: the simulation door. The shipped edge submits without
    /// waiting through [`RequestFeed::try_submit`].
    ///
    /// # Errors
    ///
    /// [`RequestRefused::Closed`] once the feed is closed and
    /// [`RequestRefused::Poisoned`] on a broken host lock. A request for a
    /// tenant past [`MAX_TENANTS`] is answered `503` here, not refused.
    #[cfg(test)]
    pub(crate) fn answer(&self, request: &Request<'_>) -> Result<Response, RequestRefused> {
        let owned = Arc::new(OwnedRequest::new(
            request.tenant,
            request.method,
            request.path,
            request.body,
        ));
        let reply = Arc::new(ReplySlot::new());
        if let Err(error) = self
            .host
            .submit_wait(request.tenant, owned, Arc::clone(&reply))
        {
            lgwks_std::trace::debug!(%error, tenant = request.tenant, "request: the waiting submit was refused");
            return Err(match error {
                CellError::Closed => RequestRefused::Closed,
                _ => RequestRefused::Poisoned,
            });
        }
        reply.wait_blocking().map_err(|error| {
            lgwks_std::trace::debug!(%error, "request: the reply slot broke");
            RequestRefused::Poisoned
        })
    }

    /// Queue one request without waiting, for the edge to await.
    ///
    /// # Errors
    ///
    /// [`SubmitRefused::Tenant`] for an invalid name,
    /// [`SubmitRefused::TooLarge`] past the message ceiling,
    /// [`SubmitRefused::Overloaded`] when the tenant's worker mailbox is full,
    /// and [`SubmitRefused::Closed`] once the feed is closed.
    pub(crate) fn try_submit(
        &self,
        tenant: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<PendingReply, SubmitRefused> {
        if !valid_tenant(tenant) {
            lgwks_std::trace::debug!(tenant, "request feed: refused tenant name");
            return Err(SubmitRefused::Tenant);
        }
        let reply = Arc::new(ReplySlot::new());
        let owned = Arc::new(OwnedRequest::new(tenant, method, path, body));
        self.host.try_submit(tenant, owned, Arc::clone(&reply))?;
        Ok(PendingReply::new(reply))
    }

    /// Refuse every later request. A request already queued finishes with its
    /// answer.
    pub(crate) fn close(&self) {
        self.host.close();
    }

    /// The counts this feed has recorded.
    #[must_use]
    pub fn counts(&self) -> RequestCounts {
        let HostCounts {
            settled,
            refused,
            tenants,
            panicked,
        } = self.host.counts();
        RequestCounts {
            settled,
            refused,
            tenants,
            panicked,
        }
    }
}

impl fmt::Debug for RequestFeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestFeed")
            .field("counts", &self.counts())
            .finish_non_exhaustive()
    }
}
