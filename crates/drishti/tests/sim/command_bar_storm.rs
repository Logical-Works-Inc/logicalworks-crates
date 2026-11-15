//! Seeded command-bar storm: generated inputs flow parse-to-emit against a
//! frozen broker, with overlong, unknown, and empty inputs as faults.
//!
//! Same seed replays the same trace hash over parse and emit outcomes.

use std::collections::BTreeMap;

use drishti::native_host::broker::{Broker, BrokerError, IntentHandle};
use drishti::native_host::command_bar::IntentPipeline;
use drishti::native_host::protocol::{JsonValue, PartialIntent};

use crate::broker_shared::Harness;
use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// Frozen broker with the intent grant.
fn bar(harness: &Harness) -> Result<IntentHandle, String> {
    harness
        .broker()
        .register_command_bar(crate::fixtures::session("tenant")?)
        .map_err(|error| format!("{error}"))
}

/// Pick one head and append one optional spaced tail.
fn headed(random: &mut SimRng, heads: &[&str]) -> String {
    // `heads` is never empty and `below_len` stays below its length, so
    // the index always exists.
    let head = heads[random.below_len(heads.len())];
    let mut text = String::from(head);
    if random.below(2) == 0 {
        text.push(' ');
        let width = random.below_len(24);
        text.push_str(&random.text(width));
    }
    text
}

/// Parse one input, emit it on success, and hash every outcome.
fn parse_emit(broker: &Broker, bar: &IntentHandle, trace: &mut TraceHash, input: &str) {
    match IntentPipeline::parse(input) {
        Err(error) => trace.mix_str(&format!("{error:?}")),
        Ok(partial) => {
            trace.mix_str(partial.kind());
            match broker.emit_intent(bar, partial) {
                Ok(envelope) => {
                    trace.mix_u64(1);
                    trace.mix_str(envelope.kind());
                }
                Err(error) => trace.mix_str(&format!("{error:?}")),
            }
        }
    }
}

/// Seeded command text: slashes, words, spacings, and overlong tails.
fn command(random: &mut SimRng) -> String {
    const HEADS: &[&str] = &[
        "/find", "/pin", "/unpin", "/help", "/exec", "/", "", "find", "/FIND",
    ];
    let mut text = headed(random, HEADS);
    if random.below(12) == 0 {
        text.push_str(&"x".repeat(600));
    }
    text
}

/// Run one storm from `seed` and hash parse/emit outcomes.
fn run_storm(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::valid();
    let bar = bar(&harness)?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    for _ in 0..250 {
        let input = command(&mut random);
        parse_emit(harness.broker(), &bar, &mut trace, &input);
    }
    Ok(trace.finish())
}

/// Same seed replays the same hash.
#[test]
fn sim_command_bar_storm_replays_identically() -> Result<(), String> {
    for seed in [31_u64, 47, 2026] {
        if run_storm(seed)? != run_storm(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_storm(31)? == run_storm(32)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}

/// Run one frozen rate-limit campaign from `seed`: seven identical intents
/// admit five and limit two, then seeded follow-ups only ever allow, limit,
/// or report a malformed kind.
fn run_limits(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::valid();
    let bar = bar(&harness)?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let intent = || {
        PartialIntent::new(
            String::from("intent.find"),
            JsonValue::Object(BTreeMap::new()),
            String::from("acme"),
        )
    };
    let mut allowed = 0_usize;
    let mut limited = 0_usize;
    for _ in 0..7 {
        match harness.broker().emit_intent(&bar, intent()) {
            Ok(_) => {
                allowed = allowed.saturating_add(1);
                trace.mix_u64(1);
            }
            Err(BrokerError::RateLimited) => {
                limited = limited.saturating_add(1);
                trace.mix_u64(2);
            }
            Err(error) => ensure(false, || format!("unexpected limit outcome: {error}"))?,
        }
    }
    ensure(allowed == 5 && limited == 2, || {
        format!(
            "seven frozen intents must allow 5 and limit 2, allowed {allowed} limited {limited}"
        )
    })?;
    // The bucket is exhausted: seeded follow-ups never allow again.
    let kinds = ["intent.find", "", "intent.pin"];
    let mut followed = 0_usize;
    for _ in 0..50 {
        let kind = random
            .pick_str(&kinds)
            .ok_or_else(|| String::from("kinds must not be empty"))?;
        let partial = PartialIntent::new(
            String::from(kind),
            JsonValue::Object(BTreeMap::new()),
            String::from("acme"),
        );
        match harness.broker().emit_intent(&bar, partial) {
            Ok(envelope) => {
                trace.mix_u64(1);
                trace.mix_str(envelope.kind());
            }
            Err(BrokerError::RateLimited) => trace.mix_u64(2),
            Err(BrokerError::MalformedKind) => trace.mix_u64(3),
            Err(error) => ensure(false, || format!("unexpected follow-up outcome: {error}"))?,
        }
        followed = followed.saturating_add(1);
    }
    ensure(followed == 50, || {
        String::from("every follow-up must resolve")
    })?;
    Ok(trace.finish())
}

/// Same seed replays the same limit hash with identical allow/limit counts.
#[test]
fn sim_command_bar_rate_limit_replays_identically() -> Result<(), String> {
    for seed in [19_u64, 61, 1_409] {
        if run_limits(seed)? != run_limits(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_limits(19)? == run_limits(20)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}

/// Seeded faulty command text: empty inputs, bare slashes, unknown verbs, and
/// overlong tails.
fn faulty_command(random: &mut SimRng) -> String {
    const HEADS: &[&str] = &["/find", "/pin", "/nope", "/", "", "/FIND", "/exec"];
    let mut text = headed(random, HEADS);
    if random.below(5) == 0 {
        text.push_str(&"x".repeat(600));
    }
    text
}

/// Run one parse-fault campaign from `seed`: every seeded input either fails
/// to parse with a typed error or emits a parsed intent; nothing else escapes.
fn run_parse_faults(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::valid();
    let bar = bar(&harness)?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut resolved = 0_usize;
    for _ in 0..250 {
        let input = faulty_command(&mut random);
        parse_emit(harness.broker(), &bar, &mut trace, &input);
        resolved = resolved.saturating_add(1);
    }
    ensure(resolved == 250, || String::from("every input must resolve"))?;
    Ok(trace.finish())
}

/// Same seed replays the same parse-fault hash.
#[test]
fn sim_command_bar_parse_faults_replay_identically() -> Result<(), String> {
    for seed in [23_u64, 71, 1_707] {
        if run_parse_faults(seed)? != run_parse_faults(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_parse_faults(23)? == run_parse_faults(24)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}
