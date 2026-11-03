//! The tenant HTTP edge for `serve` and `route` words (#782).
//!
//! Axum in, typed handlers out: every request names its tenant in its path
//! (`/t/{tenant}/{rest}`, as on the world edge), carries an idempotency key
//! (the client's `Idempotency-Key` header, else the digest of tenant, route
//! and body), and runs once through the tenant's [`Host`](crate::task::Host)
//! request-key path: [`Host::submit`](crate::task::Host::submit) records the
//! input digest first, reattaches a repeat to the first run's recorded
//! result, and answers a changed payload under the same key as a typed
//! `Conflict`. A repeat with the same key and body never runs the body twice,
//! even across a `kill -9` and restart over the same store, because the
//! records live in the run store, never in ad hoc files. Over the in-flight
//! ceiling the answer is `503` with `Retry-After`; there is never an
//! unbounded queue. Stop is graceful: stop accepting, drain within the
//! declared deadline, cancel the rest through their tokens, and report both
//! counts. The one spawned server task is owned by a `JoinSet` plus a
//! [`CancellationToken`]; every in-flight request holds an RAII guard that
//! releases its live count in `Drop`.

use std::collections::HashMap;
use std::future::Future;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use lgwks_deps::axum::{
    Router,
    extract::{Request as HttpRequest, State},
    http::HeaderMap,
    response::Response as HttpResponse,
};

use lgwks_std::hash::Hasher;
use lgwks_std::wire::{Archive, Deserialize, Serialize};

use crate::effect::InputIdentity;
use crate::rt::edge::{EdgeStop, answer, read_body, tenant_of};
use crate::rt::sync::CancellationToken;
use crate::script::Scope;
use crate::task::{Host, RequestError, RequestKey, Submission, task};

/// A route's verb, read by the edge's dispatch: `GET` or `POST`.
pub use lgwks_ast::script::RouteMethod;

/// The in-flight ceiling: at most this many runs at once per serve.
const MAX_RUNS_IN_FLIGHT_U64: u64 = 65_536;

/// The `Retry-After` delay answered with every `503`, in seconds.
const RETRY_AFTER_SECS: u64 = 1;

/// How long a graceful drain waits for in-flight runs before cancelling.
const DEFAULT_DRAIN_GRACE: Duration = Duration::from_secs(30);

/// Why a `serve` could not be built or run.
#[derive(Debug)]
#[non_exhaustive]
pub enum ServeError {
    /// The port or ceiling was refused.
    InvalidBound {
        /// What was refused.
        what: &'static str,
        /// The refused value.
        value: u64,
        /// The ceiling refused against.
        max: u64,
    },
    /// The environment did not name a usable port.
    Env {
        /// The variable read.
        var: String,
        /// What was wrong.
        reason: String,
    },
    /// The OS refused the listener, the driver, or the run store.
    Io(std::io::Error),
    /// A tenant's host could not be built.
    Host {
        /// What the host refused.
        reason: String,
    },
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::InvalidBound { what, value, max } => {
                write!(
                    formatter,
                    "lgwks_bot: the serve {what} is {value}, past the ceiling {max}"
                )
            }
            Self::Env {
                ref var,
                ref reason,
            } => {
                write!(
                    formatter,
                    "lgwks_bot: the serve port from env {var} is refused: {reason}"
                )
            }
            Self::Io(ref cause) => write!(formatter, "lgwks_bot: the serve edge failed: {cause}"),
            Self::Host { ref reason } => {
                write!(formatter, "lgwks_bot: the serve host is refused: {reason}")
            }
        }
    }
}

impl std::error::Error for ServeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::InvalidBound { .. } | Self::Env { .. } | Self::Host { .. } => None,
            Self::Io(ref cause) => Some(cause),
        }
    }
}

/// Refuse with a typed bound error, logged as every refusal is.
fn invalid_bound<T>(what: &'static str, value: u64, max: u64) -> Result<T, ServeError> {
    let refusal = Err(ServeError::InvalidBound { what, value, max });
    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "serve: the bound is refused");
    refusal
}

