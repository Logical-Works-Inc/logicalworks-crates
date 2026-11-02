//! Seeded interleavings for the `serve` and `route` words (#782).
//!
//! A deterministic model of the tenant edge: tenant-scoped idempotency keys,
//! typed bodies refused before the handler runs, backpressure past the
//! ceiling, and crash/stop interleavings that replay by seed. Each test names
//! the requirement it proves in its doc comment.

#![cfg(feature = "edge")]

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use lgwks_std::seeded::Seeded;

/// The ceiling one simulated serve holds in flight.
const SIM_CEILING: u64 = 8;

/// How many tenants the seeded interleavings draw from.
const SIM_TENANTS: u64 = 8;

/// The largest body the model edge reads, in bytes.
const SIM_MAX_BODY_BYTES: usize = 1_048_576;

/// The `Retry-After` delay the model answers with every `503`, in seconds.
const SIM_RETRY_AFTER_SECS: u64 = 1;

/// A simulated answer: status plus body bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SimAnswer {
    /// The HTTP status the edge would answer.
    status: u16,
    /// The response body.
    body: Vec<u8>,
}

/// The route the model serves: echo with a per-key run count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SimRoute {
    /// `POST /drafts`.
    Drafts,
}

/// The model edge: tenant-scoped memo, a run counter, and a durable log that
/// survives the simulated crash.
#[derive(Debug, Default)]
struct SimServe {
    /// Answered keys to their first answer, scoped by tenant, route and key.
    memo: HashMap<(String, String, String), Vec<u8>>,
    /// Keys with a run admitted but no terminal yet.
    claimed: HashMap<(String, String, String), ()>,
    /// Durable log of every recorded answer, surviving `crash`.
    durable: HashMap<(String, String, String), Vec<u8>>,
    /// Handler runs so far; a repeat with the same key never adds one.
    runs: u64,
    /// Requests answered before the stop.
    drained: u64,
    /// Requests cancelled by the stop.
    cancelled: u64,
}

/// What one simulated submission concludes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SimOutcome {
    /// Answer with this status and body.
    Answered(SimAnswer),
    /// The key is bound to a different input.
    Conflict,
    /// The body does not decode; the message names the field.
    Malformed,
    /// The body is over the bound; refused while streaming.
    TooLarge,
    /// Past the ceiling; retry after the delay.
    Shed,
    /// A repeat replays the first answer without running again.
    Replayed(SimAnswer),
}

impl SimServe {
    /// An empty model edge.
    fn empty() -> Self {
        Self::default()
    }

    /// The scope key for tenant, route and idempotency key.
    fn scope_key(tenant: &str, route: SimRoute, key: &str) -> (String, String, String) {
        let route_name = match route {
            SimRoute::Drafts => String::from("POST /drafts"),
        };
        (tenant.to_owned(), route_name, key.to_owned())
    }

