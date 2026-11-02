//! Load generator for the `serve` edge and a plain axum handler (#782).
//!
//! Serves one shaped route on both stacks over loopback, sends one shaped
//! load at each, and prints req/s, p50/p95/p99, error rate and peak RSS for
//! each, so the edge's overhead reads as numbers rather than adjectives.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lgwks_deps::axum::{
    Router,
    body::Body,
    extract::{Request as HttpRequest, State},
    http::StatusCode,
    response::{IntoResponse, Response as HttpResponse},
};

use lgwks_bot::rt::edge::EdgeStop;
use lgwks_bot::rt::serve::{RouteMethod, ServeOptions, TypedRoute};

/// Requests per stack.
const REQUESTS: usize = 1_024;
/// Requests per client thread.
const PER_CLIENT: usize = 32;
/// Client threads.
const CLIENTS: usize = 32;
/// Tenants round-robined through the load.
const TENANTS: [&str; 4] = ["acme", "other", "third", "fourth"];
/// Body every request posts.
const LOAD_BODY: &[u8] = b"{\"n\":1}";

/// One stack's load numbers.
struct LoadNumbers {
    /// Requests answered 200.
    ok: usize,
    /// Requests answered otherwise, or never answered.
    errors: usize,
    /// Requests per second over the whole burst.
    per_second: f64,
    /// Median latency, in microseconds.
    middle: u64,
    /// 95th percentile latency, in microseconds.
    high: u64,
    /// 99th percentile latency, in microseconds.
    tail: u64,
    /// The process peak resident set after the burst, in bytes.
    peak_rss: u64,
}

/// One shaped HTTP answer.
fn split_answer(answer: &str) -> Option<(u16, String)> {
    let (status_line, rest) = answer.split_once("\r\n")?;
    let code = status_line.strip_prefix("HTTP/1.1 ")?.get(..3)?;
    let (_, body) = rest.split_once("\r\n\r\n")?;
    Some((code.parse().ok()?, body.to_owned()))
}

/// One `POST` on a fresh connection, timed in microseconds.
fn timed_post(addr: SocketAddr, tenant: &str, key: &str) -> Result<(u16, u64), String> {
    let target = format!("/t/{tenant}/drafts");
    let head = format!(
        "POST {target} HTTP/1.1\r\nhost: {addr}\r\ncontent-length: {}\r\nconnection: close\r\nidempotency-key: {key}\r\n\r\n",
        LOAD_BODY.len()
    );
    let start = Instant::now();
    let mut answer = String::new();
    TcpStream::connect(addr)
        .and_then(|mut stream| {
            // Bounded, so a stalled server fails the exchange instead of
            // parking the load client forever with no report.
            let bound = Duration::from_secs(20);
            stream.set_read_timeout(Some(bound))?;
            stream.set_write_timeout(Some(bound))?;
            stream.write_all(head.as_bytes())?;
            stream.write_all(LOAD_BODY)?;
            stream.read_to_string(&mut answer)
        })
        .map_err(|cause| format!("the exchange failed: {cause}"))?;
    let micros = start.elapsed().as_micros();
    let elapsed =
        u64::try_from(micros).map_err(|cause| format!("the latency does not fit u64: {cause}"))?;
    let (status, _) =
        split_answer(&answer).ok_or_else(|| format!("not an HTTP answer: {answer:?}"))?;
    Ok((status, elapsed))
}

/// The `p`th percentile of sorted `latencies`, or nothing when empty.
fn percentile(sorted: &[u64], percent: usize) -> Option<u64> {
    let last = sorted.len().checked_sub(1)?;
    let rank = last.checked_mul(percent)?.checked_div(100)?;
    sorted.get(rank).copied()
}