/// The port the environment variable `var` names, or the typed refusal.
fn env_port(var: &str) -> Result<u16, ServeError> {
    let refused = |reason: String| {
        let refusal = ServeError::Env {
            var: var.to_owned(),
            reason,
        };
        lgwks_std::trace::debug!(error = ?refusal, "serve: the env port is refused");
        refusal
    };
    let text = std::env::var(var)
        .map_err(|cause| refused(format!("the variable could not be read: {cause}")))?;
    text.trim()
        .parse()
        .map_err(|cause: std::num::ParseIntError| {
            refused(format!("the value is not a port: {cause}"))
        })
}

/// What `serve` listens on and under.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// The port to bind.
    port: u16,
    /// At most this many runs in flight; past it the edge answers `503`.
    max_in_flight: usize,
    /// How long a graceful stop drains before cancelling the rest.
    drain_grace: Duration,
    /// Where the run store lives; every idempotency record with it.
    store_dir: PathBuf,
    /// Whether the store dir is a per-serve scratch removed on drain.
    ephemeral: bool,
}

/// A unique scratch dir for one ephemeral serve.
fn ephemeral_store() -> Result<PathBuf, ServeError> {
    let unique = lgwks_std::random::bytes::<8>().map_err(|cause| {
        ServeError::Io(std::io::Error::other(format!(
            "the store entropy is refused: {cause}"
        )))
    })?;
    let hex: String = unique.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(std::env::temp_dir().join(format!("lgwks-serve-{hex}")))
}

impl ServeOptions {
    /// Serve `port` with at most `max_in_flight` runs in flight, over `store_dir`.
    ///
    /// # Errors
    ///
    /// [`ServeError::InvalidBound`] for a zero ceiling or one past `65,536`.
    pub fn new(port: u16, max_in_flight: u64, store_dir: PathBuf) -> Result<Self, ServeError> {
        if max_in_flight == 0 || max_in_flight > MAX_RUNS_IN_FLIGHT_U64 {
            return invalid_bound("in-flight ceiling", max_in_flight, MAX_RUNS_IN_FLIGHT_U64);
        }
        let max_in_flight = match usize::try_from(max_in_flight) {
            Ok(max) => max,
            Err(error) => {
                let refusal = ServeError::InvalidBound {
                    what: "in-flight ceiling",
                    value: max_in_flight,
                    max: MAX_RUNS_IN_FLIGHT_U64,
                };
                lgwks_std::trace::debug!(error = ?error, "serve: the ceiling does not fit this target");
                return Err(refusal);
            }
        };
        Ok(Self {
            port,
            max_in_flight,
            drain_grace: DEFAULT_DRAIN_GRACE,
            store_dir,
            ephemeral: false,
        })
    }

    /// Serve `port` with at most `max_in_flight` runs in flight, over a
    /// per-serve scratch run store removed on drain.
    ///
    /// # Errors
    ///
    /// [`ServeError::InvalidBound`] for a zero ceiling or one past `65,536`;
    /// [`ServeError::Io`] when the store entropy or directory is refused.
    pub fn new_ephemeral(port: u16, max_in_flight: u64) -> Result<Self, ServeError> {
        let mut options = Self::new(port, max_in_flight, ephemeral_store()?)?;
        options.ephemeral = true;
        Ok(options)
    }

    /// Serve the port named by `var`, with at most `max_in_flight` runs, over `store_dir`.
    ///
    /// # Errors
    ///
    /// [`ServeError::Env`] when the variable is missing or is not a port;
    /// [`ServeError::InvalidBound`] for a refused ceiling.
    pub fn from_env(var: &str, max_in_flight: u64, store_dir: PathBuf) -> Result<Self, ServeError> {
        Self::new(env_port(var)?, max_in_flight, store_dir)
    }

    /// Serve the port named by `var` over a per-serve scratch run store.
    ///
    /// # Errors
    ///
    /// [`ServeError::Env`] when the variable is missing or is not a port;
    /// [`ServeError::InvalidBound`] for a refused ceiling;
    /// [`ServeError::Io`] when the store entropy or directory is refused.
    pub fn from_env_ephemeral(var: &str, max_in_flight: u64) -> Result<Self, ServeError> {
        Self::new_ephemeral(env_port(var)?, max_in_flight)
    }