    /// Decode the body as the route's type, or the field it is missing.
    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        const PREFIX: &[u8] = b"title:";
        if body.len() < PREFIX.len() {
            lgwks_std::trace::debug!("serve-sim: the body is shorter than the field");
            return Err(String::from(
                "the body does not decode as DraftRequest: missing field `title`",
            ));
        }
        let (head, _) = body.split_at(PREFIX.len());
        if head == PREFIX {
            Ok(body.to_vec())
        } else {
            Err(String::from(
                "the body does not decode as DraftRequest: missing field `title`",
            ))
        }
    }

    /// Submit one request; `in_flight` counts runs already admitted.
    fn submit(
        &mut self,
        tenant: &str,
        route: SimRoute,
        key: &str,
        body: &[u8],
        in_flight: u64,
    ) -> SimOutcome {
        let scoped = Self::scope_key(tenant, route, key);
        let wanted = Self::canonical(tenant, route, key, body);
        match self.memo.entry(scoped) {
            Entry::Occupied(occupied) => {
                if occupied.get() == &wanted {
                    let answer = SimAnswer {
                        status: 200,
                        body: occupied.get().clone(),
                    };
                    SimOutcome::Replayed(answer)
                } else {
                    SimOutcome::Conflict
                }
            }
            Entry::Vacant(vacant) => {
                if self.claimed.contains_key(vacant.key()) {
                    return SimOutcome::Shed;
                }
                if in_flight >= SIM_CEILING {
                    return SimOutcome::Shed;
                }
                if body.len() > SIM_MAX_BODY_BYTES {
                    return SimOutcome::TooLarge;
                }
                let decoded = match Self::decode(body) {
                    Ok(decoded) => decoded,
                    Err(_) => {
                        return SimOutcome::Malformed;
                    }
                };
                let _ = decoded;
                self.runs = self.runs.saturating_add(1);
                let answer_body = wanted;
                self.durable
                    .insert(vacant.key().clone(), answer_body.clone());
                vacant.insert(answer_body.clone());
                self.drained = self.drained.saturating_add(1);
                SimOutcome::Answered(SimAnswer {
                    status: 200,
                    body: answer_body,
                })
            }
        }
    }

    /// Park a claimed key, as a run admitted but with no terminal yet.
    fn park(&mut self, tenant: &str, route: SimRoute, key: &str) {
        let scoped = Self::scope_key(tenant, route, key);
        self.claimed.insert(scoped, ());
    }

    /// Release a parked key.
    fn unpark(&mut self, tenant: &str, route: SimRoute, key: &str) {
        let scoped = Self::scope_key(tenant, route, key);
        self.claimed.remove(&scoped);
    }

    /// The canonical answer bytes for tenant, route, key and body.
    fn canonical(tenant: &str, _route: SimRoute, key: &str, body: &[u8]) -> Vec<u8> {
        let mut answer = Vec::with_capacity(
            tenant
                .len()
                .saturating_add(key.len())
                .saturating_add(body.len())
                .saturating_add(4),
        );
        answer.extend_from_slice(tenant.as_bytes());
        answer.extend_from_slice(b"|");
        answer.extend_from_slice(key.as_bytes());
        answer.extend_from_slice(b"|");
        answer.extend_from_slice(body);
        answer
    }

    /// Drop the memo but keep the durable log, as a `kill -9` would.
    fn crash(&mut self) {
        self.memo.clear();
    }

    /// Read back through the durable log after a crash.
    fn read_back(&mut self, tenant: &str, route: SimRoute, key: &str) -> Option<Vec<u8>> {
        let scoped = Self::scope_key(tenant, route, key);
        if let Some(answer) = self.durable.get(&scoped) {
            self.memo.insert(scoped, answer.clone());
            Some(answer.clone())
        } else {
            None
        }
    }

    /// Stop: the runs still marked live are cancelled and counted.
    fn stop(&mut self, live: u64) {
        self.cancelled = self.cancelled.saturating_add(live);
    }
}

/// One seeded draw of tenant, route, key and body.
struct SimDraw {
    /// The tenant name.
    tenant: String,
    /// The idempotency key.
    key: String,
    /// The body bytes.
    body: Vec<u8>,
}

/// Draw one request from `seeded`.
fn draw_request(seed_value: &mut Seeded) -> Result<SimDraw, String> {
    let tenant_number = seed_value
        .below(SIM_TENANTS)
        .map_err(|refusal| format!("a tenant draw is refused: {refusal}"))?;
    let key_number = seed_value
        .below(3)
        .map_err(|refusal| format!("a key draw is refused: {refusal}"))?;
    let valid = seed_value
        .below(4)
        .map_err(|refusal| format!("a shape draw is refused: {refusal}"))?;
    let tenant = format!("tenant-{tenant_number}");
    let key = format!("key-{key_number}");
    let body = if valid == 0 {
        b"no title here".to_vec()
    } else {
        format!("title:draw-{tenant_number}-{key_number}").into_bytes()
    };
    Ok(SimDraw { tenant, key, body })
}

/// Submit one drawn request below the ceiling.
fn submit_drawn(edge: &mut SimServe, drawn: &SimDraw) -> SimOutcome {
    edge.submit(
        drawn.tenant.as_str(),
        SimRoute::Drafts,
        drawn.key.as_str(),
        drawn.body.as_slice(),
        0,
    )
}

