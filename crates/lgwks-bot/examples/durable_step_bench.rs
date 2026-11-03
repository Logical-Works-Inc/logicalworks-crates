//! One durable step, measured two ways on a matched workload (#850).
//!
//! A durable step here is one acknowledged, sealed, hash-chained append of a
//! 1 KiB payload. The same step count, payload size, payload uniqueness and
//! concurrency drive both sides:
//!
//! ```text
//! # in process: a sealed `RunStore` append, the estate owner of the step
//! /usr/bin/time -l cargo run --locked --release -p lgwks_bot --features sealed \
//!     --example durable_step_bench -- inproc <steps> <concurrency>
//!
//! # over HTTP: `POST /v1/syscall` of the legacy kernel, one bearer token per
//! # principal, one token per line in the file BENCH_TOKENS names
//! BENCH_TOKENS=<file> /usr/bin/time -l cargo run --locked --release -p lgwks_bot \
//!     --features sealed --example durable_step_bench -- http <host:port> <steps> <concurrency>
//! ```
//!
//! Every worker owns one thread and, for `http`, one keep-alive connection;
//! worker `w` takes steps `w`, `w + concurrency`, and so on, so the step set
//! is the same whatever the concurrency. Latency is microseconds per step.
//! Peak RSS is read externally (`/usr/bin/time -l`) for the server and for
//! this driver separately, never from inside: a process id is an identity the
//! OS reuses, which the estate's std-first gate refuses.
//!
//! This is a measurement, not a gate, so it prints. It compares the in-process
//! sealed append with the HTTP syscall endpoint of the legacy kernel. The
//! endpoint limits each principal separately, so `http` spreads the steps over
//! every token it is given. After an `http` run it asks for every principal's
//! fact count and sums them, so a step the server acknowledged and then lost
//! shows as a shortfall.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lgwks_bot::effect::RunId;
use lgwks_bot::script::{Scope, Tenant};
use lgwks_bot::sealed::keys::{KeyProvider, SoftwareProvider};
use lgwks_bot::task::{DefinitionIdentity, RunStore, SealedAppend};

#[path = "support/measure.rs"]
mod measure;

/// The payload every step carries, in bytes.
const PAYLOAD_BYTES: usize = 1024;
/// The stack of one worker thread: enough for the exchange, small enough for
/// a thousand of them.
const WORKER_STACK: usize = 512 * 1024;
/// How long one socket read or write may wait before the step is an error.
const IO_BOUND: Duration = Duration::from_secs(60);

/// What one worker saw.
struct Tally {
    /// Microseconds of every acknowledged step.
    latencies: Vec<u128>,
    /// Steps refused, failed or never answered.
    errors: usize,
    /// The first reason a step failed, so a shortfall is explained.
    first_error: Option<String>,
}

/// The payload of step `index`: sixteen hex digits that make it unique, then
/// a constant fill to [`PAYLOAD_BYTES`].
fn payload(index: usize) -> Vec<u8> {
    let mut bytes = format!("{index:016x}").into_bytes();
    bytes.resize(PAYLOAD_BYTES, 0x5A);
    bytes
}

/// One report line through the caller's locked stdout.
fn say(line: &str) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}").map_err(|error| format!("the report line is refused: {error}"))?;
    out.flush()
        .map_err(|error| format!("the report line is refused: {error}"))
}

/// Run `work` on `concurrency` workers and fold what they saw.
fn drive<F>(steps: usize, concurrency: usize, work: F) -> Result<(Tally, Duration), String>
where
    F: Fn(usize) -> Result<Tally, String> + Sync,
{
    let work = &work;
    let started = Instant::now();
    let tallies = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(concurrency);
        for worker in 0..concurrency {
            let spawned = std::thread::Builder::new()
                .stack_size(WORKER_STACK)
                .spawn_scoped(scope, move || work(worker));
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    lgwks_std::trace::debug!(error = ?error, "durable-step: a worker did not start");
                    return Err(format!("worker {worker} did not start: {error}"));
                }
            }
        }
        let mut tallies = Vec::with_capacity(concurrency);
        for handle in handles {
            match handle.join() {
                Ok(tally) => tallies.push(tally?),
                Err(_) => {
                    lgwks_std::trace::debug!("durable-step: a worker panicked");
                    return Err(String::from("a worker panicked"));
                }
            }
        }
        Ok(tallies)
    })?;
    let wall = started.elapsed();
    let mut folded = Tally {
        latencies: Vec::with_capacity(steps),
        errors: 0,
        first_error: None,
    };
    for mut tally in tallies {
        folded.errors = folded.errors.saturating_add(tally.errors);
        if folded.first_error.is_none() {
            folded.first_error = tally.first_error.take();
        }
        folded.latencies.append(&mut tally.latencies);
    }
    Ok((folded, wall))
}

