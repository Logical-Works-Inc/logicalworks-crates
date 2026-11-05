//! The reference route for `estate serve`: one typed draft over a file store.
//!
//! `POST /drafts` decodes its body as `DraftRequest` through
//! `lgwks_std::json`, answers `Draft`, and never reads the tenant from the
//! body: the tenant comes from the request path and is part of every store
//! key the route reaches.

use std::future::Future;
use std::pin::Pin;

use lgwks_std::json::{Deserialize, Serialize};

use lgwks_bot::rt::serve::openapi::{
    AuthPosture, DocStatus, RouteDescriptor, check_every_route_described, emit_openapi,
};
use lgwks_bot::rt::serve::{RouteMethod, TypedRoute};

/// What a draft request carries.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub struct DraftRequest {
    /// The working title.
    pub(crate) title: String,
    /// The body to shape into a draft.
    pub(crate) body: String,
}

impl DraftRequest {
    /// The headline carried into the draft.
    #[must_use]
    pub fn title(&self) -> &str {
        self.title.as_str()
    }

    /// The body to shape into a draft.
    #[must_use]
    pub fn body(&self) -> &str {
        self.body.as_str()
    }
}

/// What the reference route answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub struct Draft {
    /// The shaped draft.
    pub(crate) text: String,
}

impl Draft {
    /// The shaped draft.
    #[must_use]
    pub fn text(&self) -> &str {
        self.text.as_str()
    }
}

/// Shape `request` into its draft reply.
#[must_use]
pub fn draft_reply(request: DraftRequest) -> Draft {
    Draft {
        text: format!("draft for {}: {}", request.title(), request.body()),
    }
}

/// Decode `bytes` as [`DraftRequest`], or the `400` naming the field.
pub fn decode_request(bytes: &[u8]) -> Result<DraftRequest, (u16, String)> {
    let raw: &[u8] = if bytes.is_empty() { b"null" } else { bytes };
    match lgwks_std::json::from_slice::<DraftRequest>(raw) {
        Ok(request) => Ok(request),
        Err(error) => {
            let refusal = (
                400u16,
                format!("the body does not decode as DraftRequest: {error}"),
            );
            lgwks_std::trace::debug!("estate-serve: the body is refused");
            Err(refusal)
        }
    }
}

/// Encode `draft` as the response bytes, or the `500` for an unencodable value.
pub fn encode_draft(draft: &Draft) -> Result<Vec<u8>, (u16, String)> {
    lgwks_std::trace::debug!(text = draft.text(), "estate-serve: the draft is encoded");
    match lgwks_std::json::to_vec(draft) {
        Ok(bytes) => Ok(bytes),
        Err(error) => {
            let refusal = (500u16, format!("the draft could not be encoded: {error}"));
            lgwks_std::trace::debug!(error = ?refusal, "estate-serve: the draft is refused");
            Err(refusal)
        }
    }
}

/// The erased handler the edge runs: tenant in, JSON out.
type HandlerFuture = Pin<Box<dyn Future<Output = Result<Vec<u8>, (u16, String)>> + Send>>;

/// The erased handler the edge runs: tenant in, JSON out.
pub fn handler(tenant: String, body: Vec<u8>) -> HandlerFuture {
    Box::pin(async move {
        let request = decode_request(&body)?;
        let mut draft = draft_reply(request);
        draft.text = format!("[{}] {}", tenant, draft.text);
        encode_draft(&draft)
    })
}

/// The usage line every argument refusal names.
const USAGE: &str = "estate serve --store DIR [--port PORT] [--drain-grace-ms MS]";

/// Why `estate serve` could not start or stop cleanly.
#[derive(Debug)]
pub enum ServeRefError {
    /// The arguments do not read as `USAGE`; the text names the flag.
    Usage(String),
    /// A flag's value does not parse as the number it names.
    Number {
        /// The flag whose value was refused.
        flag: &'static str,
        /// The value as written.
        text: String,
        /// Why it does not parse.
        cause: std::num::ParseIntError,
    },
    /// The edge refused its options or failed while serving.
    Serve(lgwks_bot::rt::serve::ServeError),
    /// The serve route registry is undescribed or cannot become a document.
    Registry(String),
    /// The durable sink gate refused the start before anything was bound: the
    /// store directory cannot persist, so serving would fail one request at a
    /// time.
    StartGate {
        /// The composite sink that refused, naming the failed child in `reason`.
        sink: String,
        /// The child's name and the failure kind.
        reason: String,
    },
    /// The port could not be bound.
    Bind(std::io::Error),
    /// The stop signal could not be watched.
    Signal(std::io::Error),
}

