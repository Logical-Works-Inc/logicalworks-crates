//! Seeded runs of the real `rt::serve` edge over loopback (#1008).
//!
//! The model in `sim_serve` is a pure memo. These tests drive the shipped
//! `serve` entry point instead, so a change to how a request reaches its host
//! (the blocking-pool hop, the host table, the route table) is seen through
//! the answers a client reads. The oracle is a reference model written in this
//! file, never the edge's own reply.

#![cfg(feature = "edge")]

use std::collections::HashMap;
use std::io::Write as _;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use lgwks_bot::rt::serve::{RouteMethod, TypedRoute};
use lgwks_std::seeded::Seeded;

use crate::scratch::Scratch;
use crate::serve_edge::{
    Gate, await_entered, counted, counting_route, parse_count, post, with_serve,
};

/// Tenants one band draws from.
const TENANTS: u64 = 4;

/// Explicit keys one band draws from; one more draw means no key at all.
const KEYS: u64 = 3;

/// Request bodies one band draws from.
const BODIES: [&[u8]; 2] = [b"{\"n\":0}", b"{\"n\":1}"];

/// What the reference model says one request must answer.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// `200`, and the tenant's run count at the first run of this key.
    Count(u64),
    /// `422`: the explicit key is bound to the other body.
    Conflict,
}

/// The model: per tenant, each key's first body and the count it answered.
#[derive(Default)]
struct Model {
    /// `(tenant, key) -> (body variant, count answered)`.
    first: HashMap<(u64, String), (usize, u64)>,
    /// Runs per tenant so far.
    runs: HashMap<u64, u64>,
}

impl Model {
    /// What `tenant` posting `body` under `key` (`None` for keyless) must see.
    fn judge(&mut self, tenant: u64, key: Option<u64>, body: usize) -> Verdict {
        let name = match key {
            Some(number) => format!("key-{number}"),
            None => format!("auto-{body}"),
        };
        if let Some(&(held, count)) = self.first.get(&(tenant, name.clone())) {
            return if held == body {
                Verdict::Count(count)
            } else {
                Verdict::Conflict
            };
        }
        let runs = self.runs.entry(tenant).or_insert(0);
        *runs = runs.saturating_add(1);
        let count = *runs;
        self.first.insert((tenant, name), (body, count));
        Verdict::Count(count)
    }

    /// Runs the whole band should have made.
    fn total_runs(&self) -> u64 {
        self.runs.values().sum()
    }
}

/// One seeded draw below `bound`.
fn draw(draws: &mut Seeded, bound: u64) -> Result<u64, String> {
    draws
        .below(bound)
        .map_err(|refusal| format!("a draw is refused: {refusal}"))
}

/// How many times the retry of a held key asks before it gives up.
const RETRIES: usize = 50_000;

/// `Ok` when the edge agreed with the model, else the message `what` builds.
fn agree(agreed: bool, what: impl FnOnce() -> String) -> Result<(), String> {
    agreed.then_some(()).ok_or_else(what)
}

/// One band of `requests` seeded requests; the trace hash of every answer.
fn run_band(seed: u64, requests: usize) -> Result<u64, String> {
    let (store, counter) = counted("sim-serve-hop")?;
    let route = counting_route(&counter);
    let (trace, _report) = with_serve(std::vec![route], &store, 1_000, |addr, _stop| {
        let mut draws = Seeded::from_seed(seed);
        let mut model = Model::default();
        let mut trace: u64 = 0x51AB_1E5E;
        for _ in 0..requests {
            let tenant = draw(&mut draws, TENANTS)?;
            let key = draw(&mut draws, KEYS.saturating_add(1))?;
            let key = (key < KEYS).then_some(key);
            let body = usize::try_from(draw(&mut draws, 2)?)
                .map_err(|refusal| format!("a body draw does not fit: {refusal}"))?;
            let bytes = BODIES
                .get(body)
                .copied()
                .ok_or_else(|| String::from("the body draw is outside the table"))?;
            let wanted = model.judge(tenant, key, body);
            let name = format!("t{tenant}");
            let key_text = key.map(|number| format!("key-{number}"));
            let (status, _, answer) = post(addr, &name, "/drafts", bytes, key_text.as_deref())?;
            match wanted {
                Verdict::Count(count) => {
                    agree(status == 200 && parse_count(&answer)? == count, || {
                        format!(
                            "seed {seed}: {name} {key_text:?} answered {status} {answer:?}, \
                             the model says count {count}"
                        )
                    })?
                }
                Verdict::Conflict => agree(status == 422, || {
                    format!("seed {seed}: {name} {key_text:?} answered {status}, not 422")
                })?,
            }
            trace = answer.bytes().fold(
                trace.wrapping_mul(31).wrapping_add(u64::from(status)),
                |hash, byte| hash.wrapping_mul(31).wrapping_add(u64::from(byte)),
            );
        }
        let ran = u64::try_from(counter.runs())
            .map_err(|refusal| format!("the run count does not fit: {refusal}"))?;
        agree(ran == model.total_runs(), || {
            format!(
                "seed {seed}: the edge ran {ran} bodies, the model says {}",
                model.total_runs()
            )
        })?;
        Ok(trace)
    })?;
    Ok(trace)
}

