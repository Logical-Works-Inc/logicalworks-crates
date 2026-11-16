//! The load client: keep-alive connections, each sending its requests in turn,
//! every request timed from its first byte written to its last byte read.
//!
//! `hit` sends posts only. `drive` sends a seeded mix: each request is a `GET`
//! with probability `READ_PERMILLE / 1000` and a `POST` otherwise, drawn from
//! [`lgwks_std::seeded`] under the seed plus the connection number, so one seed
//! names the whole request sequence. Every request carries an `idempotency-key`
//! unique to its seed, connection and position, so a surface that records keys
//! (`rt::serve`) runs each request and answers none from a replay.
//!
//! The client is closed loop: a connection sends its next request only after
//! the last answer, so a server that queues slows the client too, and the tail
//! it prints hides the queueing a constant-rate client would see (coordinated
//! omission). Throughput is printed beside the tail for that reason.

use std::io::Write as _;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use lgwks_bot::rt::io::{AsyncReadExt, AsyncWriteExt};
use lgwks_bot::rt::net::TcpStream;
use lgwks_bot::rt::task::JoinSet;
use lgwks_std::seeded::Seeded;

use crate::measure::Summary;
use crate::{LoadError, refused};

/// The most bytes one response head may carry before the client calls it
/// malformed.
const MAX_HEAD_BYTES: usize = 64 * 1024;

/// What one run of the client sends.
#[derive(Clone, Copy, Debug)]
pub(super) struct Mix {
    /// Keep-alive connections, each its own task.
    conns: usize,
    /// Requests each connection sends.
    requests: usize,
    /// Tenants the requests go round-robin over.
    tenants: usize,
    /// Reads per thousand requests: a request is a read when its draw below
    /// 1,000 is under this, so zero sends posts only and 1,000 reads only.
    read_permille: u64,
    /// The seed every connection's stream is cut from.
    seed: u64,
    /// Whether each request shows its tenant's key, `key-tenant-{n}`.
    keyed: bool,
}

impl Mix {
    /// The mix the five words name, or a usage refusal.
    pub(super) fn parse(words: [&str; 5], keyed: bool) -> Result<Self, LoadError> {
        let [conns, requests, tenants, share, seed] = words;
        let read_permille = match share.parse::<u64>() {
            Ok(share) if share <= 1_000 => share,
            _ => return refused(LoadError::Usage),
        };
        let Ok(seed) = seed.parse::<u64>() else {
            return refused(LoadError::Usage);
        };
        Ok(Self {
            conns: positive(conns)?,
            requests: positive(requests)?,
            tenants: positive(tenants)?,
            read_permille,
            seed,
            keyed,
        })
    }
}

/// One positive count.
fn positive(word: &str) -> Result<usize, LoadError> {
    match word.parse::<usize>() {
        Ok(value) if value > 0 => Ok(value),
        _ => refused(LoadError::Usage),
    }
}

/// What one or more connections measured.
#[derive(Default)]
struct Measured {
    /// Each answered request's latency, in microseconds.
    latencies: Vec<u128>,
    /// Requests answered `200`.
    ok: u64,
    /// Requests answered otherwise, or lost with their connection.
    errors: u64,
    /// `GET`s sent.
    reads: u64,
    /// `POST`s sent.
    writes: u64,
}

impl Measured {
    /// Fold `one` connection's numbers into these.
    fn absorb(&mut self, one: Self) {
        self.latencies.extend(one.latencies);
        self.ok = self.ok.saturating_add(one.ok);
        self.errors = self.errors.saturating_add(one.errors);
        self.reads = self.reads.saturating_add(one.reads);
        self.writes = self.writes.saturating_add(one.writes);
    }
}

/// Drive `addr` with `mix` and print the line.
pub(super) fn run(addr: &str, mix: Mix) -> Result<(), LoadError> {
    let Ok(addr) = addr.parse::<SocketAddr>() else {
        return refused(LoadError::Usage);
    };
    let started = Instant::now();
    let measured = lgwks_bot::rt::runtime::block_on(async move {
        let mut set = JoinSet::new();
        for (conn, stream) in (0..mix.conns).zip(0_u64..) {
            set.spawn(connection(addr, conn, mix.seed.wrapping_add(stream), mix));
        }
        let mut all = Measured::default();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(Ok(one)) => all.absorb(one),
                Ok(Err(error)) => {
                    lgwks_std::trace::warn!(%error, "edge-load: a connection failed");
                    all.errors = all.errors.saturating_add(1);
                }
                Err(_panicked) => all.errors = all.errors.saturating_add(1),
            }
        }
        all
    });
    report(
        measured,
        started.elapsed(),
        mix.conns.saturating_mul(mix.requests),
    )
}

