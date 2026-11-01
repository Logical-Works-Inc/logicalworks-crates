//! The driver's HTTP edge: axum in, the world's answer out (ADR 0006 R4).
//!
//! [`serve`] runs axum's router on the process driver. Every request is handed
//! to a [`RequestFeed`], the world's request system, which queues it on the
//! mailbox of the worker that owns its tenant and answers the reply the cell
//! settles; the edge writes that answer back as the response. axum carries
//! the connection, the HTTP parsing and the router, which rebuilding would not
//! improve; tenancy and the answer stay in the world.
//!
//! A request names its tenant in its path, `/t/{tenant}/{rest}`, and the world
//! sees `/{rest}`. What the edge answers itself, without the handler:
//!
//! | Status | When |
//! |---|---|
//! | `400` | the path names no tenant, or a tenant that is not a valid name |
//! | `413` | the body is over [`MAX_BODY_BYTES`] |
//! | `503` | the feed is closed, the feed holds 65,536 tenants and this is a new one, or the tenant's worker mailbox is full (with a `Retry-After` delay) |
//!
//! A full mailbox is backpressure, not loss: the edge submits without waiting
//! and answers `503` with a `Retry-After` delay when the tenant's worker is
//! behind, so a slow tenant sheds load instead of queueing without bound. The
//! edge awaits the reply on the driver, never inside a transition.
//!
//! While it serves, [`serve`] also drives the world on the calling thread: the
//! world ticks when one of its sources is woken (a handler holding a
//! [`WakeHandle`](crate::ecs::WakeHandle) wakes it) or an `Every` deadline or
//! retry backoff comes due on its clock, and not otherwise, so an idle world
//! costs nothing (ADR 0006 R5).
//!
//! [`serve`] returns when its [`EdgeStop`] is stopped: axum stops accepting and
//! drains its connections, the feed closes, and the world ticks once so its
//! systems see the closing state.

use core::fmt;
use std::future::IntoFuture as _;
use std::io;
use std::net::TcpListener;

use lgwks_deps::axum::{
    Router,
    body::{Body, Bytes, HttpBody as _, to_bytes},
    extract::{Request as HttpRequest, State},
    http::StatusCode,
    response::{IntoResponse, Response as HttpResponse},
};

use crate::ecs::{EcsBot, NextTick, RequestFeed, SubmitRefused};
use crate::error::BotError;
use crate::rt::runtime::driver;
use crate::rt::sync::CancellationToken;

/// The largest request body the edge reads, in bytes.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Why [`serve`] ended other than by its stop.
#[derive(Debug)]
#[non_exhaustive]
pub enum EdgeError {
    /// The listener or the driver failed.
    Io(io::Error),
    /// The world's closing tick failed.
    World(BotError),
}

impl fmt::Display for EdgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Io(ref cause) => write!(f, "the HTTP edge's listener failed: {cause}"),
            Self::World(ref cause) => write!(f, "the world behind the HTTP edge failed: {cause}"),
        }
    }
}

impl std::error::Error for EdgeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Io(ref cause) => Some(cause),
            Self::World(ref cause) => Some(cause),
        }
    }
}

/// The signal that ends a [`serve`]. Cloning is a handle to one signal.
#[derive(Clone, Debug, Default)]
pub struct EdgeStop {
    /// Cancelled once, by [`EdgeStop::stop`].
    token: CancellationToken,
}

impl EdgeStop {
    /// A signal not yet given.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// End the [`serve`] this signal was given to.
    pub fn stop(&self) {
        self.token.cancel();
    }
}

/// Serve `listener` into `world`'s request system `feed` until `stop` is
/// given, blocking the calling thread.
///
/// # Errors
///
/// [`EdgeError::Io`] when the listener cannot be served or the driver cannot
/// start, and [`EdgeError::World`] when the closing tick fails. The feed is
/// closed however it ends.
pub fn serve(
    world: &mut EcsBot,
    feed: &RequestFeed,
    listener: TcpListener,
    stop: &EdgeStop,
) -> Result<(), EdgeError> {
    let served = serve_http(world, feed, listener, stop);
    feed.close();
    let ticked = world.tick().map(drop).map_err(EdgeError::World);
    served.and(ticked)
}