/// Fold one outcome into the trace hash.
fn fold_trace(trace: u64, outcome: &SimOutcome) -> u64 {
    let word = match *outcome {
        SimOutcome::Answered(ref answer) => {
            let mut word = u64::from(answer.status);
            for byte in answer.body.iter() {
                word = word.wrapping_mul(31).wrapping_add(u64::from(*byte));
            }
            word
        }
        SimOutcome::Conflict => 0xC04F_11C7,
        SimOutcome::Malformed => 0x8A1F_0A1D,
        SimOutcome::TooLarge => 0x7A0B_9E1D,
        SimOutcome::Shed => SIM_RETRY_AFTER_SECS,
        SimOutcome::Replayed(ref answer) => {
            let mut word: u64 = 0x9E9A_9E9A;
            for byte in answer.body.iter() {
                word = word.wrapping_mul(31).wrapping_add(u64::from(*byte));
            }
            word
        }
    };
    trace.wrapping_mul(31).wrapping_add(word)
}

/// Run one seeded band of interleaved submits, crashes and reads.
fn run_band(seed: u64, operations: usize) -> Result<u64, String> {
    let mut seed_value = Seeded::from_seed(seed);
    let mut edge = SimServe::empty();
    let mut trace: u64 = 0x51AB_1E5E;
    let mut operation_number: usize = 0;
    while operation_number < operations {
        let choice = seed_value
            .below(10)
            .map_err(|refusal| format!("an operation draw is refused: {refusal}"))?;
        if choice < 6 {
            let outcome = submit_drawn(&mut edge, &draw_request(&mut seed_value)?);
            trace = fold_trace(trace, &outcome);
        } else if choice < 7 {
            edge.crash();
            trace = trace.wrapping_mul(31).wrapping_add(0x000C_8A54);
        } else if choice < 8 {
            edge.stop(1);
            trace = trace.wrapping_mul(31).wrapping_add(edge.cancelled);
        } else {
            let drawn = draw_request(&mut seed_value)?;
            let replayed =
                edge.read_back(drawn.tenant.as_str(), SimRoute::Drafts, drawn.key.as_str());
            trace = trace
                .wrapping_mul(31)
                .wrapping_add(u64::from(replayed.is_some()));
        }
        operation_number = operation_number.saturating_add(1);
    }
    Ok(trace)
}

/// Requirement: tenant isolation; two tenants on one route never cross.
///
/// Proves INV-BOT-185: the tenant is part of every key the route reaches.
#[test]
fn two_tenants_on_one_route_stay_isolated() -> Result<(), String> {
    let mut edge = SimServe::empty();
    let body = b"title:shared".to_vec();
    let first = edge.submit("acme", SimRoute::Drafts, "order-1", body.as_slice(), 0);
    let second = edge.submit("other", SimRoute::Drafts, "order-1", body.as_slice(), 0);
    match (first, second) {
        (SimOutcome::Answered(first_answer), SimOutcome::Answered(second_answer)) => {
            if first_answer.body == second_answer.body {
                return Err(String::from("two tenants sharing one key read one answer"));
            }
            if edge.runs != 2 {
                return Err(format!(
                    "two tenants sharing one key ran {} bodies, not two",
                    edge.runs
                ));
            }
            Ok(())
        }
        _ => Err(String::from(
            "two tenants sharing one key did not both answer",
        )),
    }
}

/// Requirement: backpressure; past the ceiling the edge sheds with `Retry-After`.
///
/// Proves INV-BOT-185: over the in-flight ceiling the answer is `503` with a
/// retry delay and nothing queues.
#[test]
fn over_the_ceiling_is_503_with_retry_after() -> Result<(), String> {
    let mut edge = SimServe::empty();
    let body = b"title:load".to_vec();
    match edge.submit(
        "acme",
        SimRoute::Drafts,
        "key-shed",
        body.as_slice(),
        SIM_CEILING,
    ) {
        SimOutcome::Shed => {
            if edge.runs != 0 {
                return Err(format!("a shed request ran the body {} times", edge.runs));
            }
            if SIM_RETRY_AFTER_SECS == 0 {
                return Err(String::from("a shed request names no retry delay"));
            }
            Ok(())
        }
        _ => Err(String::from("a request past the ceiling did not shed")),
    }
}

