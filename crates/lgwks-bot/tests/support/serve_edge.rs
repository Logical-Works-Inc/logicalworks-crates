//! Shared edge, store and child helpers for the `serve` suites.
//!
//! One definition of the loopback exchange, the served scope, the counting
//! and parking handlers' tools, the typed-body decoder, the shipped binary's
//! path and the kill-and-wait child: `tests/it/serve.rs` proves the edge over
//! loopback through these, and `tests/it/serve_process.rs` proves the shipped
//! binary and the restart through the same ones, so no block is copied
//! between the two suites.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use lgwks_std::json::Value;

use lgwks_bot::rt::edge::EdgeStop;
use lgwks_bot::rt::serve::{RouteMethod, ServeOptions, ServeReport, TypedRoute};

/// One HTTP answer split into status, headers and body.
pub fn split_answer(answer: &str) -> Option<(u16, String, String)> {
    let (status_line, rest) = answer.split_once("\r\n")?;
    let code = status_line.strip_prefix("HTTP/1.1 ")?.get(..3)?;
    let (headers, body) = rest.split_once("\r\n\r\n")?;
    Some((code.parse().ok()?, headers.to_owned(), body.to_owned()))
}

/// One `POST` to `tenant` `path` on a fresh connection.
pub fn post(
    addr: SocketAddr,
    tenant: &str,
    path: &str,
    body: &[u8],
    key: Option<&str>,
) -> Result<(u16, String, String), String> {
    post_timeout(addr, tenant, path, body, key, None)
}

/// One `POST` to `tenant` `path` on a fresh connection, bounded by `timeout`.
///
/// A `None` timeout reads and writes without bound; `Some` bounds both, so a
/// stalled server fails the exchange instead of parking the client forever.
pub fn post_timeout(
    addr: SocketAddr,
    tenant: &str,
    path: &str,
    body: &[u8],
    key: Option<&str>,
    timeout: Option<Duration>,
) -> Result<(u16, String, String), String> {
    match key {
        Some(key) => post_with(
            addr,
            tenant,
            path,
            body,
            &[("idempotency-key", key)],
            timeout,
        ),
        None => post_with(addr, tenant, path, body, &[], timeout),
    }
}

/// One `POST` to `tenant` `path` carrying `headers`, bounded by `timeout`.
pub fn post_with(
    addr: SocketAddr,
    tenant: &str,
    path: &str,
    body: &[u8],
    headers: &[(&str, &str)],
    timeout: Option<Duration>,
) -> Result<(u16, String, String), String> {
    let target = format!("/t/{tenant}{path}");
    let mut head = format!(
        "POST {target} HTTP/1.1\r\nhost: {addr}\r\ncontent-length: {}\r\nconnection: close\r\n",
        body.len()
    );
    for &(name, value) in headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let mut answer = String::new();
    TcpStream::connect(addr)
        .and_then(|mut stream| {
            if let Some(bound) = timeout {
                stream.set_read_timeout(Some(bound))?;
                stream.set_write_timeout(Some(bound))?;
            }
            stream.write_all(head.as_bytes())?;
            stream.write_all(body)?;
            stream.read_to_string(&mut answer)
        })
        .map_err(|cause| format!("the exchange failed: {cause}"))?;
    split_answer(&answer).ok_or_else(|| format!("not an HTTP answer: {answer:?}"))
}

/// A listener on a free loopback port, and the address it holds.
pub fn loopback() -> Result<(TcpListener, SocketAddr), String> {
    let refused = |what: &str, error: std::io::Error| format!("the {what} is refused: {error}");
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| refused("listener", error))?;
    let addr = listener
        .local_addr()
        .map_err(|error| refused("address", error))?;
    Ok((listener, addr))
}

/// Run `probe` while `routes` are served from `store`, stopping after.
///
/// The serve owns the listener until the probe stops it; the report counts
/// what drained and what was cancelled.
pub fn with_serve<T>(
    routes: Vec<TypedRoute>,
    store: &crate::scratch::Scratch,
    max_in_flight: u64,
    probe: impl FnOnce(SocketAddr, &EdgeStop) -> Result<T, String>,
) -> Result<(T, ServeReport), String> {
    let options = ServeOptions::new(0, max_in_flight, store.path().to_path_buf())
        .map_err(|error| error.to_string())?;
    let (listener, addr) = loopback()?;
    let stop = EdgeStop::new();
    std::thread::scope(|scope| {
        let ending = stop.clone();
        let served = scope.spawn(move || {
            lgwks_bot::rt::serve::serve(&options, routes, listener, &ending)
                .map_err(|error| error.to_string())
        });
        let probed = probe(addr, &stop);
        stop.stop();
        match served.join() {
            Ok(joined) => {
                let report = joined?;
                let out = probed?;
                Ok((out, report))
            }
            Err(_) => {
                lgwks_std::trace::debug!("serve-test: the serve thread panicked");
                Err(String::from("the serve thread panicked"))
            }
        }
    })
}