/// axum on the process driver, and the world driven beside it on this thread,
/// until `stop` is given and every connection has drained.
fn serve_http(
    world: &mut EcsBot,
    feed: &RequestFeed,
    listener: TcpListener,
    stop: &EdgeStop,
) -> Result<(), EdgeError> {
    listener.set_nonblocking(true).map_err(EdgeError::Io)?;
    let handle = driver().map_err(EdgeError::Io)?;
    let router = Router::new().fallback(ingress).with_state(feed.clone());
    let token = &stop.token;
    handle
        .block_on(async move {
            let listener = lgwks_deps::tokio::net::TcpListener::from_std(listener)?;
            let http = lgwks_deps::axum::serve(listener, router)
                .with_graceful_shutdown(token.clone().cancelled_owned())
                .into_future();
            let (served, ()) = lgwks_deps::tokio::join!(http, drive(world, token));
            served
        })
        .map_err(EdgeError::Io)
}

/// Tick `world` whenever its next tick is due, until `stop` is cancelled.
///
/// Waits on the world's own clock for a deadline and on its wakes, so a
/// virtual clock drives it without real time passing. A tick that fails is
/// logged and the world driven on: one held transition does not stop the edge.
async fn drive(world: &mut EcsBot, stop: &CancellationToken) {
    loop {
        match world.next_tick() {
            NextTick::Now => {}
            NextTick::In(wait) => {
                lgwks_deps::tokio::select! {
                    () = stop.cancelled() => return,
                    () = world.clock().sleep(wait) => {}
                    () = world.woken() => {}
                }
            }
            NextTick::Idle => {
                lgwks_deps::tokio::select! {
                    () = stop.cancelled() => return,
                    () = world.woken() => {}
                }
            }
        }
        if stop.is_cancelled() {
            return;
        }
        if let Err(error) = world.tick_async().await {
            lgwks_std::trace::debug!(%error, "edge: a driven tick did not finish clean");
        }
    }
}

/// The one handler: every request is queued on its tenant's cell and the edge
/// awaits the reply. The await lives in the edge, never inside a transition.
async fn ingress(State(feed): State<RequestFeed>, request: HttpRequest) -> HttpResponse {
    let (parts, body) = request.into_parts();
    let Some((tenant, rest)) = tenant_of(parts.uri.path()) else {
        return answer(400, b"name a tenant: /t/{tenant}/...".to_vec());
    };
    // A body the framing says is empty is not read: most requests carry none.
    let body = if body.size_hint().exact() == Some(0) {
        Bytes::new()
    } else {
        match to_bytes(body, MAX_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(_over) => return answer(413, b"the body is over the edge's ceiling".to_vec()),
        }
    };
    let pending = match feed.try_submit(tenant, parts.method.as_str(), rest, &body) {
        Ok(pending) => pending,
        Err(SubmitRefused::Tenant) => {
            return answer(400, b"the tenant is not a valid name".to_vec());
        }
        Err(SubmitRefused::TooLarge { .. }) => {
            return answer(413, b"the message is over the host's ceiling".to_vec());
        }
        Err(SubmitRefused::Overloaded { retry_after_ms }) => {
            return answer_retry(
                503,
                b"the tenant is behind; retry the request".to_vec(),
                retry_after_ms,
            );
        }
        Err(SubmitRefused::Closed) => {
            return answer(503, b"the request feed is closed".to_vec());
        }
    };
    match pending.await {
        Ok(response) => answer(response.status(), response.into_body()),
        Err(error) => {
            lgwks_std::trace::debug!(%error, "edge: the reply was lost");
            answer(503, b"the reply was lost".to_vec())
        }
    }
}

/// `/t/{tenant}/{rest}` split into the tenant and `/{rest}`.
fn tenant_of(path: &str) -> Option<(&str, &str)> {
    let named = path.strip_prefix("/t/")?;
    match named.find('/') {
        Some(at) => Some(named.split_at(at)),
        None => Some((named, "/")),
    }
}