/// Print throughput, the percentiles and the counts, as one line of `key=value` words.
fn report(mut measured: Measured, wall: Duration, sent: usize) -> Result<(), LoadError> {
    let summary = Summary::of(&mut measured.latencies);
    let max = match measured.latencies.last() {
        Some(&micros) => micros,
        None => 0,
    };
    let rps = u128::from(measured.ok)
        .saturating_mul(1_000_000)
        .div_euclid(wall.as_micros().max(1));
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "sent={sent} ok={} errors={} reads={} writes={} rps={rps} {} max={max}us wall={}ms",
        measured.ok,
        measured.errors,
        measured.reads,
        measured.writes,
        summary.line("latency"),
        wall.as_millis(),
    )?;
    Ok(())
}

/// One keep-alive connection sending `mix.requests` requests round-robin over
/// the tenants, starting at tenant `conn`, its reads drawn from stream `seed`.
async fn connection(
    addr: SocketAddr,
    conn: usize,
    seed: u64,
    mix: Mix,
) -> Result<Measured, LoadError> {
    // No `set_nodelay`: a connection writes one request and waits for its
    // answer, so no unacknowledged data is ever outstanding for Nagle to hold.
    let mut stream = TcpStream::connect(addr).await?;
    let mut draws = Seeded::from_seed(seed);
    let mut measured = Measured {
        latencies: Vec::with_capacity(mix.requests),
        ..Measured::default()
    };
    let mut buffer = Vec::with_capacity(1024);
    for sent in 0..mix.requests {
        let tenant = conn.saturating_add(sent).rem_euclid(mix.tenants);
        let read = draws.below(1_000)? < mix.read_permille;
        let verb = if read {
            measured.reads = measured.reads.saturating_add(1);
            "GET"
        } else {
            measured.writes = measured.writes.saturating_add(1);
            "POST"
        };
        let key = if mix.keyed {
            format!("authorization: Bearer key-tenant-{tenant}\r\n")
        } else {
            String::new()
        };
        let request = format!(
            "{verb} /t/tenant-{tenant}/count HTTP/1.1\r\nhost: {addr}\r\n{key}idempotency-key: {:x}-{conn}-{sent}\r\ncontent-length: 0\r\n\r\n",
            mix.seed
        );
        let started = Instant::now();
        stream.write_all(request.as_bytes()).await?;
        let status = read_response(&mut stream, &mut buffer).await?;
        measured.latencies.push(started.elapsed().as_micros());
        if status == 200 {
            measured.ok = measured.ok.saturating_add(1);
        } else {
            measured.errors = measured.errors.saturating_add(1);
        }
    }
    Ok(measured)
}

/// Read more bytes off `stream` into `buffer`.
async fn fill(stream: &mut TcpStream, buffer: &mut Vec<u8>) -> Result<(), LoadError> {
    let mut chunk = [0_u8; 4096];
    let read = stream.read(&mut chunk).await?;
    match chunk.get(..read) {
        Some(bytes) if !bytes.is_empty() => {
            buffer.extend_from_slice(bytes);
            Ok(())
        }
        _ => refused(LoadError::Malformed("the server closed mid-response")),
    }
}

/// The status and content length of a response head.
fn parse_head(head: &[u8]) -> Result<(u16, usize), LoadError> {
    let Ok(head) = std::str::from_utf8(head) else {
        return refused(LoadError::Malformed("head is not UTF-8"));
    };
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(LoadError::Malformed("no status"))?;
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .ok_or(LoadError::Malformed("no content-length"))?;
    Ok((status, length))
}

/// Read one response off `stream`; its status. `buffer` carries bytes past
/// the response into the next read.
async fn read_response(stream: &mut TcpStream, buffer: &mut Vec<u8>) -> Result<u16, LoadError> {
    loop {
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let Some(head) = buffer.get(..end) else {
                return refused(LoadError::Malformed("head out of range"));
            };
            let (status, length) = parse_head(head)?;
            let total = end.saturating_add(4).saturating_add(length);
            while buffer.len() < total {
                fill(stream, buffer).await?;
            }
            buffer.drain(..total);
            return Ok(status);
        }
        if buffer
            .get(MAX_HEAD_BYTES..)
            .is_some_and(|past| !past.is_empty())
        {
            return refused(LoadError::Malformed("head over the ceiling"));
        }
        fill(stream, buffer).await?;
    }
}