    /// The number the listener binds.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// At most this many runs in flight.
    #[must_use]
    pub const fn max_in_flight(&self) -> usize {
        self.max_in_flight
    }

    /// Stop accepting and wait this long for in-flight runs before cancelling them.
    #[must_use]
    pub fn with_drain_grace(mut self, grace: Duration) -> Self {
        self.drain_grace = grace;
        self
    }

    /// How long a graceful stop drains before cancelling the rest.
    #[must_use]
    pub const fn drain_grace(&self) -> Duration {
        self.drain_grace
    }

    /// Where the run store lives; every idempotency record with it.
    #[must_use]
    pub fn store_dir(&self) -> &Path {
        self.store_dir.as_path()
    }
}

/// What a graceful stop counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServeReport {
    /// The port served.
    port: u16,
    /// Requests that answered before the stop.
    drained: usize,
    /// In-flight requests cancelled past the drain deadline.
    cancelled: usize,
}

impl ServeReport {
    /// The number the listener bound.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Requests that answered before the stop.
    #[must_use]
    pub const fn drained(&self) -> usize {
        self.drained
    }

    /// In-flight requests cancelled past the drain deadline.
    #[must_use]
    pub const fn cancelled(&self) -> usize {
        self.cancelled
    }
}

/// The typed route and its one handler shape.
mod route;
pub use route::TypedRoute;

/// The OpenAPI 3.1 projection of the route registry (#830).
pub mod openapi;

/// The request body as the Host request-key path sees it.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv, compare(PartialEq), derive(Debug))]
struct ServeInput {
    /// The raw body bytes the edge read.
    body: Vec<u8>,
}

impl InputIdentity for ServeInput {
    /// The schema binding this digest to serve bodies.
    const SCHEMA_ID: &'static [u8] = b"lgwks.bot.schema.v1.serve-body";

    /// The body bytes, framed.
    fn write_identity(&self, hasher: &mut Hasher) {
        hasher.write_framed(self.body.as_slice());
    }
}

/// The answered route as the Host request-key path records it.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv, compare(PartialEq), derive(Debug))]
struct ServeOutput {
    /// The HTTP status to answer.
    status: u16,
    /// The response bytes.
    body: Vec<u8>,
}

/// Shared serve state behind every request.
struct Shared {
    /// The routes in registration order.
    routes: Vec<TypedRoute>,
    /// At most `max_in_flight` permits; past it the edge answers `503`.
    room: lgwks_deps::tokio::sync::Semaphore,
    /// Cancelled on graceful stop; in-flight runs observe it.
    token: CancellationToken,
    /// One host per tenant, each over the same run store dir.
    hosts: std::sync::Mutex<HashMap<String, Host>>,
    /// Where every host's run store lives.
    store_dir: PathBuf,
    /// A per-serve scratch removed on drain, when the store is ephemeral.
    ephemeral_store: Option<PathBuf>,
    /// The per-host admission ceiling.
    max_in_flight: usize,
    /// Requests answered before the stop.
    drained: AtomicUsize,
    /// Requests still in flight, released by an RAII guard in `Drop`.
    live: AtomicUsize,
}

impl Drop for Shared {
    /// Remove a per-serve scratch store; a caller-named store is left alone.
    fn drop(&mut self) {
        if let Some(ref dir) = self.ephemeral_store
            && let Err(error) = std::fs::remove_dir_all(dir)
        {
            lgwks_std::trace::debug!(error = ?error, "serve: the scratch store was not removed");
        }
    }
}

/// What a driven request-key submission concluded, in owned bytes.
enum SubmitOutcome {
    /// Answer with this status and body.
    Answered {
        /// The HTTP status to answer.
        status: u16,
        /// The response body.
        body: Vec<u8>,
    },
    /// The first run is still in flight; retry with the same key.
    InFlight,
    /// The key is bound to a different input.
    Conflict {
        /// What collided, naming the key and both digests.
        message: String,
    },
    /// The submission itself was refused.
    Refused {
        /// What refused it.
        message: String,
    },
    /// The serve stopped before the run started; the run was never admitted.
    Stopping,
}

/// An in-flight request's live count, released in `Drop`.
struct LiveGuard {
    /// The shared counts this request entered.
    shared: Arc<Shared>,
}