/// Requirement: a retry answers from the recorded reply and runs once, with the
/// request driven on the worker that read it (#1008).
///
/// Catches a hop or a host table that runs a body twice or files an answer under
/// the wrong tenant: the model counts runs per tenant and the edge must agree.
#[test]
fn sim_serve_hop_matches_the_model() -> Result<(), String> {
    for seed in 0..4_u64 {
        run_band(0x5EED_1008_u64.wrapping_add(seed), 40)?;
    }
    Ok(())
}

/// Requirement: determinism; one seed, one trace hash (#1008).
#[test]
fn sim_serve_hop_same_seed_same_trace() -> Result<(), String> {
    let first = run_band(0x5EED_1008, 40)?;
    let second = run_band(0x5EED_1008, 40)?;
    if first == second {
        Ok(())
    } else {
        Err(format!("one seed traced {first:#x} and then {second:#x}"))
    }
}

/// Requirement: determinism; distinct seeds diverge (#1008).
#[test]
fn sim_serve_hop_distinct_seeds_diverge() -> Result<(), String> {
    let first = run_band(0x5EED_1008, 40)?;
    let second = run_band(0x5EED_1009, 40)?;
    if first != second {
        Ok(())
    } else {
        Err(format!("two seeds traced one hash {first:#x}"))
    }
}

/// Requirement: a client that hangs up mid-run does not abandon the run (#1008).
///
/// The run awaits its append through a future whose drop poisons the tenant's
/// store handle. The run is owned by the serve, not by the connection, so the
/// hang-up must leave the run to finish, record its reply, and answer the retry
/// of the same key from that record without running the body again.
#[test]
fn a_client_that_leaves_mid_run_does_not_abandon_the_run() -> Result<(), String> {
    let store = Scratch::new("serve-hangup").map_err(|error| error.to_string())?;
    let gate = Arc::new(Gate::default());
    let entered = Arc::new(AtomicBool::new(false));
    let runs = Arc::new(AtomicUsize::new(0));
    let parked = Arc::clone(&gate);
    let signalled = Arc::clone(&entered);
    let tallied = Arc::clone(&runs);
    let route = TypedRoute::new(
        RouteMethod::Post,
        "/park",
        move |_tenant: String, _body: Vec<u8>| {
            let parked = Arc::clone(&parked);
            let signalled = Arc::clone(&signalled);
            let tallied = Arc::clone(&tallied);
            async move {
                tallied.fetch_add(1, Ordering::SeqCst);
                signalled.store(true, Ordering::SeqCst);
                parked.pass().map_err(|message| (500u16, message))
            }
        },
    );
    let (answer, _report) = with_serve(std::vec![route], &store, 8, |addr, _stop| {
        let head = format!(
            "POST /t/acme/park HTTP/1.1\r\nhost: {addr}\r\ncontent-length: 4\r\n\
             idempotency-key: leaver-1\r\n\r\npark"
        );
        let mut leaver = TcpStream::connect(addr).map_err(|error| error.to_string())?;
        leaver
            .write_all(head.as_bytes())
            .map_err(|error| error.to_string())?;
        await_entered(&entered)?;
        drop(leaver);
        gate.release()?;
        let mut last = 0_u16;
        for _ in 0..RETRIES {
            let (status, _, body) = post(addr, "acme", "/park", b"park", Some("leaver-1"))?;
            if status == 200 {
                return Ok(body);
            }
            last = status;
            agree(status == 503, || {
                format!("the retry answered {status}, not 200 or 503")
            })?;
            std::thread::yield_now();
        }
        Err(format!("{RETRIES} retries still answered {last}, not 200"))
    })?;
    if answer != "released" {
        return Err(format!("the retry read {answer:?}, not the recorded reply"));
    }
    if runs.load(Ordering::SeqCst) != 1 {
        return Err(format!(
            "the body ran {} times, not once",
            runs.load(Ordering::SeqCst)
        ));
    }
    Ok(())
}
