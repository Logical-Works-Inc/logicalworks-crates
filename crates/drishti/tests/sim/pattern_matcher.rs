//! Seeded pattern corpus: generated globs, kinds, and predicate trees are
//! matched for crashes and hashed for replay; invalid shapes never match.
//!
//! Same seed replays the same trace hash.

use drishti::native_host::pattern::{match_pattern, validate_pattern};
use drishti::native_host::protocol::{JsonValue, PayloadPredicate, SignalEnvelope, SignalPattern};

use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// Seeded kind segment.
fn segment(random: &mut SimRng) -> String {
    const SEGMENTS: &[&str] = &["a", "b", "forms", "x1", "", "*", "**", "tool-id", "9lives"];
    if let Some(segment) = random.pick_str(SEGMENTS) {
        String::from(segment)
    } else {
        String::new()
    }
}

/// Seeded dotted string of 1-5 segments.
fn dotted(random: &mut SimRng, max_segments: usize) -> String {
    let count = random.below_len(max_segments).saturating_add(1);
    let mut parts = Vec::new();
    for _ in 0..count {
        parts.push(segment(random));
    }
    parts.join(".")
}

/// Seeded JSON scalar.
fn scalar(random: &mut SimRng) -> JsonValue {
    match random.below(5) {
        0 => JsonValue::String(random.text(6)),
        1 => JsonValue::Number(f64::from(random.below_u32(100)) - 50.0),
        2 => JsonValue::Bool(random.below(2) == 1),
        3 => JsonValue::Null,
        _ => JsonValue::Number(f64::from(random.below_u32(1_000_000))),
    }
}

/// Seeded predicate tree; recursion stops at depth two.
fn predicate(random: &mut SimRng, depth: usize) -> PayloadPredicate {
    let keypath = dotted(random, 3);
    if depth >= 2 {
        return match random.below(4) {
            0 => PayloadPredicate::eq(keypath, scalar(random)),
            1 => PayloadPredicate::neq(keypath, scalar(random)),
            2 => PayloadPredicate::is_in(
                keypath,
                vec![scalar(random), scalar(random), scalar(random)],
            ),
            _ => PayloadPredicate::exists(keypath),
        };
    }
    match random.below(6) {
        0 => PayloadPredicate::eq(keypath, scalar(random)),
        1 => PayloadPredicate::neq(keypath, scalar(random)),
        2 => PayloadPredicate::is_in(
            keypath,
            vec![scalar(random), scalar(random), scalar(random)],
        ),
        3 => PayloadPredicate::exists(keypath),
        4 => PayloadPredicate::And(vec![
            predicate(random, depth.saturating_add(1)),
            predicate(random, depth.saturating_add(1)),
        ]),
        _ => PayloadPredicate::Or(vec![
            predicate(random, depth.saturating_add(1)),
            predicate(random, depth.saturating_add(1)),
        ]),
    }
}

/// Build one seeded pattern plus its kind.
fn seeded_pattern(random: &mut SimRng) -> (SignalPattern, String) {
    let glob = dotted(random, 5);
    let kind = dotted(random, 5);
    let with_predicate = random.below(2) == 1;
    let pattern = SignalPattern::new(
        glob,
        if with_predicate {
            Some(predicate(random, 0))
        } else {
            None
        },
    );
    (pattern, kind)
}

/// Build one corpus envelope over a seeded payload.
fn corpus_envelope(random: &mut SimRng, kind: String) -> Result<SignalEnvelope, String> {
    Ok(SignalEnvelope::new(
        String::from("signal"),
        kind,
        drishti::native_host::protocol::SourceKind::Widget,
        String::from("source"),
        crate::fixtures::session("tenant")?,
        JsonValue::Object(
            ["k", "a", "b"]
                .into_iter()
                .map(|key| (String::from(key), scalar(random)))
                .collect(),
        ),
        String::from("2026-11-03T00:00:00Z"),
    ))
}

/// Run one corpus from `seed` and hash every verdict.
fn run_corpus(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    for _ in 0..300 {
        let (pattern, kind) = seeded_pattern(&mut random);
        let envelope = corpus_envelope(&mut random, kind)?;
        let valid = validate_pattern(&pattern);
        trace.mix_bool(valid);
        let matched = match_pattern(&envelope, &pattern);
        trace.mix_bool(matched);
        // Invalid patterns never match: the core safety property.
        ensure(valid || !matched, || {
            format!("invalid pattern matched: {:?}", pattern.kind_glob())
        })?;
    }
    Ok(trace.finish())
}

/// Same seed replays the same verdict hash.
#[test]
fn sim_pattern_corpus_replays_identically() -> Result<(), String> {
    for seed in [3_u64, 11, 777] {
        if run_corpus(seed)? != run_corpus(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_corpus(3)? == run_corpus(4)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}

/// Run one invalid-only corpus from `seed`: patterns that fail validation
/// are collected and every one of them must refuse to match.
fn run_invalid(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut refused = 0_usize;
    for _ in 0..300 {
        let (pattern, kind) = seeded_pattern(&mut random);
        if validate_pattern(&pattern) {
            continue;
        }
        refused = refused.saturating_add(1);
        let envelope = corpus_envelope(&mut random, kind)?;
        ensure(!match_pattern(&envelope, &pattern), || {
            format!("invalid pattern matched: {:?}", pattern.kind_glob())
        })?;
        trace.mix_str(pattern.kind_glob());
    }
    ensure(refused != 0, || {
        String::from("the corpus must refuse some patterns")
    })?;
    trace.mix_len(refused);
    Ok(trace.finish())
}

/// Same seed replays the same invalid-only hash with identical refusals.
#[test]
fn sim_pattern_invalid_never_matches() -> Result<(), String> {
    for seed in [13_u64, 29, 1_001] {
        if run_invalid(seed)? != run_invalid(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_invalid(13)? == run_invalid(14)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}
