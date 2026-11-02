//! Model-written plans over the tenant HTTP edge (`estate apps`, #786).
//!
//! Boots the edge on 127.0.0.1:0, posts a refused plan, reads the named
//! refusal, posts the repaired plan, runs it, and asserts each line it
//! prints. Every number below is asserted: the first deviation records a
//! mismatch and the exit is non-zero, so a clean exit proves the evening.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;

use lgwks_bot::rt::apps::{AppStore, HeldGrants, OpRegistry, PlanLimits, apps_routes};
use lgwks_bot::rt::edge::EdgeStop;
use lgwks_bot::script::{FlowError, Scope};

/// A plan naming an operation no registry holds.
pub(crate) const REFUSED: &str = "flow app():\n    let out = graph:\n        node fetch:\n            \"fetching\"\n        node steal needs fetch:\n            \"stealing\"\n";

/// The same plan repaired: only registered operations remain.
pub(crate) const REPAIRED: &str = "flow app():\n    let out = graph:\n        node fetch:\n            \"fetching\"\n        node render needs fetch:\n            \"rendering\"\n";

/// Fetch and report the input.
async fn fetch_op(_scope: Scope, input: Vec<u8>) -> Result<Vec<u8>, FlowError> {
    let mut out = b"fetched:".to_vec();
    out.extend_from_slice(input.as_slice());
    Ok(out)
}

/// Render the input reversed.
async fn render_op(_scope: Scope, input: Vec<u8>) -> Result<Vec<u8>, FlowError> {
    let mut out = b"rendered:".to_vec();
    let mut tail = input;
    tail.reverse();
    out.extend_from_slice(tail.as_slice());
    Ok(out)
}

/// Why `estate apps` could not run its evening.
#[derive(Debug)]
pub enum AppsError {
    /// The arguments do not read as `estate apps [--store DIR]`.
    Usage(String),
    /// The socket failed.
    Exchange(String),
    /// The edge answered other than the journey requires.
    Answer {
        /// What the journey sent.
        request: String,
        /// What the journey requires.
        wanted: String,
        /// What the edge answered.
        found: String,
    },
    /// The edge refused to start or serve.
    Serve(String),
    /// The store directory or the registry is refused.
    Setup(String),
}

impl std::fmt::Display for AppsError {
    /// What broke, and what the journey requires.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Usage(ref what) => {
                write!(formatter, "estate apps: {what}; estate apps [--store DIR]")
            }
            Self::Exchange(ref cause) => {
                write!(formatter, "estate apps: the exchange failed: {cause}")
            }
            Self::Answer {
                ref request,
                ref wanted,
                ref found,
            } => write!(formatter, "{request} answered {found:?}, not {wanted:?}"),
            Self::Serve(ref cause) => write!(formatter, "estate apps: the edge failed: {cause}"),
            Self::Setup(ref cause) => write!(formatter, "estate apps: the setup failed: {cause}"),
        }
    }
}

impl std::error::Error for AppsError {}

/// Log `failure` and hand it back, so every broken journey leaves a record.
pub(crate) fn refused<T>(failure: AppsError) -> Result<T, AppsError> {
    lgwks_std::trace::debug!(error = ?failure, "estate-apps: a journey failed");
    Err(failure)
}

/// The demo registry: `fetch` and `render`, needing nothing.
fn demo() -> Result<OpRegistry, AppsError> {
    let registry = OpRegistry::new()
        .operation("fetch", &[], fetch_op)
        .map_err(|cause| AppsError::Setup(format!("the demo registry is refused: {cause}")))?;
    let registry = registry
        .operation("render", &[], render_op)
        .map_err(|cause| AppsError::Setup(format!("the demo registry is refused: {cause}")))?;
    Ok(registry)
}

/// One `POST` on a fresh connection: the status and the body.
fn exchange(
    addr: SocketAddr,
    path: &str,
    body: &[u8],
    content_type: Option<&str>,
) -> Result<(u16, String), AppsError> {
    let mut head = format!(
        "POST {path} HTTP/1.1\r\nhost: {addr}\r\ncontent-length: {}\r\n",
        body.len()
    );
    if let Some(mime) = content_type {
        head.push_str("content-type: ");
        head.push_str(mime);
        head.push_str("\r\n");
    }
    head.push_str("connection: close\r\n\r\n");
    let mut raw = Vec::new();
    TcpStream::connect(addr)
        .and_then(|mut stream| {
            stream.write_all(head.as_bytes())?;
            stream.write_all(body)?;
            stream.read_to_end(&mut raw)
        })
        .map_err(|cause| AppsError::Exchange(format!("the exchange failed: {cause}")))?;
    let text = String::from_utf8_lossy(raw.as_slice()).into_owned();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| AppsError::Exchange(format!("not an HTTP answer: {text:?}")))?;
    let code: u16 = head
        .strip_prefix("HTTP/1.1 ")
        .and_then(|status| status.get(..3))
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| AppsError::Exchange(format!("not an HTTP answer: {text:?}")))?;
    Ok((code, body.to_owned()))
}

/// Require `path` with `body` to answer `status` holding `wanted`.
fn answers(
    addr: SocketAddr,
    path: &str,
    body: &[u8],
    content_type: Option<&str>,
    status: u16,
    wanted: &str,
) -> Result<String, AppsError> {
    let (found, answered) = exchange(addr, path, body, content_type)?;
    if found != status || !answered.contains(wanted) {
        return refused(AppsError::Answer {
            request: format!("POST {path}"),
            wanted: format!("{status} holding {wanted:?}"),
            found: format!("{found} {answered:?}"),
        });
    }
    Ok(answered)
}