/// Print one side's numbers.
fn report(
    side: &str,
    steps: usize,
    concurrency: usize,
    tally: &mut Tally,
    wall: Duration,
) -> Result<(), String> {
    let ok = tally.latencies.len();
    let ok_32 = u32::try_from(ok).map_err(|error| format!("the count does not fit: {error}"))?;
    let rps = f64::from(ok_32) / wall.as_secs_f64();
    let summary = measure::Summary::of(&mut tally.latencies);
    say(&format!(
        "{} steps={steps} concurrency={concurrency} ok={ok} errors={} wall={:.3}s rps={rps:.0} first_error={:?}",
        summary.line(side),
        tally.errors,
        wall.as_secs_f64(),
        tally.first_error,
    ))
}

/// The sealed `RunStore` append, `concurrency` workers on one shared store.
fn inproc(steps: usize, concurrency: usize) -> Result<(), String> {
    let unique: [u8; 8] = lgwks_std::random::bytes()
        .map_err(|error| format!("the scratch entropy is refused: {error}"))?;
    let hex: String = unique.iter().map(|byte| format!("{byte:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("lgwks-durable-step-{hex}"));
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("the scratch dir is refused: {error}"))?;
    let provider: Arc<dyn KeyProvider> = Arc::new(
        SoftwareProvider::open(&dir.join("keys"))
            .map_err(|error| format!("the key provider is refused: {error}"))?,
    );
    let store = RunStore::open_sealed(dir.join("store.runstore"), provider)
        .map_err(|error| format!("the store is refused: {error}"))?;
    let definition = DefinitionIdentity::new(
        "durable-step",
        1,
        lgwks_std::hash::blake3(b"durable-step"),
        1,
    );
    let tenant = Tenant::new("bench").map_err(|error| format!("the tenant is refused: {error}"))?;
    let scope = Scope::root(tenant);
    let store = &store;
    let definition = &definition;
    let scope = &scope;
    let (mut tally, wall) = drive(steps, concurrency, move |worker| {
        let mut tally = Tally {
            latencies: Vec::with_capacity(steps.div_ceil(concurrency)),
            errors: 0,
            first_error: None,
        };
        for index in (worker..steps).step_by(concurrency) {
            let run = RunId::from_hex(&format!("{:032x}", index.saturating_add(1)))
                .map_err(|error| format!("the run id is refused: {error}"))?;
            let path = format!("bench/{index}");
            let key = scope
                .enter(&path)
                .map_err(|error| format!("the step key is refused: {error}"))?
                .key();
            let bytes = payload(index);
            let started = Instant::now();
            match store.append_with_expiry(SealedAppend::new(
                "bench", run, key, &path, definition, bytes, None,
            )) {
                Ok(_) => tally.latencies.push(started.elapsed().as_micros()),
                Err(error) => {
                    tally.errors = tally.errors.saturating_add(1);
                    tally.first_error.get_or_insert_with(|| error.to_string());
                }
            }
        }
        Ok(tally)
    })?;
    store
        .verify_sealed_chain()
        .map_err(|error| format!("the sealed chain does not verify: {error}"))?;
    report("inproc_sealed_append", steps, concurrency, &mut tally, wall)?;
    if std::fs::remove_dir_all(&dir).is_err() {
        lgwks_std::trace::debug!("durable-step: the scratch store was not removed");
    }
    Ok(())
}

/// One HTTP answer: its status and its body.
fn read_answer(reader: &mut BufReader<TcpStream>) -> Result<(u16, Vec<u8>), String> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|error| format!("the status line is unreadable: {error}"))?;
    let status: u16 = line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| format!("not an HTTP status line: {line:?}"))?
        .parse()
        .map_err(|error| format!("the status is not a number: {error}"))?;
    let mut length = 0_usize;
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| format!("a header is unreadable: {error}"))?;
        if read == 0 {
            lgwks_std::trace::debug!("durable-step: the connection ended inside the headers");
            return Err(String::from("the connection ended inside the headers"));
        }
        let header = line.trim_end().to_ascii_lowercase();
        if header.is_empty() {
            break;
        }
        if let Some(value) = header.strip_prefix("content-length:") {
            length = value
                .trim()
                .parse()
                .map_err(|error| format!("the content length is not a number: {error}"))?;
        }
    }
    let mut body = vec![0_u8; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| format!("the body is unreadable: {error}"))?;
    Ok((status, body))
}