impl std::fmt::Display for ServeRefError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Usage(ref what) => write!(formatter, "estate serve: {what}; {USAGE}"),
            Self::Number {
                flag,
                ref text,
                ref cause,
            } => write!(
                formatter,
                "estate serve: {flag} {text:?} is refused: {cause}"
            ),
            Self::Serve(ref cause) => write!(formatter, "estate serve: {cause}"),
            Self::Registry(ref reason) => write!(formatter, "estate serve: {reason}"),
            Self::StartGate {
                ref sink,
                ref reason,
            } => write!(
                formatter,
                "estate serve: the durable sink '{sink}' is not ready: {reason}"
            ),
            Self::Bind(ref cause) => write!(formatter, "estate serve could not bind: {cause}"),
            Self::Signal(ref cause) => {
                write!(
                    formatter,
                    "estate serve could not watch its stop signal: {cause}"
                )
            }
        }
    }
}

impl std::error::Error for ServeRefError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Usage(_) => None,
            Self::Number { ref cause, .. } => Some(cause),
            Self::Serve(ref cause) => Some(cause),
            Self::Registry(_) | Self::StartGate { .. } => None,
            Self::Bind(ref cause) | Self::Signal(ref cause) => Some(cause),
        }
    }
}

/// Refuse with `error`, logged as every refusal is.
fn refused<T>(error: ServeRefError) -> Result<T, ServeRefError> {
    lgwks_std::trace::debug!(error = ?error, "estate-serve: refused");
    Err(error)
}

/// The value after `flag` at `index`, or the usage refusal naming it.
fn flag_value<'args>(
    args: &'args [String],
    index: usize,
    flag: &str,
) -> Result<&'args String, ServeRefError> {
    match args.get(index) {
        Some(value) => Ok(value),
        None => refused(ServeRefError::Usage(format!("{flag} names no value"))),
    }
}

/// Parse `text` as the number `flag` names.
fn number<N: std::str::FromStr<Err = std::num::ParseIntError>>(
    flag: &'static str,
    text: &str,
) -> Result<N, ServeRefError> {
    match text.trim().parse() {
        Ok(value) => Ok(value),
        Err(cause) => refused(ServeRefError::Number {
            flag,
            text: text.to_owned(),
            cause,
        }),
    }
}

/// Fail fast before binding: the library `serve` consults the same gate, but a
/// refused start must not claim a port it will never serve.
///
/// # Errors
///
/// [`ServeRefError::StartGate`] naming the composite sink and the failed child
/// when the store directory cannot persist.
fn check_store_gate(store_dir: &std::path::Path) -> Result<(), ServeRefError> {
    let child: std::sync::Arc<dyn lgwks_bot::task::DurableSink> =
        std::sync::Arc::new(lgwks_bot::task::StoreSink::for_dir(store_dir));
    if let Err(refusal) = lgwks_bot::task::CompositeSink::new("durable-sink", [child]).readiness() {
        lgwks_std::trace::debug!(
            error = ?refusal,
            probe = lgwks_bot::task::START_PROBE_FILE,
            "estate-serve: the durable sink gate refused the start"
        );
        return refused(ServeRefError::StartGate {
            sink: refusal.sink().to_owned(),
            reason: refusal.reason().to_owned(),
        });
    }
    Ok(())
}

/// Block until SIGINT, SIGTERM or `done`, then give `stop`.
async fn watch_stop(
    stop: lgwks_bot::rt::edge::EdgeStop,
    done: lgwks_bot::rt::sync::CancellationToken,
) -> Result<(), ServeRefError> {
    #[cfg(unix)]
    let mut terminate =
        lgwks_bot::rt::signal::unix::signal(lgwks_bot::rt::signal::unix::SignalKind::terminate())
            .map_err(ServeRefError::Signal)?;
    #[cfg(unix)]
    let terminated = terminate.recv();
    #[cfg(not(unix))]
    let terminated = std::future::pending::<Option<()>>();
    lgwks_deps::tokio::select! {
        interrupted = lgwks_bot::rt::signal::ctrl_c() => interrupted.map_err(ServeRefError::Signal)?,
        _ = terminated => {}
        () = done.cancelled() => {}
    }
    stop.stop();
    Ok(())
}

/// Map a registry refusal into the serve error, logged as every refusal is.
fn registry_refusal(error: impl std::fmt::Display) -> ServeRefError {
    let refusal = ServeRefError::Registry(format!("the serve registry is refused: {error}"));
    lgwks_std::trace::debug!(error = ?refusal, "estate-serve: the registry is refused");
    refusal
}

/// The reference `POST /drafts` route, described for the contract.
fn drafts_route() -> Result<TypedRoute, ServeRefError> {
    let descriptor = RouteDescriptor::new(
        RouteMethod::Post,
        "/drafts",
        "estate::serve::drafts",
        "Shape a draft request into its draft reply.",
        AuthPosture::TenantEdge,
        DocStatus::Stub,
    )
    .map_err(registry_refusal)?;
    Ok(TypedRoute::new(RouteMethod::Post, "/drafts", handler).describe(descriptor))
}

