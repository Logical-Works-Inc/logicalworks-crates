//! Keep-alive pool comparison: one `Client` against one agent per call.
//!
//! A loopback server answers every request on a connection until the peer
//! goes away, and counts the connections it accepted. The rig fires one
//! sequential burst of small GETs through a pooled [`lgwks_std::http::Client`]
//! and the same burst through the one-shot free functions, then reports
//! per-request p50/p95/p99 for each side and how many connections each side
//! cost. The numbers are loopback numbers for this host, not a cross-platform
//! claim: the point is the ratio and the connection count, which is what the
//! pool exists to change.
//!
//! ```sh
//! cargo run --release --locked --manifest-path bench/http-pool/Cargo.toml
//! cargo run --release --locked --manifest-path bench/http-pool/Cargo.toml -- --requests 500
//! cargo run --release --locked --manifest-path bench/http-pool/Cargo.toml -- --json bench/http-pool/results.json
//! ```

use std::io::Write;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// The one request-head reader every loopback server uses, included by
/// path like the `lgwks_std` test harnesses include it: this root is
/// invisible to the estate dependency contract, so a file-level include
/// adds no edge while keeping one server behavior everywhere.
#[path = "../../crates/lgwks-std/tests/support/loopback.rs"]
mod loopback;

use loopback::read_request_head;

/// Requests per side unless `--requests` says otherwise.
const REQUESTS: usize = 200;

/// The response every request gets: small, fixed, keep-alive friendly.
const REPLY: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";

/// Why the rig did not print its rows.
#[derive(Debug)]
enum PoolError {
    /// A socket, file or stdout operation failed.
    Io {
        /// What was being done.
        context: &'static str,
        /// The cause.
        source: std::io::Error,
    },
    /// The HTTP client refused to build or a request failed.
    Http {
        /// What was being done.
        context: &'static str,
        /// The client's own account of it.
        detail: String,
    },
    /// The fixture answered something other than 200.
    Status {
        /// Which burst saw it.
        burst: &'static str,
        /// The status the client read.
        status: String,
    },
    /// A burst recorded no latencies, so it has no percentiles.
    EmptyBurst {
        /// Which burst.
        burst: &'static str,
    },
    /// The pooled burst took longer than its deadline.
    Overran {
        /// The deadline.
        deadline: Duration,
    },
    /// A fixture thread ended by unwinding.
    Panicked {
        /// Which thread.
        thread: String,
        /// What its panic carried.
        message: String,
    },
    /// The list of connection threads was poisoned, so they cannot be joined.
    Poisoned,
}

impl std::fmt::Display for PoolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Io {
                context,
                ref source,
            } => write!(formatter, "{context}: {source}"),
            Self::Http {
                context,
                ref detail,
            } => write!(formatter, "{context}: {detail}"),
            Self::Status { burst, ref status } => {
                write!(formatter, "the {burst} burst saw {status}")
            }
            Self::EmptyBurst { burst } => {
                write!(formatter, "the {burst} burst recorded no latencies")
            }
            Self::Overran { deadline } => {
                write!(
                    formatter,
                    "the pooled burst overran its {deadline:?} deadline"
                )
            }
            Self::Panicked {
                ref thread,
                ref message,
            } => write!(formatter, "{thread} panicked: {message}"),
            Self::Poisoned => formatter.write_str(
                "the bench handler list is poisoned: the fixture cannot join its threads",
            ),
        }
    }
}

impl std::error::Error for PoolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Io { ref source, .. } => Some(source),
            _ => None,
        }
    }
}

/// An I/O failure while doing `context`.
fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> PoolError {
    move |source| {
        lgwks_std::trace::error!(context, %source, "an I/O step of the bench failed");
        PoolError::Io { context, source }
    }
}

/// A client failure while doing `context`.
fn http<E: std::fmt::Display>(context: &'static str) -> impl FnOnce(E) -> PoolError {
    move |error| {
        let detail = error.to_string();
        lgwks_std::trace::error!(
            context,
            detail = detail.as_str(),
            "an HTTP step of the bench failed"
        );
        PoolError::Http { context, detail }
    }
}

/// One line to the locked stdout; a write error reaches `main` as an I/O error.
fn say(line: &str) -> Result<(), PoolError> {
    writeln!(std::io::stdout().lock(), "{line}").map_err(io("writing to stdout"))
}

/// Serve one connection until the peer goes away; a torn head or a refused
/// write is returned to the join that owns this thread.
fn serve_connection(mut stream: TcpStream, requests: &AtomicUsize) -> std::io::Result<()> {
    loop {
        let head = read_request_head(&mut stream)?;
        if !head.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(());
        }
        requests.fetch_add(1, Ordering::SeqCst);
        stream.write_all(REPLY)?;
    }
}