/// The app id a verdict answers.
fn app_id_of(body: &str) -> Result<String, AppsError> {
    let key = "\"app\":\"";
    let start = body
        .find(key)
        .ok_or_else(|| AppsError::Exchange(format!("no app id in {body:?}")))?;
    let rest = &body[start.saturating_add(key.len())..];
    let end = rest
        .find('"')
        .ok_or_else(|| AppsError::Exchange(format!("unterminated app id in {body:?}")))?;
    Ok(rest[..end].to_owned())
}

/// The evening: refuse, repair, run, and assert every printed line; answers
/// what the edge said, for the offline half to match.
fn journey(addr: SocketAddr) -> Result<super::apps_offline::EdgeAnswers, AppsError> {
    let reported = answers(
        addr,
        "/t/acme/apps",
        REFUSED.as_bytes(),
        None,
        422,
        "unknown-operation",
    )?;
    if !reported.contains("steal") {
        return refused(AppsError::Answer {
            request: String::from("POST /t/acme/apps"),
            wanted: String::from("422 naming steal"),
            found: reported,
        });
    }
    say("refused: the plan names the unknown operation steal")?;
    let admitted = answers(
        addr,
        "/t/acme/apps",
        REPAIRED.as_bytes(),
        None,
        200,
        "\"app\"",
    )?;
    let id = app_id_of(admitted.as_str())?;
    say(format!("admitted: app {id}").as_str())?;
    let asked = format!("{{\"app\":\"{id}\"}}");
    let run = answers(
        addr,
        "/t/acme/apps/run",
        asked.as_bytes(),
        None,
        200,
        "fetched:",
    )?;
    let fetch = run
        .find("fetched:")
        .ok_or_else(|| AppsError::Exchange(format!("no fetch line in {run:?}")))?;
    let render = run
        .find("rendered:")
        .ok_or_else(|| AppsError::Exchange(format!("no render line in {run:?}")))?;
    if render < fetch {
        return refused(AppsError::Answer {
            request: String::from("POST /t/acme/apps/run"),
            wanted: String::from("200 printing fetch before render"),
            found: run,
        });
    }
    say("ran: the plan printed fetch then render")?;
    Ok(super::apps_offline::EdgeAnswers {
        refused: reported,
        id,
        run,
    })
}

/// One line to stdout; a closed stdout fails the journey.
pub(crate) fn say(line: &str) -> Result<(), AppsError> {
    let mut out = std::io::stdout().lock();
    out.write_all(line.as_bytes())
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush())
        .map_err(|cause| AppsError::Exchange(format!("a report line failed: {cause}")))
}

/// Serve admitted plans from `store_dir` on 127.0.0.1:0 and run the evening.
pub fn run(args: &[String]) -> Result<(), AppsError> {
    let mut store_arg: Option<&String> = None;
    let mut index = 0;
    while let Some(flag) = args.get(index) {
        match flag.as_str() {
            "--store" => {
                let value = args
                    .get(index.saturating_add(1))
                    .ok_or_else(|| AppsError::Usage(String::from("--store names no value")))?;
                store_arg = Some(value);
            }
            other => return refused(AppsError::Usage(format!("{other} is not an apps flag"))),
        }
        index = index.saturating_add(2);
    }
    let store_dir = match store_arg {
        Some(dir) => std::path::PathBuf::from(dir),
        None => {
            let unique = lgwks_std::random::bytes::<8>()
                .map_err(|cause| AppsError::Setup(format!("the scratch name failed: {cause}")))?;
            let hex: String = unique.iter().map(|byte| format!("{byte:02x}")).collect();
            std::env::temp_dir().join(format!("lgwks-estate-apps-{hex}"))
        }
    };
    std::fs::create_dir_all(store_dir.as_path())
        .map_err(|cause| AppsError::Setup(format!("the store dir failed: {cause}")))?;
    let apps = Arc::new(AppStore::new(PlanLimits::default()));
    let registry = Arc::new(demo()?);
    let held: HeldGrants = Arc::new(|_: &str| Vec::new());
    let routes = apps_routes(Arc::clone(&apps), Arc::clone(&registry), held);
    let options = lgwks_bot::rt::serve::ServeOptions::new(0, 1_000, store_dir.clone())
        .map_err(|cause| AppsError::Serve(format!("the serve options are refused: {cause}")))?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|cause| AppsError::Serve(format!("the listener failed: {cause}")))?;
    let addr = listener
        .local_addr()
        .map_err(|cause| AppsError::Serve(format!("the address failed: {cause}")))?;
    say(format!("serving apps on {addr} store={}", store_dir.display()).as_str())?;
    let stop = EdgeStop::new();
    let ending = stop.clone();
    std::thread::scope(|scope| {
        let served = scope.spawn(move || {
            lgwks_bot::rt::serve::serve(&options, routes, listener, &ending)
                .map_err(|cause| format!("the edge failed: {cause}"))
        });
        let outcome = journey(addr);
        stop.stop();
        match served.join() {
            Ok(joined) => {
                let report = joined.map_err(AppsError::Serve)?;
                say(format!(
                    "stopped apps on {addr}: drained={} cancelled={}",
                    report.drained(),
                    report.cancelled()
                )
                .as_str())?;
                super::apps_offline::check(&apps, &registry, &outcome?)
            }
            Err(_) => refused(AppsError::Serve(String::from("the serve thread panicked"))),
        }
    })?;
    if store_arg.is_none() {
        drop(std::fs::remove_dir_all(store_dir.as_path()));
    }
    Ok(())
}