/// The `GET /openapi.json` route answering `document`.
fn openapi_route(document: Vec<u8>) -> Result<TypedRoute, ServeRefError> {
    let descriptor = RouteDescriptor::new(
        RouteMethod::Get,
        "/openapi.json",
        "estate::serve::openapi",
        "Render the serve route registry as an OpenAPI 3.1 document.",
        AuthPosture::TenantEdge,
        DocStatus::Stub,
    )
    .map_err(registry_refusal)?;
    Ok(TypedRoute::new(
        RouteMethod::Get,
        "/openapi.json",
        move |_tenant: String, _body: Vec<u8>| {
            let document = document.clone();
            async move { Ok(document) }
        },
    )
    .describe(descriptor))
}

/// Serve the reference route over the run store at `--store DIR`.
///
/// Reads `--store DIR`, `--port PORT` (or `PORT` from the environment) and
/// `--drain-grace-ms MS`, serves `POST /drafts` with tenant isolation and
/// idempotency until SIGINT or SIGTERM, then drains gracefully and prints
/// what drained and what was cancelled. Called by the shipped `estate` bin.
pub fn run(args: &[String]) -> Result<(), ServeRefError> {
    let mut store: Option<&String> = None;
    let mut port_flag: Option<&String> = None;
    let mut grace_flag: Option<&String> = None;
    let mut index = 0;
    while let Some(flag) = args.get(index) {
        let value = flag_value(args, index.saturating_add(1), flag)?;
        match flag.as_str() {
            "--store" => store = Some(value),
            "--port" => port_flag = Some(value),
            "--drain-grace-ms" => grace_flag = Some(value),
            other => return refused(ServeRefError::Usage(format!("{other} is not a serve flag"))),
        }
        index = index.saturating_add(2);
    }
    let Some(store) = store else {
        return refused(ServeRefError::Usage(String::from(
            "--store DIR is required",
        )));
    };
    let store_dir = std::path::PathBuf::from(store);
    check_store_gate(store_dir.as_path())?;
    let options = match port_flag {
        Some(text) => {
            lgwks_bot::rt::serve::ServeOptions::new(number("--port", text)?, 1_000, store_dir)
        }
        None => lgwks_bot::rt::serve::ServeOptions::from_env("PORT", 1_000, store_dir),
    }
    .map_err(ServeRefError::Serve)?;
    let options = match grace_flag {
        Some(text) => options.with_drain_grace(std::time::Duration::from_millis(number(
            "--drain-grace-ms",
            text,
        )?)),
        None => options,
    };
    let port = options.port();
    let drafts = drafts_route()?;
    let shell = openapi_route(b"{}".to_vec())?;
    let described = std::vec![drafts.clone(), shell];
    check_every_route_described(described.as_slice()).map_err(registry_refusal)?;
    let document = emit_openapi(described.as_slice()).map_err(registry_refusal)?;
    let bytes = lgwks_std::json::to_vec(&document).map_err(|error| {
        ServeRefError::Registry(format!("the contract does not encode: {error}"))
    })?;
    let openapi = openapi_route(bytes)?;
    let routes = std::vec![drafts, openapi];
    let listener =
        std::net::TcpListener::bind(format!("127.0.0.1:{port}")).map_err(ServeRefError::Bind)?;
    say(&format!("serving drafts on 127.0.0.1:{port} store={store}"));
    let stop = lgwks_bot::rt::edge::EdgeStop::new();
    let done = lgwks_bot::rt::sync::CancellationToken::new();
    let report: lgwks_bot::rt::serve::ServeReport = std::thread::scope(|scope| {
        let watcher = scope.spawn(|| lgwks_bot::block_on(watch_stop(stop.clone(), done.clone())));
        let served = lgwks_bot::rt::serve::serve(&options, routes, listener, &stop);
        done.cancel();
        match watcher.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                lgwks_std::trace::debug!(error = ?error, "estate-serve: the stop watcher failed")
            }
            Err(_) => lgwks_std::trace::debug!("estate-serve: the stop watcher panicked"),
        }
        served
    })
    .map_err(ServeRefError::Serve)?;
    let drained = report.drained();
    let cancelled = report.cancelled();
    let served_port = report.port();
    say(&format!(
        "stopped drafts on 127.0.0.1:{served_port}: drained={drained} cancelled={cancelled}"
    ));
    Ok(())
}

/// One line to stdout; a closed stdout is logged, never fatal.
fn say(line: &str) {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
        lgwks_std::trace::debug!("estate-serve: a report line could not be written");
    }
}
