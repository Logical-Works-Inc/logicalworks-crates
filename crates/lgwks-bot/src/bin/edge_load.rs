//! The HTTP edge under load (ADR 0006 R4): the world's edge, the app SDK,
//! `rt::serve` and a plain axum server answer the same per-tenant counter, and
//! one client drives any of them.
//!
//! ```text
//! edge-load world            serve the world's edge until stdin closes
//! edge-load axum             serve plain axum with a locked map until stdin closes
//! edge-load app              serve the counter as an app SDK app until stdin closes
//! edge-load app-ask          the app counter with one provider ask per post
//! edge-load app-keyed        the app counter behind a key per tenant, `key-{tenant}`
//! edge-load app-command      the app counter whose answer comes from a command
//! edge-load serve            serve the counter on rt::serve over a scratch run store
//! edge-load serve-durable DIR  the rt::serve counter over the durable run store in DIR
//! edge-load plan FILE        serve a model-written plan through App::from_plan
//! edge-load hit ADDR CONNS REQUESTS TENANTS
//! edge-load hit-keyed ADDR CONNS REQUESTS TENANTS
//! edge-load drive ADDR CONNS REQUESTS TENANTS READ_PERMILLE SEED
//! edge-load --rig [--short] [--db FILE] [--host LABEL] [--commit SHA] [--only SURFACE]
//! ```
//!
//! Every counter answers `POST /t/{tenant}/count` with the tenant's count after
//! adding one, and `GET` with the count as it stands, so a tenant's answers are
//! its own sequence whichever server holds them; a plan answers its fixed
//! status and body. Each prints `listening ADDR` on its first stdout line. The
//! client (`drive`) prints one line: throughput, p50, p95, p99, max and the
//! error count. `--rig` runs every surface on one workload and stores each
//! round as a row (`rig`).
//!
//! The world's edge is driven as a world (ADR 0006 R5): it samples the settled
//! count every second and reads the rejections the handler wakes it for, and
//! it prints both poll counts with its record when it stops.

use std::collections::HashMap;
use std::fmt;
use std::future::{Future, ready};
use std::io::{BufRead, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use lgwks_bot::app::{
    App, Effect, EffectResult, Method as AppMethod, OnInput, Principal, Provider, ReplyTag,
    RequestHeaders,
};
use lgwks_bot::rt::edge::{EdgeError, EdgeStop, MAX_BODY_BYTES, serve};
use lgwks_bot::rt::serve::{RouteMethod, ServeOptions, TypedRoute};
use lgwks_bot::rt::sync::CancellationToken;
use lgwks_bot::spec::EffectScope;
use lgwks_bot::{
    Auth, Bot, BotError, Cadence, Cap, GrantSet, Observe, RequestFeed, Response, WakeHandle,
};
use lgwks_deps::axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{Method, StatusCode},
    routing::post,
};

#[path = "edge_load/drive.rs"]
mod drive;
#[path = "../../examples/support/measure.rs"]
mod measure;
#[path = "edge_load/rig.rs"]
mod rig;
#[path = "edge_load/usage.rs"]
mod usage;

/// At most this many requests run at once on `rt::serve`; past it the edge
/// answers `503`. Above the rig's 1,024 connections, so a round measures the
/// work and not the ceiling.
const IN_FLIGHT: u64 = 4_096;

/// Why a run of this binary failed.
enum LoadError {
    /// The arguments did not name a mode this binary has.
    Usage,
    /// A socket or stdio failed.
    Io(std::io::Error),
    /// The world's edge failed.
    Edge(EdgeError),
    /// The world could not be built.
    World(String),
    /// The server or the driver answered something this binary cannot read.
    Malformed(&'static str),
    /// The seeded stream refused a draw.
    Draw(lgwks_std::seeded::DrawError),
    /// A full rig was asked of a debug build, whose numbers mean nothing.
    DebugBuild,
    /// The `sqlite3` program is missing or refused the rows.
    Store(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Usage => {
                f.write_str(
                    "usage: edge-load world | axum | app | app-ask | app-keyed | app-command | serve | serve-durable DIR | plan FILE | hit ADDR CONNS REQUESTS TENANTS | hit-keyed ADDR CONNS REQUESTS TENANTS | drive ADDR CONNS REQUESTS TENANTS READ_PERMILLE SEED | --rig [--short] [--db FILE] [--host LABEL] [--commit SHA] [--only SURFACE]",
                )
            }
            Self::Io(ref cause) => write!(f, "i/o: {cause}"),
            Self::Edge(ref cause) => write!(f, "edge: {cause}"),
            Self::World(ref cause) => write!(f, "world: {cause}"),
            Self::Malformed(what) => write!(f, "malformed answer: {what}"),
            Self::Draw(ref cause) => write!(f, "draw: {cause}"),
            Self::DebugBuild => f.write_str(
                "a full rig needs a release build: a debug build's numbers are refused; pass --short for a check run",
            ),
            Self::Store(ref cause) => write!(f, "store: {cause}"),
        }
    }
}