/// A scratch store named `name` and a fresh run counter for it.
pub fn counted(name: &str) -> Result<(crate::scratch::Scratch, Arc<Counter>), String> {
    let store = crate::scratch::Scratch::new(name).map_err(|error| error.to_string())?;
    Ok((store, Arc::new(Counter::default())))
}

/// `POST /drafts`, folding every request into `counter` under its tenant.
pub fn counting_route(counter: &Arc<Counter>) -> TypedRoute {
    let worker = Arc::clone(counter);
    TypedRoute::new(
        RouteMethod::Post,
        "/drafts",
        move |tenant: String, _body: Vec<u8>| {
            let worker = Arc::clone(&worker);
            async move { worker.fold(tenant.as_str()) }
        },
    )
}

/// `POST {path}` that flags `entered`, then parks on `gate` until released.
pub fn parking_route(path: &str, gate: &Arc<Gate>, entered: &Arc<AtomicBool>) -> TypedRoute {
    let parked = Arc::clone(gate);
    let signalled = Arc::clone(entered);
    TypedRoute::new(
        RouteMethod::Post,
        path,
        move |_tenant: String, _body: Vec<u8>| {
            let parked = Arc::clone(&parked);
            let signalled = Arc::clone(&signalled);
            async move {
                signalled.store(true, Ordering::SeqCst);
                parked.pass().map_err(|message| (500u16, message))
            }
        },
    )
}

/// Yield until a parking handler has `entered`, or refuse after 15 seconds.
pub fn await_entered(entered: &AtomicBool) -> Result<(), String> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(15))
        .ok_or_else(|| String::from("the park deadline does not fit the clock"))?;
    while !entered.load(Ordering::SeqCst) {
        if Instant::now() >= deadline {
            lgwks_std::trace::debug!("serve-test: the parked handler never entered");
            return Err(String::from("the parked handler never entered"));
        }
        std::thread::yield_now();
    }
    Ok(())
}

/// A handler's synchronous answer: response bytes, or `(status, message)`.
pub type Answer = fn(Vec<u8>) -> Result<Vec<u8>, (u16, String)>;

/// `POST /drafts` that counts each run in `runs`, then answers `answer(body)`.
pub fn tallying_route(runs: &Arc<AtomicUsize>, answer: Answer) -> TypedRoute {
    let counted = Arc::clone(runs);
    TypedRoute::new(
        RouteMethod::Post,
        "/drafts",
        move |_tenant: String, body: Vec<u8>| {
            let counted = Arc::clone(&counted);
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                answer(body)
            }
        },
    )
}

/// Counts what the edge ran, per tenant.
#[derive(Debug, Default)]
pub struct Counter {
    /// Handler runs so far.
    runs: AtomicUsize,
    /// Requests answered per tenant.
    per_tenant: Mutex<HashMap<String, u64>>,
}

impl Counter {
    /// Fold one request for `tenant`, answering its running count.
    pub fn fold(&self, tenant: &str) -> Result<Vec<u8>, (u16, String)> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let mut counts = match self.per_tenant.lock() {
            Ok(counts) => counts,
            Err(error) => {
                lgwks_std::trace::debug!(error = ?error, "serve-test: the counter is poisoned");
                return Err((500u16, String::from("the counter is poisoned")));
            }
        };
        let count = counts.entry(tenant.to_owned()).or_insert(0);
        *count = count.saturating_add(1);
        Ok(format!("{{\"count\":{count}}}").into_bytes())
    }

    /// Handler runs so far.
    pub fn runs(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }
}

/// A gate parked handlers wait on, released once by the test.
#[derive(Debug, Default)]
pub struct Gate {
    /// Whether the gate is open.
    open: Mutex<bool>,
    /// Rung on release.
    bell: Condvar,
}