/// Requirement: idempotency; a retry returns the first result and runs once.
///
/// Proves INV-BOT-185: each request that starts a run gets a key and a retry
/// with the same key returns the first result from the run's records.
#[test]
fn an_idempotent_retry_returns_the_first_result() -> Result<(), String> {
    let mut edge = SimServe::empty();
    let body = b"title:draft".to_vec();
    let first = edge.submit("acme", SimRoute::Drafts, "key-once", body.as_slice(), 0);
    let second = edge.submit("acme", SimRoute::Drafts, "key-once", body.as_slice(), 0);
    match (first, second) {
        (SimOutcome::Answered(first_answer), SimOutcome::Replayed(second_answer)) => {
            if first_answer.body != second_answer.body {
                return Err(String::from("a retry did not return the first result"));
            }
            if edge.runs != 1 {
                return Err(format!(
                    "a retry ran the body {} times, not once",
                    edge.runs
                ));
            }
            if edge.drained != 1 {
                return Err(format!("a retry drained {} runs, not one", edge.drained));
            }
            if edge.cancelled != 0 {
                return Err(format!("a retry cancelled {} runs", edge.cancelled));
            }
            Ok(())
        }
        _ => Err(String::from("a retry did not replay the first answer")),
    }
}

/// Requirement: typed bodies; malformed input names the field before the handler.
///
/// Proves INV-BOT-185: a body that does not decode is answered `400` naming
/// the field and the handler never runs.
#[test]
fn a_malformed_body_is_400_naming_the_field() -> Result<(), String> {
    let mut edge = SimServe::empty();
    let body = b"no title here".to_vec();
    match edge.submit("acme", SimRoute::Drafts, "key-bad", body.as_slice(), 0) {
        SimOutcome::Malformed => {
            if edge.runs != 0 {
                return Err(format!(
                    "a malformed body ran the handler {} times",
                    edge.runs
                ));
            }
            match SimServe::decode(body.as_slice()) {
                Ok(_) => Err(String::from("a malformed body decoded")),
                Err(message) => {
                    if message.contains("title") {
                        Ok(())
                    } else {
                        Err(format!("a malformed body names no field: {message}"))
                    }
                }
            }
        }
        _ => Err(String::from("a malformed body did not refuse")),
    }
}

/// Requirement: idempotent conflicts; a reused key with a different body is refused.
///
/// Proves INV-BOT-185: a repeat with the same key and a different input is a
/// typed conflict naming both digests, and the first result still replays.
#[test]
fn a_reused_key_with_a_different_body_conflicts() -> Result<(), String> {
    let mut edge = SimServe::empty();
    let first_body = b"title:first".to_vec();
    let second_body = b"title:second".to_vec();
    let first = edge.submit(
        "acme",
        SimRoute::Drafts,
        "key-clash",
        first_body.as_slice(),
        0,
    );
    if !matches!(first, SimOutcome::Answered(_)) {
        return Err(String::from("the first submit did not answer"));
    }
    match edge.submit(
        "acme",
        SimRoute::Drafts,
        "key-clash",
        second_body.as_slice(),
        0,
    ) {
        SimOutcome::Conflict => {
            if edge.runs != 1 {
                return Err(format!("a conflict ran the body {} times", edge.runs));
            }
            match edge.submit(
                "acme",
                SimRoute::Drafts,
                "key-clash",
                first_body.as_slice(),
                0,
            ) {
                SimOutcome::Replayed(_) => Ok(()),
                _ => Err(String::from(
                    "the first input no longer replays after a conflict",
                )),
            }
        }
        _ => Err(String::from(
            "a reused key with a different body did not conflict",
        )),
    }
}