/// `main` prints its error through `Debug`, so the user reads the sentence.
impl fmt::Debug for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for LoadError {}

impl From<std::io::Error> for LoadError {
    fn from(cause: std::io::Error) -> Self {
        Self::Io(cause)
    }
}

impl From<lgwks_std::seeded::DrawError> for LoadError {
    fn from(cause: lgwks_std::seeded::DrawError) -> Self {
        Self::Draw(cause)
    }
}

/// Log `error` and hand it back, so every refusal leaves a record.
fn refused<T>(error: LoadError) -> Result<T, LoadError> {
    lgwks_std::trace::warn!(%error, "edge-load: refused");
    Err(error)
}

fn main() -> Result<(), LoadError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match *words.as_slice() {
        ["world"] => world(),
        ["axum"] => plain_axum(),
        ["app"] => app(),
        ["app-ask"] => app_ask(),
        ["app-keyed"] => app_keyed(),
        ["app-command"] => app_command(),
        ["serve"] => serve_counter(None),
        ["serve-durable", dir] => serve_counter(Some(dir)),
        ["plan", path] => plan(path),
        ["hit", addr, conns, requests, tenants] => drive::run(
            addr,
            drive::Mix::parse([conns, requests, tenants, "0", "0"], false)?,
        ),
        ["hit-keyed", addr, conns, requests, tenants] => drive::run(
            addr,
            drive::Mix::parse([conns, requests, tenants, "0", "0"], true)?,
        ),
        ["drive", addr, conns, requests, tenants, share, seed] => drive::run(
            addr,
            drive::Mix::parse([conns, requests, tenants, share, seed], false)?,
        ),
        ["--rig", ref rest @ ..] => rig::run(rest),
        _ => refused(LoadError::Usage),
    }
}

/// The counter as an app SDK app: the same cell host as the world's edge,
/// with the route table, typed body and effect runner in front of it. A body
/// on a count does not decode as `()` and is answered `400`.
fn app() -> Result<(), LoadError> {
    let counter =
        |count: &mut u64, (): ()| vec![Effect::reply(200, count.to_string().into_bytes())];
    serve_app(
        App::new("edge-load")
            .state::<u64>()
            .route(AppMethod::POST, "/count", move |count: &mut u64, (): ()| {
                *count = count.saturating_add(1);
                counter(count, ())
            })
            .route(AppMethod::GET, "/count", counter),
    )
}

/// The counter behind a key per tenant. A tenant's key is `key-{tenant}`, shown as a
/// bearer token or as `x-api-key`, and each answer names who asked.
fn app_keyed() -> Result<(), LoadError> {
    let counter = |count: &mut u64, who: &Principal, (): ()| {
        let page = format!("{}:{count}", who.as_str());
        vec![Effect::reply(200, page.into_bytes())]
    };
    serve_app(
        App::new("edge-load-keyed")
            .state::<u64>()
            .authenticate(|tenant: &str, headers: &RequestHeaders<'_>| {
                let key = match headers.bearer() {
                    Some(key) => Some(key),
                    None => headers.get("x-api-key"),
                };
                if key? == format!("key-{tenant}") {
                    Principal::new(tenant)
                } else {
                    None
                }
            })
            .route_as(
                AppMethod::POST,
                "/count",
                move |count: &mut u64, who: &Principal, (): ()| {
                    *count = count.saturating_add(1);
                    counter(count, who, ())
                },
            )
            .route_as(AppMethod::GET, "/count", counter),
    )
}

/// Answers every ask with the asking tenant's name, so the round trip
/// through the provider and back into the tenant's mailbox is what is timed.
struct EchoTenant;

