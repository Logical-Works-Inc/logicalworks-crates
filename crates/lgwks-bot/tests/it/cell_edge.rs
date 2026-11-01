//! The cell host behind the HTTP edge, driven over a real socket (#790).
//!
//! Source: the R4 falsifier, "two tenants never read each other's state",
//! against the thread-per-core host that replaced the lock-striped
//! partitions: concurrent tenants keep their own counts and a panicking route
//! is contained and serves again. The full-mailbox journey sizes the mailbox
//! below the public surface, so it lives beside the edge in
//! `src/rt/edge.rs`.

use std::future::{Future, ready};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lgwks_bot::rt::edge::{EdgeStop, serve};
use lgwks_bot::spec::EffectScope;
use lgwks_bot::{
    Auth, Bot, BotError, Cadence, Cap, GrantSet, Observe, Request, RequestFeed, Response,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Log `why` and hand it back, so every failed journey leaves a record.
fn refused<T>(why: String) -> Result<T, String> {
    lgwks_std::trace::warn!(%why, "cell_edge: a journey failed");
    Err(why)
}

/// Send one request on a fresh connection and read the whole answer; its
/// status, head and body.
fn call_head(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<(u16, String, String), String> {
    let mut stream = TcpStream::connect(addr).map_err(|error| error.to_string())?;
    let head = format!(
        "{method} {path} HTTP/1.1\r\nhost: {addr}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .map_err(|error| format!("the request head was not written: {error}"))?;
    stream
        .write_all(body)
        .map_err(|error| format!("the request body was not written: {error}"))?;
    let mut answer = String::new();
    stream
        .read_to_string(&mut answer)
        .map_err(|error| format!("the answer was not read: {error}"))?;
    let status = answer
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("no status line in {answer:?}"))?;
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("no body in {answer:?}"))?;
    Ok((status, head.to_owned(), body.to_owned()))
}

/// A tenant's counter: `POST /count` adds one and answers it, `GET /count`
/// answers it as it stands, and a body of `boom` panics inside the cell.
fn counter(request: &Request<'_>, count: &mut u64) -> Response {
    if request.path() != "/count" {
        return Response::new(404, b"no such route".to_vec());
    }
    assert!(
        request.body() != b"boom",
        "the handler panics on a poisoned message"
    );
    if request.method() == "POST" {
        *count = count.saturating_add(1);
    }
    Response::new(200, count.to_string().into_bytes())
}

/// The world's view of the traffic: a `Wake` source that never fires, so the
/// world idles while the edge serves.
struct Idle {
    /// Polls so far.
    polls: Arc<AtomicU64>,
}

impl Observe for Idle {
    type Output = u64;

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn poll(&self, _call: (Auth, ())) -> impl Future<Output = Result<u64, BotError>> {
        self.polls.fetch_add(1, Ordering::AcqRel);
        ready(Ok(0))
    }

    fn cadence(&self) -> Cadence {
        Cadence::Wake
    }

    fn domain_id(&self) -> &str {
        "cell-edge-test::idle"
    }
}

/// Serve `feed` on a loopback port until the client finishes; the client's
/// result.
fn serve_until(
    feed: RequestFeed,
    client: impl FnOnce(SocketAddr) -> Result<(), String> + Send,
) -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let polls = Arc::new(AtomicU64::new(0));
    let mut world = Bot::builder("cell-edge")
        .with_effects(EffectScope::ephemeral()?)
        .observe(Idle {
            polls: Arc::clone(&polls),
        })
        .build(&GrantSet::empty())?;
    let stop = EdgeStop::new();
    std::thread::scope(|scope| -> TestResult {
        let finished = scope.spawn(|| {
            let journey = client(addr);
            stop.stop();
            journey
        });
        serve(&mut world, &feed, listener, &stop)?;
        match finished.join() {
            Ok(journey) => Ok(journey?),
            Err(_panicked) => Ok(refused(String::from("the client thread panicked"))?),
        }
    })
}

/// Post `posts` times then read the count for each tenant in `tenants`,
/// sharded over `threads` client threads.
fn hammer(addr: SocketAddr, tenants: u64, posts: u64, threads: u64) -> Result<(), String> {
    std::thread::scope(|scope| -> Result<(), String> {
        let mut workers = Vec::new();
        for thread in 0..threads {
            workers.push(scope.spawn(move || -> Result<(), String> {
                let mut tenant = thread;
                while tenant < tenants {
                    for _ in 0..posts {
                        let path = format!("/t/tenant-{tenant}/count");
                        let (status, _, _) = call_head(addr, "POST", &path, b"")?;
                        if status != 200 {
                            return refused(format!("tenant-{tenant} posted {status}"));
                        }
                    }
                    tenant = tenant.saturating_add(threads);
                }
                Ok(())
            }));
        }
        for worker in workers {
            match worker.join() {
                Ok(journey) => journey?,
                Err(_panicked) => return refused(String::from("a hammer thread panicked")),
            }
        }
        Ok(())
    })?;
    for tenant in 0..tenants {
        let path = format!("/t/tenant-{tenant}/count");
        let (status, _, body) = call_head(addr, "GET", &path, b"")?;
        if status != 200 || body != posts.to_string() {
            return refused(format!("tenant-{tenant} read {body:?} at {status}"));
        }
    }
    Ok(())
}

/// A hundred tenants over HTTP keep their own counts. Cites #790: the cell,
/// not the partition, holds the state.
#[test]
fn a_hundred_tenants_served_over_http_keep_their_own_counts() -> TestResult {
    let feed = RequestFeed::new(counter);
    serve_until(feed, |addr| hammer(addr, 100, 3, 10))?;
    Ok(())
}

/// A thousand tenants over HTTP keep their own counts. Cites #790: the host
/// holds thousands of cells with no cross-read.
#[test]
fn a_thousand_tenants_served_over_http_keep_their_own_counts() -> TestResult {
    let feed = RequestFeed::new(counter);
    serve_until(feed, |addr| hammer(addr, 1_000, 1, 20))?;
    Ok(())
}

/// A panicking route is answered `500` and serves again after the rebuild,
/// without the poisoned message. Cites #790: the panic rebuilds one cell.
#[test]
fn a_panicking_route_is_answered_500_and_serves_again_after_rebuild() -> TestResult {
    let feed = RequestFeed::new(counter);
    serve_until(feed.clone(), |addr| {
        let (first, _, first_body) = call_head(addr, "POST", "/t/acme/count", b"")?;
        if first != 200 || first_body != "1" {
            return refused(format!("the first post answered {first} {first_body:?}"));
        }
        let (panicked, _, _) = call_head(addr, "POST", "/t/acme/count", b"boom")?;
        if panicked != 500 {
            return refused(format!("the panic answered {panicked}, not 500"));
        }
        let (again, _, again_body) = call_head(addr, "POST", "/t/acme/count", b"")?;
        if again != 200 || again_body != "2" {
            return refused(format!(
                "after the panic acme read {again_body:?} at {again}"
            ));
        }
        let (other, _, other_body) = call_head(addr, "GET", "/t/globex/count", b"")?;
        if other != 200 || other_body != "0" {
            return refused(format!("globex read {other_body:?} at {other}"));
        }
        Ok(())
    })?;
    let counts = feed.counts();
    if counts.panicked != 1 {
        return Ok(refused(format!("panicked is {counts:?}"))?);
    }
    Ok(())
}
