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
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use lgwks_deps::axum::Router;

use crate::rt::edge::EdgeStop;
use crate::rt::sync::CancellationToken;
use crate::task::Host;

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
    /// The durable sink gate refused the start: the store directory cannot
    /// persist, so serving would fail one request at a time.
    StartGate {
        /// The composite sink that refused, naming the failed child in `reason`.
        sink: String,
        /// The child's name and the failure kind.
        reason: String,
    },
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
            Self::StartGate {
                ref sink,
                ref reason,
            } => write!(
                formatter,
                "lgwks_bot: the durable sink '{sink}' is not ready: {reason}"
            ),
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
            Self::StartGate { .. } => None,
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

mod ingress;
use ingress::ingress;

/// Shared serve state behind every request.
pub(crate) struct Shared {
    /// The routes in registration order.
    pub(crate) routes: Vec<TypedRoute>,
    /// At most `max_in_flight` permits; past it the edge answers `503`.
    pub(crate) room: lgwks_deps::tokio::sync::Semaphore,
    /// Cancelled on graceful stop; in-flight runs observe it.
    pub(crate) token: CancellationToken,
    /// One host per tenant, each over the same run store dir.
    pub(crate) hosts: std::sync::Mutex<HashMap<String, Host>>,
    /// Where every host's run store lives.
    pub(crate) store_dir: PathBuf,
    /// A per-serve scratch removed on drain, when the store is ephemeral.
    pub(crate) ephemeral_store: Option<PathBuf>,
    /// The per-host admission ceiling.
    pub(crate) max_in_flight: usize,
    /// Requests answered before the stop.
    pub(crate) drained: AtomicUsize,
    /// Requests still in flight, released by an RAII guard in `Drop`.
    pub(crate) live: AtomicUsize,
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

/// The start gate over the durable sink: refuse before the listener converts.
///
/// The store directory exists by now ([`std::fs::create_dir_all`] above), so a
/// refusal means it cannot persist — read-only remount, replaced data
/// directory, a squatter on the probe path — and serving would fail one
/// request at a time instead of refusing to start.
fn check_serve_store(dir: &Path) -> Result<(), crate::task::SinkRefusal> {
    let child: Arc<dyn crate::task::DurableSink> = Arc::new(crate::task::StoreSink::for_dir(dir));
    crate::task::CompositeSink::new("durable-sink", [child]).readiness()
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
    if let Err(refusal) = check_serve_store(options.store_dir.as_path()) {
        lgwks_std::trace::debug!(error = ?refusal, "serve: the durable sink gate refused the start");
        return Err(ServeError::StartGate {
            sink: refusal.sink().to_owned(),
            reason: refusal.reason().to_owned(),
        });
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