/// The process peak resident set, in bytes.
///
/// Reads every process's resident set from `ps` and keeps the largest one
/// sharing this binary's name: with one load running at a time, that row is
/// this process. A command name longer than the platform's 15 bytes would
/// collide with its own prefix, so this binary's name stays short. The match
/// is an equality first and a suffix second, because `comm` may carry the
/// name with its own decorations on some targets.
fn peak_rss_bytes() -> Result<u64, String> {
    let me = std::env::args()
        .next()
        .ok_or_else(|| String::from("argv has no entry"))?;
    let name = std::path::Path::new(me.as_str())
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| String::from("argv has no file name"))?;
    let output = std::process::Command::new("ps")
        .arg("-Ao")
        .arg("rss=,comm=")
        .output()
        .map_err(|cause| format!("ps did not run: {cause}"))?;
    let text = String::from_utf8(output.stdout)
        .map_err(|cause| format!("ps did not answer text: {cause}"))?;
    let mut peak = 0_u64;
    let mut seen = false;
    let mut rows = 0_usize;
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let (Some(rss_text), Some(comm)) = (words.next(), words.next()) else {
            continue;
        };
        rows = rows.saturating_add(1);
        if comm != name && !comm.ends_with(name) {
            continue;
        }
        seen = true;
        let kilobytes: u64 = rss_text
            .parse()
            .map_err(|cause| format!("ps answered {line:?}: {cause}"))?;
        let bytes = kilobytes
            .checked_mul(1_024)
            .ok_or_else(|| String::from("the resident set does not fit u64"))?;
        if bytes > peak {
            peak = bytes;
        }
    }
    if seen {
        Ok(peak)
    } else {
        Err(format!("ps answered {rows} rows and none for {name:?}"))
    }
}

/// Send the shaped load at `addr`, keyed `prefix`.
fn load(addr: SocketAddr, prefix: &str) -> Result<LoadNumbers, String> {
    if CLIENTS.checked_mul(PER_CLIENT) != Some(REQUESTS) {
        lgwks_std::trace::debug!("serve-load: the load does not split over its clients");
        return Err(String::from("the load does not split over its clients"));
    }
    let start = Instant::now();
    let outcomes = std::thread::scope(|scope| {
        let mut clients = Vec::new();
        for client in 0..CLIENTS {
            clients.push(scope.spawn(move || {
                let mut tenants = TENANTS.iter().cycle();
                let mut latencies = Vec::with_capacity(PER_CLIENT);
                let mut ok = 0_usize;
                let mut errors = 0_usize;
                for index in 0..PER_CLIENT {
                    let tenant = match tenants.next() {
                        Some(tenant) => tenant,
                        None => {
                            lgwks_std::trace::debug!("serve-load: the tenant cycle ended");
                            return Err(String::from("the tenant cycle ended"));
                        }
                    };
                    let key = format!("{prefix}-{client}-{index}");
                    match timed_post(addr, tenant, key.as_str()) {
                        Ok((200, micros)) => {
                            ok = ok.saturating_add(1);
                            latencies.push(micros);
                        }
                        Ok(_) => {
                            errors = errors.saturating_add(1);
                        }
                        Err(_) => {
                            errors = errors.saturating_add(1);
                        }
                    }
                }
                Ok((ok, errors, latencies))
            }));
        }
        let mut ok = 0_usize;
        let mut errors = 0_usize;
        let mut latencies = Vec::with_capacity(REQUESTS);
        for client in clients {
            match client.join() {
                Ok(Ok((client_ok, client_errors, mut client_latencies))) => {
                    ok = ok.saturating_add(client_ok);
                    errors = errors.saturating_add(client_errors);
                    latencies.append(&mut client_latencies);
                }
                Ok(Err(error)) => {
                    lgwks_std::trace::debug!(error = ?error, "serve-load: a load client failed");
                    return Err(error);
                }
                Err(_) => {
                    lgwks_std::trace::debug!("serve-load: a load client panicked");
                    return Err(String::from("a load client panicked"));
                }
            }
        }
        Ok((ok, errors, latencies))
    })?;
    let (ok, errors, mut latencies) = outcomes;
    latencies.sort();
    let middle =
        percentile(latencies.as_slice(), 50).ok_or_else(|| String::from("no latencies to rank"))?;
    let high =
        percentile(latencies.as_slice(), 95).ok_or_else(|| String::from("no latencies to rank"))?;
    let tail =
        percentile(latencies.as_slice(), 99).ok_or_else(|| String::from("no latencies to rank"))?;
    let seconds = start.elapsed().as_secs_f64();
    let total_32 = u32::try_from(ok.saturating_add(errors))
        .map_err(|cause| format!("the total does not fit u32: {cause}"))?;
    let per_second = f64::from(total_32) / seconds;
    let peak_rss = peak_rss_bytes()?;
    Ok(LoadNumbers {
        ok,
        errors,
        per_second,
        middle,
        high,
        tail,
        peak_rss,
    })
}