impl Provider for EchoTenant {
    fn dispatch(&self, tenant: &str, effect: &Effect) -> Option<Vec<u8>> {
        match *effect {
            Effect::Ask { .. } => Some(tenant.as_bytes().to_vec()),
            _ => None,
        }
    }

    fn command(&self, tenant: &str, _: Option<&Principal>, _: &[u8]) -> Option<Vec<u8>> {
        Some(tenant.as_bytes().to_vec())
    }
}

/// One tenant's counts in `app-ask`: posts, and the asks answered back.
#[derive(Debug, Clone, Default)]
struct Asked {
    /// Posts counted.
    posts: u64,
    /// Ask answers the `on` handler folded.
    answers: u64,
}

/// The counter with one effect per post: each post asks the provider, whose
/// answer re-enters the tenant's mailbox and is folded by the `asked` handler.
/// `GET` answers `posts/answers`; the record line prints neither, the client
/// times the posts.
fn app_ask() -> Result<(), LoadError> {
    serve_app(
        App::new("edge-load-ask")
            .state::<Asked>()
            .grant(Cap::new("app.ask"))
            .provider(EchoTenant)
            .route(AppMethod::POST, "/count", |asked: &mut Asked, (): ()| {
                asked.posts = asked.posts.saturating_add(1);
                vec![
                    Effect::reply(200, asked.posts.to_string().into_bytes()),
                    Effect::Ask {
                        prompt: "count".into(),
                        reply_to: ReplyTag::new("asked"),
                    },
                ]
            })
            .route(AppMethod::GET, "/count", |asked: &mut Asked, (): ()| {
                let page = format!("{}/{}", asked.posts, asked.answers);
                vec![Effect::reply(200, page.into_bytes())]
            })
            .on(
                ReplyTag::new("asked"),
                |asked: &mut Asked, input: &OnInput| {
                    if let EffectResult::Value(_) = *input.result() {
                        asked.answers = asked.answers.saturating_add(1);
                    }
                    Vec::new()
                },
            ),
    )
}

/// The counter with its request held across a command: each post leaves the worker for
/// the command pool and comes back through the tenant's mailbox before it is answered,
/// so the round trip is what the client times. `GET` answers the count as it stands.
fn app_command() -> Result<(), LoadError> {
    serve_app(
        App::new("edge-load-command")
            .state::<u64>()
            .grant(Cap::new("app.command"))
            .provider(EchoTenant)
            .route(AppMethod::POST, "/count", |_: &mut u64, (): ()| {
                vec![Effect::Command {
                    body: Box::default(),
                    reply_to: ReplyTag::new("counted"),
                }]
            })
            .route(AppMethod::GET, "/count", |count: &mut u64, (): ()| {
                vec![Effect::reply(200, count.to_string().into_bytes())]
            })
            .on(
                ReplyTag::new("counted"),
                |count: &mut u64, input: &OnInput| match *input.result() {
                    EffectResult::Value(_) => {
                        *count = count.saturating_add(1);
                        vec![Effect::reply(200, count.to_string().into_bytes())]
                    }
                    _ => vec![Effect::reply(503, Vec::new())],
                },
            ),
    )
}

/// A plan a model wrote, read from `path` and served through the same check.
fn plan(path: &str) -> Result<(), LoadError> {
    // One byte past the 64 KiB plan ceiling, so an oversized file is refused
    // by `from_plan` with its typed error instead of read whole.
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(std::fs::File::open(path)?, 64 * 1_024 + 1),
        &mut bytes,
    )?;
    match App::<()>::from_plan(&bytes) {
        Ok(app) => serve_app(app),
        Err(cause) => refused(LoadError::World(cause.to_string())),
    }
}

/// Serve `app` on a fresh loopback port until stdin closes.
fn serve_app<S: Clone + Default + Send + 'static>(app: App<S>) -> Result<(), LoadError> {
    let listener = listen()?;
    let stop = EdgeStop::new();
    let watcher = stop.clone();
    until_stdin_then(
        move || watcher.stop(),
        || {
            app.serve(listener, &stop)
                .map_err(|cause| LoadError::World(cause.to_string()))
        },
    )
}