impl LiveGuard {
    /// Enter one in-flight request.
    fn enter(shared: &Arc<Shared>) -> Self {
        shared.live.fetch_add(1, Ordering::SeqCst);
        Self {
            shared: Arc::clone(shared),
        }
    }
}

impl Drop for LiveGuard {
    /// Leave, however the handler future ended: completion, error, or drop.
    fn drop(&mut self) {
        self.shared.live.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The idempotency key for this request: the client's header, else the digest
/// of tenant, route and body.
fn idempotency_key(
    headers: &HeaderMap,
    tenant: &str,
    method: &str,
    path: &str,
    body: &[u8],
) -> String {
    if let Some(header) = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
    {
        let trimmed = header.trim();
        if !trimmed.is_empty() && trimmed.len() <= 128 {
            let allowed = trimmed
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte));
            if allowed {
                return trimmed.to_owned();
            }
        }
    }
    let mut hasher = Hasher::new();
    hasher.write_framed(b"lgwks.bot.serve.v1");
    hasher.write_framed(tenant.as_bytes());
    hasher.write_framed(method.as_bytes());
    hasher.write_framed(path.as_bytes());
    hasher.write_framed(body);
    let digest = hasher.finalize();
    lgwks_std::hex::encode(digest.as_bytes())
}

/// The host for `tenant`, built once over the shared run store dir.
///
/// One lock held from the first lookup through the build, so two first
/// requests for one tenant admit one host: two builders would open one
/// journal file twice, and the second open is refused as locked.
fn host_for(shared: &Arc<Shared>, tenant: &str) -> Result<Host, ServeError> {
    let mut hosts = shared.hosts.lock().map_err(|cause| {
        let refusal = ServeError::Host {
            reason: format!("the tenant hosts are poisoned: {cause}"),
        };
        lgwks_std::trace::debug!(error = ?refusal, "serve: the host lock is refused");
        refusal
    })?;
    if let Some(host) = hosts.get(tenant) {
        return Ok(host.clone());
    }
    let bound = match std::num::NonZeroUsize::new(shared.max_in_flight) {
        Some(bound) => bound,
        None => {
            return invalid_bound("in-flight ceiling", 0, MAX_RUNS_IN_FLIGHT_U64);
        }
    };
    let stored = Host::builder(tenant)
        .map_err(|cause| {
            let refusal = ServeError::Host {
                reason: format!("the tenant is refused: {cause}"),
            };
            lgwks_std::trace::debug!(error = ?refusal, "serve: the host builder is refused");
            refusal
        })?
        .max_concurrent_tasks(bound)
        .run_store(shared.store_dir.as_path())
        .map_err(|cause| {
            let refusal = ServeError::Io(std::io::Error::other(format!(
                "the run store is refused: {cause}"
            )));
            lgwks_std::trace::debug!(error = ?refusal, "serve: the run store is refused");
            refusal
        })?
        .build()
        .map_err(|cause| {
            let refusal = ServeError::Host {
                reason: format!("the host is refused: {cause}"),
            };
            lgwks_std::trace::debug!(error = ?refusal, "serve: the host is refused");
            refusal
        })?;
    hosts.insert(tenant.to_owned(), stored.clone());
    Ok(stored)
}

/// Serve `options`' routes on `listener` until `stop`, then drain.
///
/// Blocks the calling thread on the process driver, like the world edge:
/// axum stops accepting on `stop`, in-flight runs drain within
/// `options.drain_grace`, the rest are cancelled through their token, and the
/// report counts both.
pub fn serve(
    options: &ServeOptions,
    routes: Vec<TypedRoute>,
    listener: TcpListener,
    stop: &EdgeStop,
) -> Result<ServeReport, ServeError> {
    let driver = crate::rt::runtime::driver().map_err(|cause| {
        let refusal = ServeError::Io(cause);
        lgwks_std::trace::debug!(error = ?refusal, "serve: the driver is refused");
        refusal
    })?;
    driver.block_on(serve_async(
        options.clone(),
        routes,
        listener,
        stop.token().clone(),
    ))
}