impl Gate {
    /// Block until released, answering the parked body.
    pub fn pass(&self) -> Result<Vec<u8>, String> {
        match self.open.lock() {
            Ok(mut open) => {
                while !*open {
                    open = match self.bell.wait(open) {
                        Ok(woke) => woke,
                        Err(error) => {
                            lgwks_std::trace::debug!(error = ?error, "serve-test: the test gate is poisoned");
                            return Err(String::from("the test gate is poisoned"));
                        }
                    };
                }
                Ok(b"released".to_vec())
            }
            Err(error) => {
                lgwks_std::trace::debug!(error = ?error, "serve-test: the test gate is poisoned");
                Err(String::from("the test gate is poisoned"))
            }
        }
    }

    /// Release every parked handler.
    pub fn release(&self) -> Result<(), String> {
        match self.open.lock() {
            Ok(mut open) => {
                *open = true;
            }
            Err(error) => {
                lgwks_std::trace::debug!(error = ?error, "serve-test: the test gate is poisoned");
                return Err(String::from("the test gate is poisoned"));
            }
        }
        self.bell.notify_all();
        Ok(())
    }
}

/// Decode the body as the route's JSON input, naming the field it misses.
pub fn decode_draft(body: &[u8]) -> Result<String, (u16, String)> {
    match lgwks_std::json::from_slice::<Value>(body) {
        Ok(value) => match value.get("title").and_then(|title| title.as_str()) {
            Some(title) => Ok(format!("draft:{title}")),
            None => Err((
                400u16,
                String::from("the body does not decode as DraftRequest: missing field `title`"),
            )),
        },
        Err(error) => Err((400u16, format!("the body is not JSON: {error}"))),
    }
}

/// Read the running total out of an answer.
pub fn parse_count(body: &str) -> Result<u64, String> {
    let trimmed = body.trim();
    let digits = trimmed
        .strip_prefix("{\"count\":")
        .and_then(|rest| rest.strip_suffix('}'))
        .ok_or_else(|| {
            lgwks_std::trace::debug!(body = trimmed, "serve-test: no count to parse");
            format!("no count in {trimmed:?}")
        })?;
    let mut count: u64 = 0;
    for byte in digits.bytes() {
        if !byte.is_ascii_digit() {
            lgwks_std::trace::debug!(body = trimmed, "serve-test: the count is not a number");
            return Err(format!("no count in {trimmed:?}"));
        }
        count = count
            .saturating_mul(10)
            .saturating_add(u64::from(byte.saturating_sub(b'0')));
    }
    Ok(count)
}

/// A child server killed on drop.
pub struct ChildGuard {
    /// The child, while running.
    child: Option<Child>,
}

impl ChildGuard {
    /// Hold `child` until stopped or dropped.
    pub fn hold(child: Child) -> Self {
        Self { child: Some(child) }
    }

    /// Stop the child and wait for it.
    pub fn stop(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.child.take() {
            match child.kill() {
                Ok(()) => {}
                Err(error) => {
                    lgwks_std::trace::debug!(error = ?error, "serve-test: the child is already gone");
                }
            }
            match child.wait() {
                Ok(_) => Ok(()),
                Err(error) => Err(format!("the child did not exit: {error}")),
            }
        } else {
            Ok(())
        }
    }
}

impl ChildGuard {
    /// Send SIGTERM, wait for the exit, and answer its status and stdout.
    #[cfg(unix)]
    pub fn terminate(&mut self) -> Result<(std::process::ExitStatus, String), String> {
        let Some(child) = self.child.take() else {
            lgwks_std::trace::debug!("serve-test: SIGTERM asked of a stopped child");
            return Err(String::from("the child was already stopped"));
        };
        let refusal = match std::process::Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status()
        {
            Ok(status) if status.success() => None,
            Ok(status) => Some(format!("kill -TERM answered {status}")),
            Err(error) => Some(format!("kill -TERM did not run: {error}")),
        };
        if let Some(refusal) = refusal {
            lgwks_std::trace::debug!(error = refusal.as_str(), "serve-test: SIGTERM was not sent");
            self.child = Some(child);
            return Err(refusal);
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("the child did not exit: {error}"))?;
        Ok((
            output.status,
            String::from_utf8_lossy(&output.stdout).into_owned(),
        ))
    }
}