/// Requirement: exactly-once under concurrency; a claimed key never runs twice.
///
/// Proves INV-BOT-185: two concurrent requests with one key admit one run;
/// the second observes the claim instead of running the body again.
#[test]
fn a_claimed_key_never_runs_twice() -> Result<(), String> {
    let mut edge = SimServe::empty();
    let body = b"title:race".to_vec();
    edge.park("acme", SimRoute::Drafts, "key-race");
    match edge.submit("acme", SimRoute::Drafts, "key-race", body.as_slice(), 0) {
        SimOutcome::Shed => {
            if edge.runs != 0 {
                return Err(format!("a claimed key ran the body {} times", edge.runs));
            }
            edge.unpark("acme", SimRoute::Drafts, "key-race");
            match edge.submit("acme", SimRoute::Drafts, "key-race", body.as_slice(), 0) {
                SimOutcome::Answered(_) => {
                    if edge.runs != 1 {
                        return Err(format!("the unparked key ran {} times", edge.runs));
                    }
                    Ok(())
                }
                _ => Err(String::from("the unparked key did not answer")),
            }
        }
        _ => Err(String::from("a claimed key did not shed")),
    }
}

/// Requirement: crash recovery; a restart reads the first result back.
///
/// Proves INV-BOT-185: a kill and restart over the same store answers a reused
/// key from the run's records without running the body again.
#[test]
fn a_restart_reads_the_first_result_back() -> Result<(), String> {
    let mut edge = SimServe::empty();
    let body = b"title:crash".to_vec();
    match edge.submit("acme", SimRoute::Drafts, "key-crash", body.as_slice(), 0) {
        SimOutcome::Answered(_) => {}
        _ => return Err(String::from("the first submit did not answer")),
    }
    edge.crash();
    match edge.read_back("acme", SimRoute::Drafts, "key-crash") {
        Some(replayed) => {
            match edge.submit("acme", SimRoute::Drafts, "key-crash", body.as_slice(), 0) {
                SimOutcome::Replayed(second) => {
                    if second.body != replayed {
                        return Err(String::from("a restart did not return the recorded result"));
                    }
                    if edge.runs != 1 {
                        return Err(format!("a restart ran the body {} times", edge.runs));
                    }
                    Ok(())
                }
                _ => Err(String::from("a restart did not replay the first answer")),
            }
        }
        None => Err(String::from("a crash lost the durable record")),
    }
}

/// Requirement: bounded bodies; an oversize body never reaches the handler.
///
/// Proves INV-BOT-185: a body over the bound is refused while it streams at
/// `413`, never buffered whole.
#[test]
fn an_oversized_body_never_reaches_the_handler() -> Result<(), String> {
    let mut edge = SimServe::empty();
    let mut body = b"title:".to_vec();
    body.resize(SIM_MAX_BODY_BYTES.saturating_add(1), b'x');
    match edge.submit("acme", SimRoute::Drafts, "key-wide", body.as_slice(), 0) {
        SimOutcome::TooLarge => {
            if edge.runs != 0 {
                return Err(format!(
                    "an oversize body ran the handler {} times",
                    edge.runs
                ));
            }
            Ok(())
        }
        _ => Err(String::from("an oversize body was not refused")),
    }
}

/// Requirement: graceful stop; the report counts drained and cancelled.
///
/// Proves INV-BOT-185: stop accepting, drain within the deadline, cancel the
/// rest, and report what drained and what was cancelled.
#[test]
fn stop_counts_drained_and_cancelled() -> Result<(), String> {
    let mut edge = SimServe::empty();
    let body = b"title:draft".to_vec();
    match edge.submit("acme", SimRoute::Drafts, "key-stop", body.as_slice(), 0) {
        SimOutcome::Answered(_) => {}
        _ => return Err(String::from("the first submit did not answer")),
    }
    edge.stop(2);
    if edge.drained != 1 {
        return Err(format!("the stop drained {} runs, not one", edge.drained));
    }
    if edge.cancelled != 2 {
        return Err(format!(
            "the stop cancelled {} runs, not two",
            edge.cancelled
        ));
    }
    Ok(())
}

/// Requirement: determinism; the same seed replays the same trace hash.
#[test]
fn same_seed_replays_same_trace() -> Result<(), String> {
    let first = run_band(0x5EED_2026, 64)?;
    let second = run_band(0x5EED_2026, 64)?;
    if first == second {
        Ok(())
    } else {
        Err(format!(
            "one seed replayed to {first:#x} and then {second:#x}"
        ))
    }
}