/// Serve until `stop` is cancelled: accept, drain, cancel, report.
///
/// The one spawned server task is owned by a `JoinSet`; `stop` cancels it
/// through the shared token.
pub(crate) async fn serve_async(
    options: ServeOptions,
    routes: Vec<TypedRoute>,
    listener: TcpListener,
    stop: CancellationToken,
) -> Result<ServeReport, ServeError> {
    if let Err(error) = listener.set_nonblocking(true) {
        let refusal = ServeError::Io(error);
        lgwks_std::trace::debug!(error = ?refusal, "serve: the listener is refused");
        return Err(refusal);
    }
    if let Err(error) = std::fs::create_dir_all(options.store_dir.as_path()) {
        let refusal = ServeError::Io(error);
        lgwks_std::trace::debug!(error = ?refusal, "serve: the run store dir is refused");
        return Err(refusal);
    }
    let bound_listener =
        lgwks_deps::tokio::net::TcpListener::from_std(listener).map_err(|cause| {
            let refusal = ServeError::Io(cause);
            lgwks_std::trace::debug!(error = ?refusal, "serve: the listener is refused");
            refusal
        })?;
    let shared = Arc::new(Shared {
        routes,
        room: lgwks_deps::tokio::sync::Semaphore::new(options.max_in_flight),
        token: stop.clone(),
        hosts: std::sync::Mutex::new(HashMap::new()),
        store_dir: options.store_dir.clone(),
        ephemeral_store: if options.ephemeral {
            Some(options.store_dir.clone())
        } else {
            None
        },
        max_in_flight: options.max_in_flight,
        drained: AtomicUsize::new(0),
        live: AtomicUsize::new(0),
    });
    let router = Router::new()
        .fallback(ingress)
        .with_state(Arc::clone(&shared));
    let port = options.port;
    let grace = options.drain_grace;
    let mut owned = lgwks_deps::tokio::task::JoinSet::new();
    let serve_token = stop.clone();
    owned.spawn(async move {
        let http = lgwks_deps::axum::serve(bound_listener, router)
            .with_graceful_shutdown(serve_token.cancelled_owned());
        if let Err(error) = http.await {
            lgwks_std::trace::debug!(error = ?error, "serve: the server ended with an error");
        }
    });
    stop.cancelled().await;
    let (drained, cancelled) = drain_owned(&mut owned, shared, grace).await;
    Ok(ServeReport {
        port,
        drained,
        cancelled,
    })
}

/// Drain the owned server task within `grace`, reporting answered requests.
///
/// The counts come from the shared drained counter read after the drain;
/// cancelled is what was still live then. Past the deadline the hosts are
/// cancelled through their tokens before the server task is aborted.
async fn drain_owned(
    owned: &mut lgwks_deps::tokio::task::JoinSet<()>,
    shared: Arc<Shared>,
    grace: Duration,
) -> (usize, usize) {
    let waited = crate::rt::time::timeout(grace, async {
        while owned.join_next().await.is_some() {
            // One owned task: the server. Requests run inside it.
        }
    })
    .await;
    if waited.is_err() {
        lgwks_std::trace::debug!("serve: the drain deadline passed; cancelling the rest");
        shared.token.cancel();
        match shared.hosts.lock() {
            Ok(hosts) => {
                let mut tenants: Vec<String> = hosts.keys().cloned().collect();
                tenants.sort();
                for tenant in tenants {
                    if let Some(host) = hosts.get(&tenant) {
                        host.cancel();
                    }
                }
            }
            Err(error) => {
                lgwks_std::trace::debug!(?error, "serve: the host lock is poisoned at drain");
            }
        }
        owned.abort_all();
    }
    let drained = shared.drained.load(Ordering::SeqCst);
    let cancelled = shared.live.load(Ordering::SeqCst);
    (drained, cancelled)
}

// One thread's private submit runtime, built on first use.
//
// The request-key path is driven on this, never on the process driver (see
// `ingress`): a current-thread runtime polls a future on its owning thread,
// so a future that is not `Send` still drives, and no second `block_on`
// ever lands on the runtime `serve` already holds. The slot is built once
// per blocking-pool thread and dies with it; the submitted future never
// re-enters this slot, so the borrow below cannot recurse.
std::thread_local! {
    static SUBMIT_RUNTIME: std::cell::RefCell<Option<lgwks_deps::tokio::runtime::Runtime>> =
        const { std::cell::RefCell::new(None) };
}