/// One `POST /v1/syscall` on an open connection.
fn post(
    reader: &mut BufReader<TcpStream>,
    writer: &mut TcpStream,
    addr: &str,
    token: &str,
    body: &str,
) -> Result<(u16, Vec<u8>), String> {
    let head = format!(
        "POST /v1/syscall HTTP/1.1\r\nhost: {addr}\r\nauthorization: Bearer {token}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    writer
        .write_all(head.as_bytes())
        .and_then(|()| writer.write_all(body.as_bytes()))
        .map_err(|error| format!("the request is refused: {error}"))?;
    read_answer(reader)
}

/// Open one keep-alive connection, bounded in both directions.
fn connect(addr: &str) -> Result<(BufReader<TcpStream>, TcpStream), String> {
    let stream = TcpStream::connect(addr).map_err(|error| format!("connect {addr}: {error}"))?;
    stream
        .set_nodelay(true)
        .and_then(|()| stream.set_read_timeout(Some(IO_BOUND)))
        .and_then(|()| stream.set_write_timeout(Some(IO_BOUND)))
        .map_err(|error| format!("the socket is refused its bounds: {error}"))?;
    let reader = stream
        .try_clone()
        .map_err(|error| format!("the socket is not cloneable: {error}"))?;
    Ok((BufReader::new(reader), stream))
}

/// The append body for step `index`.
fn append_body(index: usize) -> String {
    format!(
        "{{\"call\":{{\"op\":\"append\",\"payload_b64\":\"{}\"}}}}",
        lgwks_std::encoding::base64::encode(payload(index))
    )
}

/// The count a `fact_count` answer carries, read from its `"count":N` field.
fn count_in(body: &[u8]) -> Option<u64> {
    let text = String::from_utf8_lossy(body);
    let (_, rest) = text.split_once("\"count\":")?;
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// The `POST /v1/syscall` append, `concurrency` keep-alive connections. Step
/// `index` is sent as principal `index % tokens.len()`, because the endpoint
/// rate-limits each principal on its own.
fn http(addr: &str, tokens: &[String], steps: usize, concurrency: usize) -> Result<(), String> {
    let (mut tally, wall) = drive(steps, concurrency, |worker| {
        let (mut reader, mut writer) = connect(addr)?;
        let mut tally = Tally {
            latencies: Vec::with_capacity(steps.div_ceil(concurrency)),
            errors: 0,
            first_error: None,
        };
        for index in (worker..steps).step_by(concurrency) {
            let body = append_body(index);
            let token = index
                .checked_rem(tokens.len())
                .and_then(|slot| tokens.get(slot))
                .ok_or_else(|| String::from("the token set is empty"))?;
            let started = Instant::now();
            match post(&mut reader, &mut writer, addr, token, &body) {
                Ok((200, _)) => tally.latencies.push(started.elapsed().as_micros()),
                Ok((status, answer)) => {
                    tally.errors = tally.errors.saturating_add(1);
                    tally.first_error.get_or_insert_with(|| {
                        format!("status {status}: {}", String::from_utf8_lossy(&answer))
                    });
                }
                Err(error) => {
                    tally.first_error.get_or_insert_with(|| error.clone());
                    lgwks_std::trace::debug!(error = ?error, "durable-step: the connection failed");
                    // The step in flight and every one after it never ran.
                    let left = (index..steps).step_by(concurrency).count();
                    tally.errors = tally.errors.saturating_add(left);
                    break;
                }
            }
        }
        Ok(tally)
    })?;
    report("http_syscall_append", steps, concurrency, &mut tally, wall)?;
    let (mut reader, mut writer) = connect(addr)?;
    let (mut total, mut refused) = (0_u64, 0_usize);
    for token in tokens {
        let (status, body) = post(
            &mut reader,
            &mut writer,
            addr,
            token,
            "{\"call\":{\"op\":\"fact_count\"}}",
        )?;
        match (status, count_in(&body)) {
            (200, Some(count)) => total = total.saturating_add(count),
            _ => refused = refused.saturating_add(1),
        }
    }
    say(&format!(
        "fact_count: principals={} total={total} unreadable={refused}",
        tokens.len()
    ))
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let number = |text: &String| -> Result<usize, String> {
        text.parse::<usize>()
            .map_err(|error| format!("{text:?} is not a count: {error}"))
    };
    match *args.as_slice() {
        [ref mode, ref steps, ref concurrency] if mode == "inproc" => {
            let (steps, concurrency) = (number(steps)?, number(concurrency)?);
            if concurrency == 0 {
                lgwks_std::trace::debug!("durable-step: concurrency 0 is refused");
                return Err(String::from("concurrency is at least 1"));
            }
            inproc(steps, concurrency)
        }
        [ref mode, ref addr, ref steps, ref concurrency] if mode == "http" => {
            let (steps, concurrency) = (number(steps)?, number(concurrency)?);
            if concurrency == 0 {
                lgwks_std::trace::debug!("durable-step: concurrency 0 is refused");
                return Err(String::from("concurrency is at least 1"));
            }
            let path = std::env::var("BENCH_TOKENS")
                .map_err(|error| format!("BENCH_TOKENS is not readable: {error}"))?;
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("the token file {path:?} is unreadable: {error}"))?;
            let tokens: Vec<String> = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect();
            if tokens.is_empty() {
                lgwks_std::trace::debug!(path = ?path, "durable-step: the token file is empty");
                return Err(format!("the token file {path:?} holds no token"));
            }
            http(addr, &tokens, steps, concurrency)
        }
        _ => Err(String::from(
            "usage: durable_step_bench inproc <steps> <concurrency> | http <host:port> <steps> <concurrency>",
        )),
    }
}