/// The counter on `rt::serve`: every request runs once through its tenant's
/// host, under the request's `idempotency-key`, over a scratch run store or,
/// when `store` names a directory, the durable store there.
fn serve_counter(store: Option<&str>) -> Result<(), LoadError> {
    let listener = listen()?;
    let options = match store {
        Some(dir) => ServeOptions::new(0, IN_FLIGHT, PathBuf::from(dir)),
        None => ServeOptions::new_ephemeral(0, IN_FLIGHT),
    }
    .map_err(|cause| LoadError::World(cause.to_string()))?;
    let counts: Counts = Arc::new(Mutex::new(HashMap::new()));
    let routes = [(RouteMethod::Post, true), (RouteMethod::Get, false)].map(|(method, adds)| {
        let counts = Arc::clone(&counts);
        TypedRoute::new(method, "/count", move |tenant: String, body: Vec<u8>| {
            ready(
                tally(&counts, tenant, &body, adds)
                    .map(|count| count.to_string().into_bytes())
                    .map_err(|why| (400, why.to_owned())),
            )
        })
    });
    let stop = EdgeStop::new();
    let watcher = stop.clone();
    until_stdin_then(
        move || watcher.stop(),
        || {
            lgwks_bot::rt::serve::serve(&options, Vec::from(routes), listener, &stop)
                .map_err(|cause| LoadError::World(cause.to_string()))?;
            Ok(())
        },
    )
}

/// Run `serve` on this thread while a scoped thread waits for stdin to close
/// and then calls `cancel`; what `serve` returned.
fn until_stdin_then<T>(cancel: impl FnOnce() + Send, serve: impl FnOnce() -> T) -> T {
    std::thread::scope(|scope| {
        scope.spawn(move || {
            until_stdin_closes();
            cancel();
        });
        serve()
    })
}

/// Bind a loopback port and announce it on stdout.
fn listen() -> Result<TcpListener, LoadError> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "listening {}", listener.local_addr()?)?;
    out.flush()?;
    Ok(listener)
}

/// Block until stdin reaches its end.
fn until_stdin_closes() {
    let mut line = String::new();
    let mut input = std::io::stdin().lock();
    while matches!(input.read_line(&mut line), Ok(read) if read > 0) {
        line.clear();
    }
}

/// How often the world samples the traffic it serves.
const SAMPLE: Duration = Duration::from_secs(1);

/// What the world reads about the traffic: the requests the feed settled,
/// sampled `Every` [`SAMPLE`], or the requests the handler rejected, read on
/// each `Wake` the handler gives. Its polls are counted, so a run shows how
/// often the driven world ticked under load (ADR 0006 R5).
struct Traffic {
    /// The feed whose settled count is sampled, once it exists.
    feed: Arc<OnceLock<RequestFeed>>,
    /// The rejections the handler counted; read instead of the feed when set.
    rejected: Option<Arc<AtomicU64>>,
    /// When the world polls this source.
    cadence: Cadence,
    /// Polls so far.
    polls: Arc<AtomicU64>,
}

impl Observe for Traffic {
    type Output = u64;

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn poll(&self, _call: (Auth, ())) -> impl Future<Output = Result<u64, BotError>> {
        self.polls.fetch_add(1, Ordering::Relaxed);
        let read = match (self.rejected.as_deref(), self.feed.get()) {
            (Some(rejected), _) => rejected.load(Ordering::Acquire),
            (None, Some(feed)) => feed.counts().settled,
            (None, None) => 0,
        };
        ready(Ok(read))
    }

    fn cadence(&self) -> Cadence {
        self.cadence
    }

    fn domain_id(&self) -> &str {
        "edge-load::traffic"
    }
}