/// One line of the report, through the caller's locked stdout.
fn say(line: &str) -> Result<(), String> {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}").map_err(|error| format!("the report line is refused: {error}"))?;
    out.flush()
        .map_err(|error| format!("the report line is refused: {error}"))
}

/// The plain axum handler: per-tenant counts, no run store.
async fn plain_ingress(
    State(counts): State<Arc<Mutex<HashMap<String, u64>>>>,
    request: HttpRequest,
) -> HttpResponse {
    let (parts, body) = request.into_parts();
    let Some(rest) = parts.uri.path().strip_prefix("/t/") else {
        return (StatusCode::BAD_REQUEST, Body::from("name a tenant")).into_response();
    };
    let Some((tenant, _)) = rest.split_once('/') else {
        return (StatusCode::BAD_REQUEST, Body::from("name a tenant")).into_response();
    };
    let body = match lgwks_deps::axum::body::to_bytes(body, 1_048_576).await {
        Ok(body) => body,
        Err(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Body::from("the body is over the edge's ceiling"),
            )
                .into_response();
        }
    };
    let _ = body;
    let mut counts = match counts.lock() {
        Ok(counts) => counts,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Body::from("the counter is poisoned"),
            )
                .into_response();
        }
    };
    let count = counts.entry(tenant.to_owned()).or_insert(0);
    *count = count.saturating_add(1);
    (StatusCode::OK, Body::from(format!("{{\"count\":{count}}}"))).into_response()
}

/// One single POST against the edge with a 2 s bound, printing the raw
/// answer or the timeout. A silent server fails here in two seconds instead
/// of costing every load client its own full timeout with no output.
fn probe_once(addr: SocketAddr) -> Result<(), String> {
    let target = "/t/acme/drafts";
    let key = "probe-0";
    let head = format!(
        "POST {target} HTTP/1.1\r\nhost: {addr}\r\ncontent-length: {}\r\nconnection: close\r\nidempotency-key: {key}\r\n\r\n",
        LOAD_BODY.len()
    );
    let bound = Duration::from_secs(2);
    let mut answer = String::new();
    let probed = TcpStream::connect(addr)
        .and_then(|mut stream| {
            stream.set_read_timeout(Some(bound))?;
            stream.set_write_timeout(Some(bound))?;
            stream.write_all(head.as_bytes())?;
            stream.write_all(LOAD_BODY)?;
            stream.read_to_string(&mut answer)
        })
        .map_err(|cause| format!("the probe exchange failed: {cause}"));
    match probed {
        Ok(_) => match split_answer(&answer) {
            Some((status, body)) => say(&format!("probe: status={status} body={body:?}")),
            None => say(&format!("probe: not an HTTP answer: {answer:?}")),
        },
        Err(cause) => say(&format!("probe: {cause}")),
    }
}