/// Start the shipped `estate serve` over `store` on `port`, stdout to `stdout`.
pub fn spawn_estate_serve(
    store: &std::path::Path,
    port: u16,
    extra: &[&str],
    stdout: std::process::Stdio,
) -> Result<ChildGuard, String> {
    let store_arg = store.to_str().ok_or_else(|| {
        lgwks_std::trace::debug!("serve-test: the store path is not text");
        String::from("the store path is not text")
    })?;
    let child = std::process::Command::new(estate_bin()?)
        .arg("serve")
        .arg("--store")
        .arg(store_arg)
        .arg("--port")
        .arg(format!("{port}"))
        .args(extra)
        .stdout(stdout)
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("estate serve did not start: {error}"))?;
    Ok(ChildGuard::hold(child))
}

/// The deadline a freshly started server has to answer its first request.
pub fn ready_deadline() -> Result<std::time::Instant, String> {
    std::time::Instant::now()
        .checked_add(std::time::Duration::from_secs(20))
        .ok_or_else(|| {
            lgwks_std::trace::debug!("serve-test: the ready deadline does not fit the clock");
            String::from("the ready deadline does not fit the clock")
        })
}

impl Drop for ChildGuard {
    /// Kill and wait, logging what refuses.
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if let Err(error) = child.kill() {
                lgwks_std::trace::debug!(error = ?error, "serve-test: the dropped child is already gone");
            }
            match child.wait() {
                Ok(_) => {}
                Err(error) => {
                    lgwks_std::trace::debug!(error = ?error, "serve-test: the dropped child did not exit");
                }
            }
        }
    }
}

/// Where the built command lives for the process test.
pub fn estate_bin() -> Result<PathBuf, String> {
    let test_bin = std::env::current_exe()
        .map_err(|error| format!("the test binary path is refused: {error}"))?;
    let mut dir = test_bin
        .parent()
        .ok_or_else(|| {
            lgwks_std::trace::debug!("serve-test: the test binary has no directory");
            String::from("the test binary has no directory")
        })?
        .to_path_buf();
    if dir.file_name().is_some_and(|name| name == "deps") {
        dir = dir
            .parent()
            .ok_or_else(|| {
                lgwks_std::trace::debug!("serve-test: the target directory has no parent");
                String::from("the target directory has no parent")
            })?
            .to_path_buf();
    }
    let name = if cfg!(windows) {
        "estate.exe"
    } else {
        "estate"
    };
    Ok(dir.join(name))
}

/// A loopback port nothing holds yet.
pub fn free_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("no loopback port is free: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("the loopback address is refused: {error}"))?
        .port();
    drop(listener);
    Ok(port)
}

/// POST until the server answers or the deadline passes.
pub fn wait_ready(
    addr: SocketAddr,
    tenant: &str,
    path: &str,
    body: &[u8],
    key: Option<&str>,
    deadline: Instant,
) -> Result<(u16, String, String), String> {
    loop {
        match post(addr, tenant, path, body, key) {
            Ok(answered) => {
                return Ok(answered);
            }
            Err(_) => {
                if Instant::now() >= deadline {
                    lgwks_std::trace::debug!("serve-test: the server never answered");
                    return Err(String::from("the server never answered"));
                }
                std::thread::yield_now();
            }
        }
    }
}

/// One `GET` from `tenant` `path` on a fresh connection.
pub fn get(addr: SocketAddr, tenant: &str, path: &str) -> Result<(u16, String, String), String> {
    let target = format!("/t/{tenant}{path}");
    let head = format!("GET {target} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\r\n");
    let mut answer = String::new();
    TcpStream::connect(addr)
        .and_then(|mut stream| {
            stream.write_all(head.as_bytes())?;
            stream.read_to_string(&mut answer)
        })
        .map_err(|cause| format!("the exchange failed: {cause}"))?;
    split_answer(&answer).ok_or_else(|| format!("not an HTTP answer: {answer:?}"))
}

/// POST a JSON draft to the reference route, or the refusal.
pub fn post_draft(
    addr: SocketAddr,
    tenant: &str,
    title: &str,
    key: Option<&str>,
) -> Result<(u16, String), String> {
    let body = format!("{{\"title\": \"{title}\", \"body\": \"B\"}}");
    let (status, _, answered) = post(addr, tenant, "/drafts", body.as_bytes(), key)?;
    Ok((status, answered))
}