/// The outcome of one burst: per-request latencies and connections opened.
struct Burst {
    /// Per-request latency, in microseconds, in firing order.
    latencies_us: Vec<u128>,
    /// Connections the server accepted during the burst.
    accepts: usize,
}

/// Sort a copy and read one quantile, or `None` when there is nothing to read.
fn quantile(sorted: &[u128], ratio: f64) -> Option<u128> {
    let last = sorted.len().checked_sub(1)?;
    let at = (last as f64 * ratio).round() as usize;
    sorted.get(at.min(last)).copied()
}

/// Render a joined thread's panic payload as text, never echoing it raw.
///
/// A panic payload can carry anything the fixture thread formatted, so the
/// failure names the fixture instead of echoing the payload.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast_ref::<&str>() {
        Some(text) => (*text).to_owned(),
        None => match payload.downcast_ref::<String>() {
            Some(text) => text.clone(),
            None => "a payload that is not text".to_owned(),
        },
    }
}

/// Print one burst row and return its p50/p95/p99; an empty burst is refused.
fn report(name: &'static str, burst: &Burst) -> Result<(u128, u128, u128), PoolError> {
    let mut sorted = burst.latencies_us.clone();
    sorted.sort_unstable();
    let (Some(p50), Some(p95), Some(p99)) = (
        quantile(&sorted, 0.50),
        quantile(&sorted, 0.95),
        quantile(&sorted, 0.99),
    ) else {
        lgwks_std::trace::error!(name, "the burst recorded no latencies");
        return Err(PoolError::EmptyBurst { burst: name });
    };
    say(&format!(
        "{name:>12}  p50 {p50:>7} us  p95 {p95:>7} us  p99 {p99:>7} us  connections {accepts}",
        accepts = burst.accepts
    ))?;
    Ok((p50, p95, p99))
}

/// Read `--requests N` and `--json PATH` from the arguments.
fn args() -> (usize, Option<String>) {
    let mut requests = REQUESTS;
    let mut json: Option<String> = None;
    let mut rest = std::env::args().skip(1);
    while let Some(arg) = rest.next() {
        if arg == "--requests" {
            if let Some(value) = rest.next()
                && let Ok(count) = value.parse()
            {
                requests = count;
            }
        } else if arg == "--json" {
            json = rest.next();
        }
    }
    (requests.max(1), json)
}

/// Join one fixture thread, turning its unwind into a typed failure.
fn join_fixture(
    thread: String,
    handle: std::thread::JoinHandle<std::io::Result<()>>,
) -> Result<(), PoolError> {
    match handle.join() {
        Ok(served) => served.map_err(io("serving the bench fixture")),
        Err(payload) => {
            let message = panic_message(payload);
            lgwks_std::trace::error!(thread = thread.as_str(), %message, "a bench fixture thread panicked");
            Err(PoolError::Panicked { thread, message })
        }
    }
}