/// Report one stack's numbers.
fn report(name: &str, numbers: &LoadNumbers) -> Result<(), String> {
    say(&format!(
        "{name}: sent={} ok={} errors={} rps={:.0} p50={}us p95={}us p99={}us rss={}B",
        numbers.ok.saturating_add(numbers.errors),
        numbers.ok,
        numbers.errors,
        numbers.per_second,
        numbers.middle,
        numbers.high,
        numbers.tail,
        numbers.peak_rss,
    ))
}

fn main() -> Result<(), String> {
    let unique = lgwks_std::random::bytes::<8>()
        .map_err(|cause| format!("the store entropy is refused: {cause}"))?;
    let hex: String = unique.iter().map(|byte| format!("{byte:02x}")).collect();
    let store = std::env::temp_dir().join(format!("lgwks-serve-load-{hex}"));
    std::fs::create_dir_all(store.as_path())
        .map_err(|error| format!("the store dir is refused: {error}"))?;
    let served = {
        let options =
            ServeOptions::new(0, 1_000, store.clone()).map_err(|error| error.to_string())?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("the listener is refused: {error}"))?;
        let addr = listener
            .local_addr()
            .map_err(|error| format!("the address is refused: {error}"))?;
        let stop = EdgeStop::new();
        let route = TypedRoute::new(
            RouteMethod::Post,
            "/drafts",
            move |tenant: String, _body: Vec<u8>| async move {
                Ok(format!("draft:{tenant}").into_bytes())
            },
        );
        std::thread::scope(|scope| {
            let ending = stop.clone();
            let served = scope.spawn(move || {
                lgwks_bot::rt::serve::serve(&options, std::vec![route], listener, &ending)
                    .map_err(|error| error.to_string())
            });
            probe_once(addr)?;
            let probed = load(addr, "serve");
            // Stop first: propagating a load error before stopping leaves
            // `scope` joining a serve thread that never stops.
            stop.stop();
            match served.join() {
                Ok(joined) => {
                    joined?;
                    Ok(probed?)
                }
                Err(_) => Err(String::from("the serve thread panicked")),
            }
        })?
    };
    report("serve", &served)?;
    let plain = {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("the listener is refused: {error}"))?;
        let addr = listener
            .local_addr()
            .map_err(|error| format!("the address is refused: {error}"))?;
        let token = lgwks_bot::rt::sync::CancellationToken::new();
        let stopping = token.clone();
        let counts: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
        let router = Router::new()
            .fallback(plain_ingress)
            .with_state(Arc::clone(&counts));
        std::thread::scope(|scope| {
            let served = scope.spawn(move || {
                if let Err(error) = listener.set_nonblocking(true) {
                    lgwks_std::trace::debug!(error = ?error, "serve-load: the plain listener is refused");
                    return Err(format!("the plain listener is refused: {error}"));
                }
                lgwks_bot::block_on(async move {
                    let listener = lgwks_deps::tokio::net::TcpListener::from_std(listener)
                        .map_err(|error| format!("the plain listener is refused: {error}"))?;
                    if let Err(error) = lgwks_deps::axum::serve(listener, router)
                        .with_graceful_shutdown(stopping.cancelled_owned())
                        .await
                    {
                        lgwks_std::trace::debug!(error = ?error, "serve-load: the plain server ended with an error");
                    }
                    Ok::<(), String>(())
                })
            });
            let probed = load(addr, "plain");
            // Cancel first: propagating a load error before cancelling leaves
            // `scope` joining a server thread that never stops.
            token.cancel();
            match served.join() {
                Ok(joined) => {
                    joined?;
                    Ok(probed?)
                }
                Err(_) => Err(String::from("the plain thread panicked")),
            }
        })?
    };
    report("plain", &plain)?;
    if plain.per_second.is_normal() {
        let overhead = (plain.per_second - served.per_second) / plain.per_second * 100.0;
        say(&format!("overhead: {overhead:.1}% rps vs plain axum"))?;
    }
    if std::fs::remove_dir_all(store.as_path()).is_err() {
        lgwks_std::trace::debug!("serve-load: the scratch store was not removed");
    }
    Ok(())
}