/// [`answer`] with `Retry-After`, for a full mailbox: whole seconds, rounded
/// up from the host's millisecond delay. A value the header cannot carry is
/// left off rather than failing the request.
fn answer_retry(status: u16, body: Vec<u8>, retry_after_ms: u64) -> HttpResponse {
    let mut response = answer(status, body);
    let secs = retry_after_ms.div_ceil(1_000).max(1);
    match lgwks_deps::axum::http::HeaderValue::from_str(&secs.to_string()) {
        Ok(value) => {
            response
                .headers_mut()
                .insert(lgwks_deps::axum::http::header::RETRY_AFTER, value);
        }
        Err(_) => {
            lgwks_std::trace::debug!(retry_after_ms, "edge: the retry delay cannot be a header");
        }
    }
    response
}

/// An HTTP response with `status` and `body`; a status HTTP does not define is
/// answered `500`.
fn answer(status: u16, body: Vec<u8>) -> HttpResponse {
    match StatusCode::from_u16(status) {
        Ok(status) => (status, Body::from(body)).into_response(),
        Err(_invalid) => {
            lgwks_std::trace::warn!(status, "edge: the world answered an undefined status");
            (StatusCode::INTERNAL_SERVER_ERROR, Body::from(body)).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tenant_of;

    #[test]
    fn a_tenant_path_splits_into_the_tenant_and_the_rest() {
        assert_eq!(tenant_of("/t/acme/count"), Some(("acme", "/count")));
        assert_eq!(tenant_of("/t/acme"), Some(("acme", "/")));
        assert_eq!(tenant_of("/count"), None);
    }
}

/// The full-mailbox journey over a real socket (#790): the mailbox is sized
/// below the public surface, so the journey lives beside the edge.
#[cfg(all(test, feature = "ephemeral"))]
mod backpressure {
    use std::future::{Future, ready};
    use std::io::{Read as _, Write as _};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    use super::{EdgeStop, serve};
    use crate::ecs::{Request, RequestFeed, Response};
    use crate::spec::EffectScope;
    use crate::{Auth, Bot, BotError, Cadence, Cap, GrantSet, Observe};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Clients parked at once: past 64 workers of 8, the most any host
    /// admits, so every mailbox fills however the host sized itself. Also the
    /// shed channel's bound: each client reports at most once, so no report
    /// ever waits for room.
    const PARKED: usize = 576;

    /// The per-worker bound the journey serves under.
    const MAILBOX_BOUND: usize = 8;

    /// How long the journey waits for the first shed request: a bound on a
    /// broken host, never the pace of a working one.
    const FIRST_SHED: Duration = Duration::from_secs(60);

    /// How the backpressure journey failed.
    #[derive(Debug)]
    enum JourneyFailed {
        /// The exchange with the edge failed on the socket.
        Exchange {
            /// Where the edge listened.
            addr: SocketAddr,
            /// What the socket said.
            cause: std::io::Error,
        },
        /// The edge answered something that is not an HTTP/1.1 response.
        NotHttp {
            /// What it answered.
            answer: String,
        },
        /// A request answered a status the journey does not allow.
        Status {
            /// The request's path.
            path: String,
            /// What it answered.
            status: u16,
        },
        /// A client never completed an exchange in its attempts.
        NeverConnected {
            /// The request's path.
            path: String,
            /// The last attempt's failure.
            last: Option<Box<JourneyFailed>>,
        },
        /// The test gate's lock is poisoned.
        Poisoned,
        /// A shed report could not reach the journey.
        ReportLost,
        /// A thread of the journey panicked.
        Panicked {
            /// Which thread.
            what: &'static str,
        },
        /// No full mailbox was answered `503`.
        NoShed,
        /// No `503` carried `Retry-After`.
        NoRetryAfter,
        /// The host did not serve again after the release.
        NotServed {
            /// What it answered.
            status: u16,
            /// The body it answered with.
            body: String,
        },
        /// The feed counted no refusal.
        NothingRefused {
            /// The feed's counts, rendered.
            counts: String,
        },
    }

    impl std::fmt::Display for JourneyFailed {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match *self {
                Self::Exchange { addr, ref cause } => {
                    write!(f, "the exchange with {addr} failed: {cause}")
                }
                Self::NotHttp { ref answer } => write!(f, "not an HTTP/1.1 answer: {answer:?}"),
                Self::Status { ref path, status } => write!(f, "{path} answered {status}"),
                Self::NeverConnected { ref path, ref last } => match *last {
                    Some(ref last) => write!(f, "{path} never completed an exchange: {last}"),
                    None => write!(f, "{path} never completed an exchange"),
                },
                Self::Poisoned => f.write_str("the test gate is poisoned"),
                Self::ReportLost => f.write_str("a shed report was lost"),
                Self::Panicked { what } => write!(f, "the {what} thread panicked"),
                Self::NoShed => f.write_str("no full mailbox answered 503"),
                Self::NoRetryAfter => f.write_str("no 503 carried retry-after"),
                Self::NotServed { status, ref body } => {
                    write!(f, "the host did not serve again: {status} {body:?}")
                }
                Self::NothingRefused { ref counts } => {
                    write!(f, "no refusal was counted: {counts}")
                }
            }
        }
    }

    impl std::error::Error for JourneyFailed {}

    /// Log `failure` and hand it back, so every failed journey leaves a record.
    fn refused<T>(failure: JourneyFailed) -> Result<T, JourneyFailed> {
        lgwks_std::trace::warn!(%failure, "edge backpressure: a journey failed");
        Err(failure)
    }

    /// An HTTP/1.1 answer split into its status, its header lines and its
    /// body; `None` when it is not one.
    fn split_answer(answer: &str) -> Option<(u16, &str, &str)> {
        let (status_line, rest) = answer.split_once("\r\n")?;
        let code = status_line.strip_prefix("HTTP/1.1 ")?.get(..3)?;
        let (headers, body) = rest.split_once("\r\n\r\n")?;
        Some((code.parse().ok()?, headers, body))
    }

    /// One `POST` on a fresh connection: its status, headers and body.
    fn call(
        addr: SocketAddr,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, String, String), JourneyFailed> {
        let request = [
            format!(
                "POST {path} HTTP/1.1\r\nhost: {addr}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes(),
            body.to_vec(),
        ]
        .concat();
        let mut answer = String::new();
        TcpStream::connect(addr)
            .and_then(|mut stream| {
                stream.write_all(&request)?;
                stream.read_to_string(&mut answer)
            })
            .map_err(|cause| JourneyFailed::Exchange { addr, cause })?;
        match split_answer(&answer) {
            Some((status, headers, body)) => Ok((status, headers.to_owned(), body.to_owned())),
            None => refused(JourneyFailed::NotHttp { answer }),
        }
    }

    /// The gate the parked handler waits on: closed until the journey opens it.
    #[derive(Debug, Default)]
    struct Gate {
        /// Whether the handler may return.
        open: Mutex<bool>,
        /// Rung on release.
        bell: Condvar,
    }

    impl Gate {
        /// Block until the gate opens.
        fn pass(&self) -> Response {
            let Ok(mut open) = self.open.lock() else {
                return Response::new(500, b"the test gate is poisoned".to_vec());
            };
            while !*open {
                open = match self.bell.wait(open) {
                    Ok(woke) => woke,
                    Err(_) => return Response::new(500, b"the test gate is poisoned".to_vec()),
                };
            }
            Response::new(200, b"released".to_vec())
        }

        /// Open the gate for every parked handler.
        fn release(&self) -> Result<(), JourneyFailed> {
            match self.open.lock() {
                Ok(mut open) => *open = true,
                Err(_) => return refused(JourneyFailed::Poisoned),
            }
            self.bell.notify_all();
            Ok(())
        }
    }

    /// A `Wake` source that never fires, so the world idles while the edge
    /// serves.
    struct Idle;

    impl Observe for Idle {
        type Output = u64;

        fn required_caps(&self) -> &[Cap] {
            &[]
        }

        fn poll(&self, _call: (Auth, ())) -> impl Future<Output = Result<u64, BotError>> {
            ready(Ok(0))
        }

        fn cadence(&self) -> Cadence {
            Cadence::Wake
        }

        fn domain_id(&self) -> &str {
            "edge-backpressure::idle"
        }
    }

    /// One parked client: retries a reset connection, which is load shedding
    /// one level down, and reports whether its answer was a `503` and whether
    /// that carried `Retry-After`.
    fn park_one(
        addr: SocketAddr,
        tenant: usize,
        shed: &SyncSender<bool>,
    ) -> Result<(), JourneyFailed> {
        let path = format!("/t/tenant-{tenant}/count");
        let mut last = None;
        for _ in 0..10_u32 {
            match call(addr, &path, b"park") {
                Ok((503, headers, _)) => {
                    let delayed = headers.to_lowercase().contains("retry-after:");
                    if let Err(lost) = shed.send(delayed) {
                        lgwks_std::trace::warn!(%lost, "edge backpressure: the journey stopped listening");
                        return refused(JourneyFailed::ReportLost);
                    }
                    return Ok(());
                }
                Ok((200, _, _)) => return Ok(()),
                Ok((status, _, _)) => return refused(JourneyFailed::Status { path, status }),
                Err(failure) => last = Some(Box::new(failure)),
            }
        }
        refused(JourneyFailed::NeverConnected { path, last })
    }

    /// A full mailbox is answered `503` with `Retry-After`, and the host
    /// serves again once the handler returns. Cites #790: the mailbox is the
    /// bound, and backpressure is an answer, not a queue without end.
    #[test]
    fn a_full_mailbox_is_answered_503_with_retry_after() -> TestResult {
        let gate = std::sync::Arc::new(Gate::default());
        let parked = std::sync::Arc::clone(&gate);
        let feed = RequestFeed::with_mailbox_bound(
            move |request: &Request<'_>, _count: &mut u64| match request.body() {
                b"park" => parked.pass(),
                _ => Response::new(200, b"served".to_vec()),
            },
            MAILBOX_BOUND,
        );
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let mut world = Bot::builder("edge-backpressure")
            .with_effects(EffectScope::ephemeral()?)
            .observe(Idle)
            .build(&GrantSet::empty())?;
        let stop = EdgeStop::new();
        let (shed, sheds) = mpsc::sync_channel::<bool>(PARKED);
        let ended = stop.clone();
        std::thread::scope(|scope| -> TestResult {
            let journey = scope.spawn(move || -> Result<(), JourneyFailed> {
                let outcome = std::thread::scope(|clients| -> Result<(), JourneyFailed> {
                    let mut parked = Vec::with_capacity(PARKED);
                    for tenant in 0..PARKED {
                        let shed = shed.clone();
                        parked.push(clients.spawn(move || park_one(addr, tenant, &shed)));
                    }
                    let first = sheds.recv_timeout(FIRST_SHED);
                    gate.release()?;
                    for client in parked {
                        match client.join() {
                            Ok(done) => done?,
                            Err(_panicked) => {
                                return refused(JourneyFailed::Panicked { what: "client" });
                            }
                        }
                    }
                    let mut delayed = match first {
                        Ok(delayed) => delayed,
                        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                            return refused(JourneyFailed::NoShed);
                        }
                    };
                    delayed |= sheds.try_iter().any(|carried| carried);
                    if !delayed {
                        return refused(JourneyFailed::NoRetryAfter);
                    }
                    let (status, _, body) = call(addr, "/t/after-release/count", b"")?;
                    if status != 200 || body != "served" {
                        return refused(JourneyFailed::NotServed { status, body });
                    }
                    Ok(())
                });
                ended.stop();
                outcome
            });
            serve(&mut world, &feed, listener, &stop)?;
            match journey.join() {
                Ok(outcome) => Ok(outcome?),
                Err(_panicked) => Ok(refused(JourneyFailed::Panicked { what: "journey" })?),
            }
        })?;
        let counts = feed.counts();
        if counts.refused == 0 {
            return Ok(refused(JourneyFailed::NothingRefused {
                counts: format!("{counts:?}"),
            })?);
        }
        Ok(())
    }
}