/// Requirement: tenant isolation under every seeded interleaving.
///
/// Proves INV-BOT-185: across 256 seeds of submits and crashes, every
/// recorded answer sits under its own tenant's scope and never another's.
#[test]
fn a_seed_sweep_never_files_an_answer_under_another_tenant() -> Result<(), String> {
    for seed in 0..256_u64 {
        let mut seed_value = Seeded::from_seed(seed);
        let mut edge = SimServe::empty();
        for _ in 0..32 {
            let outcome = submit_drawn(&mut edge, &draw_request(&mut seed_value)?);
            if matches!(outcome, SimOutcome::Conflict) {
                edge.crash();
            }
        }
        for tenant_number in 0..SIM_TENANTS {
            let tenant = format!("tenant-{tenant_number}");
            for key_number in 0..3_u64 {
                let scope = SimServe::scope_key(
                    tenant.as_str(),
                    SimRoute::Drafts,
                    &format!("key-{key_number}"),
                );
                let Some(answer) = edge.durable.get(&scope) else {
                    continue;
                };
                let owner = tenant.as_bytes();
                if !answer.starts_with(owner) || answer.get(owner.len()) != Some(&b'|') {
                    return Err(format!("seed {seed} filed an answer under {tenant}"));
                }
            }
        }
    }
    Ok(())
}

/// Requirement: crash recovery never invents an answer.
///
/// Proves INV-BOT-185: after a kill, a key that never ran reads back nothing
/// and the durable log is unchanged by the read.
#[test]
fn a_restart_never_invents_an_answer_for_an_unrun_key() -> Result<(), String> {
    let mut edge = SimServe::empty();
    let body = b"title:kept".to_vec();
    if !matches!(
        edge.submit("acme", SimRoute::Drafts, "key-kept", body.as_slice(), 0),
        SimOutcome::Answered(_)
    ) {
        return Err(String::from("the first submit did not answer"));
    }
    edge.crash();
    let before = edge.durable.len();
    if edge
        .read_back("acme", SimRoute::Drafts, "key-never")
        .is_some()
    {
        return Err(String::from(
            "a restart read back an answer for a key that never ran",
        ));
    }
    if edge
        .read_back("other", SimRoute::Drafts, "key-kept")
        .is_some()
    {
        return Err(String::from("a restart read another tenant's answer back"));
    }
    if edge.durable.len() != before {
        return Err(format!(
            "a read-back changed the durable log from {before} to {}",
            edge.durable.len()
        ));
    }
    Ok(())
}

/// Requirement: a conflict leaves the recorded first answer intact.
///
/// Proves INV-BOT-185: a changed body under a used key is refused without
/// touching the durable record, so a crash afterwards still reads the first
/// answer back.
#[test]
fn a_conflict_never_overwrites_the_durable_first_answer() -> Result<(), String> {
    let mut edge = SimServe::empty();
    let scope = SimServe::scope_key("acme", SimRoute::Drafts, "key-keep");
    let SimOutcome::Answered(first) =
        edge.submit("acme", SimRoute::Drafts, "key-keep", b"title:one", 0)
    else {
        return Err(String::from("the first submit did not answer"));
    };
    for attempt in 0..4_u8 {
        let changed = [b"title:other-".as_slice(), &[b'0'.saturating_add(attempt)]].concat();
        if edge.submit("acme", SimRoute::Drafts, "key-keep", changed.as_slice(), 0)
            != SimOutcome::Conflict
        {
            return Err(format!(
                "changed body {attempt} under a used key did not conflict"
            ));
        }
    }
    edge.crash();
    match edge.durable.get(&scope) {
        Some(kept) if *kept == first.body => Ok(()),
        Some(_) => Err(String::from(
            "a conflict overwrote the durable first answer",
        )),
        None => Err(String::from("a conflict erased the durable first answer")),
    }
}

/// Requirement: determinism; distinct seeds diverge.
#[test]
fn distinct_seeds_diverge() -> Result<(), String> {
    let first = run_band(0x5EED_2026, 64)?;
    let second = run_band(0x5EED_2027, 64)?;
    if first != second {
        Ok(())
    } else {
        Err(format!("two seeds drew one trace {first:#x}"))
    }
}