/// The world's edge: each tenant's count is the world's per-tenant state, and
/// the world itself samples the traffic and reads every rejection.
fn world() -> Result<(), LoadError> {
    let listener = listen()?;
    let scope = EffectScope::ephemeral().map_err(|cause| LoadError::World(cause.to_string()))?;
    let served = Arc::new(OnceLock::new());
    let rejected = Arc::new(AtomicU64::new(0));
    let (samples, reads) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let mut bot = Bot::builder("edge-load")
        .with_effects(scope)
        .observe(Traffic {
            feed: Arc::clone(&served),
            rejected: None,
            cadence: Cadence::Every(SAMPLE),
            polls: Arc::clone(&samples),
        })
        .observe(Traffic {
            feed: Arc::clone(&served),
            rejected: Some(Arc::clone(&rejected)),
            cadence: Cadence::Wake,
            polls: Arc::clone(&reads),
        })
        .build(&GrantSet::empty())
        .map_err(|cause| LoadError::World(cause.to_string()))?;
    let Some(wake) = WakeHandle::for_source(&bot, 1) else {
        return refused(LoadError::World(String::from(
            "the rejection source has no wake",
        )));
    };
    let reject = move |status: u16, body: Vec<u8>| {
        rejected.fetch_add(1, Ordering::AcqRel);
        wake.wake();
        Response::new(status, body)
    };
    let feed = RequestFeed::new(move |request, count: &mut u64| {
        if request.path() != "/count" {
            let named = format!("{} has no {}", request.tenant(), request.path());
            return reject(404, named.into_bytes());
        }
        if !request.body().is_empty() {
            return reject(400, b"a count takes no body".to_vec());
        }
        if request.method() == "POST" {
            *count = count.saturating_add(1);
        }
        Response::new(200, count.to_string().into_bytes())
    });
    if served.set(feed.clone()).is_err() {
        return refused(LoadError::World(String::from("the feed was set twice")));
    }
    let stop = EdgeStop::new();
    let watcher = stop.clone();
    until_stdin_then(
        move || watcher.stop(),
        || serve(&mut bot, &feed, listener, &stop).map_err(LoadError::Edge),
    )?;
    // The world's own record, so a run can be checked against what the client sent.
    let counts = feed.counts();
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "world settled={} refused={} tenants={} samples={} rejection_reads={} probes={}",
        counts.settled,
        counts.refused,
        counts.tenants,
        samples.load(Ordering::Acquire),
        reads.load(Ordering::Acquire),
        counts.probes,
    )?;
    Ok(())
}

/// Every tenant's count, behind one lock.
type Counts = Arc<Mutex<HashMap<String, u64>>>;

/// The counts, whatever a panicking holder left behind.
fn held(counts: &Counts) -> MutexGuard<'_, HashMap<String, u64>> {
    match counts.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The tenant's count after a `POST` (`adds`) or as it stands; a body on
/// either is refused, as the world edge refuses it.
fn tally(counts: &Counts, tenant: String, body: &[u8], adds: bool) -> Result<u64, &'static str> {
    if !body.is_empty() {
        lgwks_std::trace::debug!(%tenant, bytes = body.len(), "edge-load: a count with a body is refused");
        return Err("a count takes no body");
    }
    let mut held = held(counts);
    if adds {
        let count = held.entry(tenant).or_default();
        *count = count.saturating_add(1);
        return Ok(*count);
    }
    Ok(match held.get(&tenant) {
        Some(&count) => count,
        None => 0,
    })
}

/// Plain axum: the same counter in a locked map, the way an axum service
/// keeps shared state, under the world edge's body ceiling and its refusal of
/// a body on a count, so both servers do the same work per request.
fn plain_axum() -> Result<(), LoadError> {
    let listener = listen()?;
    listener.set_nonblocking(true)?;
    let counts: Counts = Arc::new(Mutex::new(HashMap::new()));
    let router = Router::new()
        .route("/t/{tenant}/count", post(count).get(count))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(counts);
    let stop = CancellationToken::new();
    let watcher = stop.clone();
    until_stdin_then(
        move || watcher.cancel(),
        || {
            lgwks_bot::block_on(async move {
                let listener = lgwks_bot::rt::net::TcpListener::from_std(listener)?;
                lgwks_deps::axum::serve(listener, router)
                    .with_graceful_shutdown(stop.cancelled_owned())
                    .await
            })
            .map_err(LoadError::Io)
        },
    )
}

/// `POST` adds one to the tenant's count and answers it; `GET` answers it as
/// it stands; a body on either is refused `400`, as the world edge does.
async fn count(
    method: Method,
    State(counts): State<Counts>,
    Path(tenant): Path<String>,
    body: Bytes,
) -> (StatusCode, String) {
    match tally(&counts, tenant, &body, method == Method::POST) {
        Ok(count) => (StatusCode::OK, count.to_string()),
        Err(why) => (StatusCode::BAD_REQUEST, String::from(why)),
    }
}