fn main() -> Result<(), PoolError> {
    let (requests, json) = args();
    // The server is this process's own fixture: the loopback address and a
    // port the OS picks, and the URL is read back from the bound socket.
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(io("binding the fixture"))?;
    let bound = listener
        .local_addr()
        .map_err(io("reading the fixture address"))?;
    let accepts = Arc::new(AtomicUsize::new(0));
    let served = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let url = format!("http://{bound}/");

    let handlers: Arc<std::sync::Mutex<Vec<std::thread::JoinHandle<std::io::Result<()>>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let server = std::thread::Builder::new()
        .name("lgwks-bench-http-pool-server".to_owned())
        .spawn({
            let accepts = Arc::clone(&accepts);
            let served = Arc::clone(&served);
            let stop = Arc::clone(&stop);
            let handlers = Arc::clone(&handlers);
            move || -> std::io::Result<()> {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                    let stream = stream?;
                    accepts.fetch_add(1, Ordering::SeqCst);
                    let served = Arc::clone(&served);
                    let handler = std::thread::Builder::new()
                        .name("lgwks-bench-http-pool-conn".to_owned())
                        .spawn(move || serve_connection(stream, &served))?;
                    let Ok(mut held) = handlers.lock() else {
                        lgwks_std::trace::error!("bench handler list poisoned");
                        return Err(std::io::Error::other(
                            "bench handler list poisoned: the fixture cannot track its threads",
                        ));
                    };
                    held.push(handler);
                }
                Ok(())
            }
        })
        .map_err(io("starting the fixture server"))?;

    // Warm up the process-global TLS and allocator state so the measured
    // bursts do not pay for first use. The warm client is dropped right
    // away: a pooled client that lived on would hold its keep-alive
    // connection open and the server thread serving it would never join.
    let warm = lgwks_std::http::Client::new(lgwks_std::http::Options::default())
        .map_err(http("building the warm-up client"))?;
    drop(
        warm.get_with(&url, warm.options())
            .map_err(http("the warm-up request"))?,
    );
    drop(warm);

    // The pool under measurement, pinned to the values
    // `PoolConfig::default` carries (ureq's own): the burst below measures
    // pooling behavior, so the configuration it measures is written down,
    // not inherited silently.
    let pool = lgwks_std::http::PoolConfig::default()
        .max_idle_connections(10)
        .max_idle_connections_per_host(3)
        .max_idle_age(Duration::from_secs(15));

    let pooled = |requests: usize| -> Result<Burst, PoolError> {
        let before = accepts.load(Ordering::SeqCst);
        let client = lgwks_std::http::Client::with_pool(lgwks_std::http::Options::default(), pool)
            .map_err(http("building the pooled client"))?;
        let mut latencies_us = Vec::with_capacity(requests);
        for _ in 0..requests {
            let started = Instant::now();
            let response = client
                .get_with(&url, client.options())
                .map_err(http("a pooled request"))?;
            if response.status != 200 {
                lgwks_std::trace::error!(status = response.status, "pooled burst saw a failure");
                return Err(PoolError::Status {
                    burst: "pooled",
                    status: response.status.to_string(),
                });
            }
            latencies_us.push(started.elapsed().as_micros());
        }
        drop(client);
        Ok(Burst {
            latencies_us,
            accepts: accepts.load(Ordering::SeqCst).saturating_sub(before),
        })
    };

    let one_shot = |requests: usize| -> Result<Burst, PoolError> {
        let before = accepts.load(Ordering::SeqCst);
        let mut latencies_us = Vec::with_capacity(requests);
        for _ in 0..requests {
            let started = Instant::now();
            let response = lgwks_std::http::get(&url).map_err(http("a one-shot request"))?;
            if response.status != 200 {
                lgwks_std::trace::error!(status = response.status, "one-shot burst saw a failure");
                return Err(PoolError::Status {
                    burst: "one-shot",
                    status: response.status.to_string(),
                });
            }
            latencies_us.push(started.elapsed().as_micros());
        }
        Ok(Burst {
            latencies_us,
            accepts: accepts.load(Ordering::SeqCst).saturating_sub(before),
        })
    };

    let timeout = Duration::from_secs(120);
    let deadline = Instant::now() + timeout;
    let pooled = pooled(requests)?;
    if Instant::now() > deadline {
        lgwks_std::trace::error!(?timeout, "pooled burst overran its deadline");
        return Err(PoolError::Overran { deadline: timeout });
    }
    let single = one_shot(requests)?;

    say(&format!(
        "keep-alive burst of {requests} sequential GETs, loopback, this host:"
    ))?;
    let (pooled_p50, pooled_p95, pooled_p99) = report("pooled", &pooled)?;
    let (single_p50, single_p95, single_p99) = report("one-shot", &single)?;
    say(&format!(
        "served {} requests in total",
        served.load(Ordering::SeqCst)
    ))?;

    if let Some(path) = json {
        let document = format!(
            "{{\n  \"requests\": {requests},\n  \"pooled\": {{\"p50_us\": {pooled_p50}, \"p95_us\": {pooled_p95}, \"p99_us\": {pooled_p99}, \"connections\": {}}},\n  \"one_shot\": {{\"p50_us\": {single_p50}, \"p95_us\": {single_p95}, \"p99_us\": {single_p99}, \"connections\": {}}}\n}}\n",
            pooled.accepts, single.accepts
        );
        std::fs::write(&path, document).map_err(io("writing the JSON report"))?;
        say(&format!("wrote {path}"))?;
    }

    stop.store(true, Ordering::SeqCst);
    drop(TcpStream::connect(bound).map_err(io("waking the fixture server"))?);
    join_fixture("the bench server thread".to_owned(), server)?;
    let Ok(mut handlers) = handlers.lock() else {
        lgwks_std::trace::error!("the bench handler list is poisoned");
        return Err(PoolError::Poisoned);
    };
    for (index, handler) in handlers.drain(..).enumerate() {
        join_fixture(format!("bench connection {index}"), handler)?;
    }
    Ok(())
}