/// Drive `future` — one `Host::submit` — to completion on this thread's
/// private runtime.
///
/// # Errors
///
/// Whatever the OS reports when the thread's runtime cannot be built. The
/// runtime is built once per thread; a thread that cannot build one answers
/// every request it holds with a refusal rather than a hang.
fn block_on_submit<Fut>(future: Fut) -> Result<Fut::Output, std::io::Error>
where
    Fut: Future,
{
    SUBMIT_RUNTIME.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let built = lgwks_deps::tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            *slot = Some(built);
        }
        let Some(runtime) = slot.as_ref() else {
            let refusal = Err(std::io::Error::other(
                "the submit runtime slot is empty after building it",
            ));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "serve: the submit runtime is refused");
            return refusal;
        };
        Ok(runtime.block_on(future))
    })
}

/// One request: tenant from the path, one run through the tenant's host.
async fn ingress(State(shared): State<Arc<Shared>>, request: HttpRequest) -> HttpResponse {
    let (parts, body) = request.into_parts();
    let Some((tenant, rest)) = tenant_of(parts.uri.path()) else {
        return answer(400, b"name a tenant: /t/{tenant}/...".to_vec());
    };
    if !crate::ecs::valid_tenant(tenant) {
        return answer(400, b"the tenant is not a valid name".to_vec());
    }
    let body = match read_body(body).await {
        Ok(body) => body,
        Err(over) => return *over,
    };
    let verb = parts.method.as_str().to_owned();
    let path = rest.to_owned();
    let key = idempotency_key(&parts.headers, tenant, &verb, &path, &body);
    let key = match RequestKey::new(&key) {
        Ok(key) => key,
        Err(cause) => {
            lgwks_std::trace::debug!(error = ?cause, "serve: the idempotency key is refused");
            return answer(400, b"the idempotency key is not a valid name".to_vec());
        }
    };
    let Ok(_permit) = shared.room.try_acquire() else {
        lgwks_std::trace::debug!("serve: past the in-flight ceiling; answering 503");
        return answer_retry(503, b"the serve is past its in-flight ceiling".to_vec());
    };
    let method = match verb.as_str() {
        "GET" => Some(RouteMethod::Get),
        "POST" => Some(RouteMethod::Post),
        _ => None,
    };
    let Some(route) = shared
        .routes
        .iter()
        .find(|route| Some(route.method) == method && route.path.as_str() == path.as_str())
    else {
        return answer(404, b"no route answers this method and path".to_vec());
    };
    if shared.token.is_cancelled() {
        return answer(503, b"the serve is stopping".to_vec());
    }
    let host = match host_for(&shared, tenant) {
        Ok(host) => host,
        Err(cause) => {
            lgwks_std::trace::debug!(error = ?cause, "serve: the tenant host is refused");
            return answer(500, b"the tenant host is refused".to_vec());
        }
    };
    let _live = LiveGuard::enter(&shared);
    let content_type = parts
        .headers
        .get(lgwks_deps::axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .map(str::to_owned);
    let handler = Arc::clone(&route.handler);
    let tenant_owned = tenant.to_owned();
    let body_vec = body.to_vec();
    let stopping = Arc::clone(&shared);
    // The request-key path runs on a blocking-pool thread on that thread's own
    // private current-thread runtime, and is awaited here through the pool
    // handle. Two facts force this shape. First, `Host::submit`'s future is
    // not `Send` — the run's scope and the store's awaiting write are not —
    // so awaiting it in this handler would not satisfy axum's `Handler`
    // bound, and the tree does not compile that way. Second, driving it with
    // the process driver's `block_on` from inside the pool nests a second
    // `block_on` on the runtime `serve` itself already holds in `block_on`,
    // which parks every request behind the first (the serve_load hang the
    // flood test guards). The submit path spawns nothing onto a runtime and
    // only needs a timer driver, which the private runtime provides; the job
    // is bounded by the pool, never queued without bound, and the permit
    // above already bounds in-flight runs.
    let driven = lgwks_std::task::try_spawn_blocking(move || {
        // Cancellation first: the serve may have stopped between the
        // pre-spawn check and this job's first poll. A stopped serve never
        // admits a run; the waiter below answers `503`, and a host stop past
        // the drain deadline cancels the run through the host's own token.
        if stopping.token.is_cancelled() {
            return SubmitOutcome::Stopping;
        }
        let inner_tenant = tenant_owned.clone();
        let run_task = match task("serve-route", move |_scope: Scope, input: ServeInput| {
            let handler = Arc::clone(&handler);
            let tenant_owned = inner_tenant.clone();
            let content = content_type.clone();
            async move {
                match handler(tenant_owned, content, input.body).await {
                    Ok(bytes) => Ok(ServeOutput {
                        status: 200,
                        body: bytes,
                    }),
                    Err((status, message)) => Ok(ServeOutput {
                        status,
                        body: message.into_bytes(),
                    }),
                }
            }
        }) {
            Ok(run_task) => run_task,
            Err(cause) => {
                return SubmitOutcome::Refused {
                    message: format!("the route task is refused: {cause}"),
                };
            }
        };
        let submitted =
            match block_on_submit(host.submit(&key, &run_task, ServeInput { body: body_vec })) {
                Ok(submitted) => submitted,
                Err(cause) => {
                    return SubmitOutcome::Refused {
                        message: format!("the submit runtime is refused: {cause}"),
                    };
                }
            };
        match submitted {
            Ok(Submission::Executed(report)) | Ok(Submission::Reattached(report)) => {
                match report.output() {
                    Some(answered) => SubmitOutcome::Answered {
                        status: answered.status,
                        body: answered.body.clone(),
                    },
                    None => match report.error() {
                        Some(cause) => SubmitOutcome::Refused {
                            message: format!("the route run failed: {cause}"),
                        },
                        None => SubmitOutcome::Refused {
                            message: String::from("the route run reported nothing"),
                        },
                    },
                }
            }
            Ok(Submission::InFlight(_)) => SubmitOutcome::InFlight,
            Err(RequestError::Conflict(conflict)) => SubmitOutcome::Conflict {
                message: conflict.to_string(),
            },
            Err(cause) => SubmitOutcome::Refused {
                message: format!("the submission is refused: {cause}"),
            },
        }
    });
    let outcome = match driven {
        Ok(driven) => driven.await,
        Err(cause) => {
            lgwks_std::trace::debug!(error = ?cause, "serve: the blocking pool is full");
            return answer_retry(503, b"the serve is past its in-flight ceiling".to_vec());
        }
    };
    match outcome {
        SubmitOutcome::Answered { status, body } => {
            if status != 200 {
                lgwks_std::trace::debug!(status, "serve: the route answered an error");
            }
            shared.drained.fetch_add(1, Ordering::SeqCst);
            answer(status, body)
        }
        SubmitOutcome::InFlight => {
            lgwks_std::trace::debug!("serve: the first run is still in flight; answering 503");
            answer_retry(
                503,
                b"the first run with this key is still in flight".to_vec(),
            )
        }
        SubmitOutcome::Conflict { message } => {
            lgwks_std::trace::debug!(
                conflict = message.as_str(),
                "serve: the idempotency key conflicts"
            );
            answer(422, message.into_bytes())
        }
        SubmitOutcome::Refused { message } => {
            lgwks_std::trace::debug!(
                refusal = message.as_str(),
                "serve: the submission is refused"
            );
            answer(500, message.into_bytes())
        }
        SubmitOutcome::Stopping => {
            lgwks_std::trace::debug!(
                "serve: the serve stopped before the run started; answering 503"
            );
            answer(503, b"the serve is stopping".to_vec())
        }
    }
}

/// [`answer`] with `Retry-After`, for a full house.
fn answer_retry(status: u16, body: Vec<u8>) -> HttpResponse {
    let mut response = answer(status, body);
    let value = match lgwks_deps::axum::http::HeaderValue::from_str(&RETRY_AFTER_SECS.to_string()) {
        Ok(value) => value,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "serve: the retry delay cannot be a header");
            return response;
        }
    };
    response
        .headers_mut()
        .insert(lgwks_deps::axum::http::header::RETRY_AFTER, value);
    response
}
