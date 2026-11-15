//! Value and predicate sims: random trees are written out by an encoder that
//! lives in this file (not the crate's serializer), decoded by the crate, and
//! compared; hostile shapes and absurd nesting must be refused, never crash.

use drishti::native_host::pattern::match_pattern;
use drishti::native_host::protocol::{
    JsonValue, PayloadPredicate, SignalEnvelope, SignalPattern, SourceKind,
};

use crate::fixtures::session;
use crate::motion_kit::{describe, ensure};
use crate::rng::SimRng;

/// JSON string literal for the text, escaping what must be escaped.
fn encode_string(text: &str) -> String {
    let mut out = String::from("\"");
    for found in text.chars() {
        match found {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// JSON text of a value, written independently of the crate's serializer.
fn encode_value(value: &JsonValue) -> String {
    match *value {
        JsonValue::String(ref text) => encode_string(text),
        JsonValue::Number(number) => format!("{number}"),
        JsonValue::Bool(flag) => format!("{flag}"),
        JsonValue::Array(ref items) => {
            let parts: Vec<String> = items.iter().map(encode_value).collect();
            format!("[{}]", parts.join(","))
        }
        JsonValue::Object(ref map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|entry| format!("{}:{}", encode_string(entry.0), encode_value(entry.1)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        _ => String::from("null"),
    }
}

/// Seeded text drawn from awkward pieces: quotes, backslashes, newlines,
/// tabs, accents, emoji and brackets.
fn gen_text(random: &mut SimRng) -> String {
    const PIECES: &[&str] = &[
        "a",
        "\"",
        "\\",
        "\n",
        "\u{e9}",
        "\u{1f600}",
        "",
        "\t",
        "x y",
        "{",
        "]",
    ];
    let count = random.below_len(4);
    let mut out = String::new();
    for _ in 0..count {
        if let Some(piece) = random.pick_str(PIECES) {
            out.push_str(piece);
        }
    }
    out
}

/// Seeded value tree no deeper than three levels.
fn gen_value(random: &mut SimRng, depth: usize) -> JsonValue {
    let kinds = if depth >= 3 { 4 } else { 6 };
    match random.below(kinds) {
        0 => JsonValue::String(gen_text(random)),
        1 => JsonValue::Number(f64::from(random.below_i32(2_000)) - 1_000.0),
        2 => JsonValue::Bool(random.below(2) == 1),
        3 => JsonValue::Null,
        4 => {
            let count = random.below_len(4);
            let mut items = Vec::new();
            for _ in 0..count {
                items.push(gen_value(random, depth.saturating_add(1)));
            }
            JsonValue::Array(items)
        }
        _ => {
            let count = random.below_len(4);
            let mut map = std::collections::BTreeMap::new();
            for _ in 0..count {
                let key = gen_text(random);
                map.insert(key, gen_value(random, depth.saturating_add(1)));
            }
            JsonValue::Object(map)
        }
    }
}

/// Bug caught: a value codec that confuses booleans with numbers, loses an
/// escape, or reorders keys, so a payload changes meaning on the wire.
#[test]
fn sim_json_value_roundtrips_through_an_independent_encoder() -> Result<(), String> {
    for seed in 0..300_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(50_000));
        let value = gen_value(&mut random, 0);
        let text = encode_value(&value);
        let decoded: JsonValue = lgwks_std::json::from_str(&text).map_err(describe)?;
        ensure(
            decoded == value,
            &format!("seed {seed}: {text} decoded to {decoded:?}"),
        )?;
        let generic: lgwks_std::json::Value = lgwks_std::json::from_str(&text).map_err(describe)?;
        ensure(
            JsonValue::from_json(&generic) == value,
            &format!("seed {seed}: the estate value of {text} converted differently"),
        )?;
        let written = lgwks_std::json::to_value(&value).map_err(describe)?;
        ensure(
            JsonValue::from_json(&written) == value,
            &format!("seed {seed}: serializing {text} changed the value"),
        )?;
    }
    Ok(())
}

/// Seeded keypath of one to three segments, with empty ones now and then.
fn gen_keypath(random: &mut SimRng) -> String {
    const SEGMENTS: &[&str] = &["a", "b", "c", "a", "b", ""];
    let count = random.below_len(3).saturating_add(1);
    let mut parts = Vec::new();
    for _ in 0..count {
        if let Some(part) = random.pick_str(SEGMENTS) {
            parts.push(part);
        }
    }
    parts.join(".")
}

/// Seeded predicate tree.
fn gen_predicate(random: &mut SimRng, depth: usize) -> PayloadPredicate {
    let kinds = if depth >= 3 { 4 } else { 6 };
    match random.below(kinds) {
        0 => PayloadPredicate::eq(gen_keypath(random), gen_value(random, 2)),
        1 => PayloadPredicate::neq(gen_keypath(random), gen_value(random, 2)),
        2 => {
            let count = random.below_len(4);
            let mut values = Vec::new();
            for _ in 0..count {
                values.push(gen_value(random, 2));
            }
            PayloadPredicate::is_in(gen_keypath(random), values)
        }
        3 => PayloadPredicate::exists(gen_keypath(random)),
        other => {
            let count = random.below_len(4);
            let mut children = Vec::new();
            for _ in 0..count {
                children.push(gen_predicate(random, depth.saturating_add(1)));
            }
            if other == 4 {
                PayloadPredicate::And(children)
            } else {
                PayloadPredicate::Or(children)
            }
        }
    }
}

/// The documented wire form of a predicate, written by hand.
fn encode_predicate(predicate: &PayloadPredicate) -> String {
    match *predicate {
        PayloadPredicate::Eq {
            ref keypath,
            ref value,
        } => format!(
            "{{\"eq\":[{},{}]}}",
            encode_string(keypath),
            encode_value(value)
        ),
        PayloadPredicate::Neq {
            ref keypath,
            ref value,
        } => format!(
            "{{\"neq\":[{},{}]}}",
            encode_string(keypath),
            encode_value(value)
        ),
        PayloadPredicate::In {
            ref keypath,
            ref values,
        } => {
            let parts: Vec<String> = values.iter().map(encode_value).collect();
            format!(
                "{{\"in\":[{},[{}]]}}",
                encode_string(keypath),
                parts.join(",")
            )
        }
        PayloadPredicate::Exists { ref keypath } => {
            format!("{{\"exists\":{}}}", encode_string(keypath))
        }
        PayloadPredicate::And(ref children) => {
            let parts: Vec<String> = children.iter().map(encode_predicate).collect();
            format!("{{\"and\":[{}]}}", parts.join(","))
        }
        PayloadPredicate::Or(ref children) => {
            let parts: Vec<String> = children.iter().map(encode_predicate).collect();
            format!("{{\"or\":[{}]}}", parts.join(","))
        }
        _ => String::from("null"),
    }
}

/// Bug caught: a predicate that decodes from the documented wire form but
/// serializes to a different form, so a pattern saved and reloaded changes
/// what it matches (the `in` list was written flat and read nested).
#[test]
fn sim_json_predicate_decodes_and_serializes_the_documented_form() -> Result<(), String> {
    for seed in 0..300_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(60_000));
        let predicate = gen_predicate(&mut random, 0);
        let text = encode_predicate(&predicate);
        let decoded: PayloadPredicate = lgwks_std::json::from_str(&text).map_err(describe)?;
        ensure(
            decoded == predicate,
            &format!("seed {seed}: {text} decoded to {decoded:?}"),
        )?;
        let written = lgwks_std::json::to_value(&predicate).map_err(describe)?;
        let expected: lgwks_std::json::Value =
            lgwks_std::json::from_str(&text).map_err(describe)?;
        ensure(
            JsonValue::from_json(&written) == JsonValue::from_json(&expected),
            &format!("seed {seed}: serialized form differs from the documented {text}"),
        )?;
        let pattern = SignalPattern::new(String::from("a.**"), Some(predicate));
        let saved = lgwks_std::json::to_value(&pattern).map_err(describe)?;
        let reloaded = JsonValue::from_json(&saved);
        let by_hand = format!("{{\"kind_glob\":\"a.**\",\"payload_predicate\":{text}}}");
        let hand_value: lgwks_std::json::Value =
            lgwks_std::json::from_str(&by_hand).map_err(describe)?;
        ensure(
            reloaded == JsonValue::from_json(&hand_value),
            &format!("seed {seed}: a saved pattern differs from the documented form"),
        )?;
        let again: SignalPattern = lgwks_std::json::from_str(&by_hand).map_err(describe)?;
        ensure(
            again == pattern,
            &format!("seed {seed}: pattern reload drifted"),
        )?;
    }
    Ok(())
}

/// Bug caught: a malformed predicate accepted as some other operator or as
/// an empty clause, which would turn a typo into match-everything.
#[test]
fn sim_json_malformed_predicates_are_refused() -> Result<(), String> {
    let hostile = [
        "{\"eq\":[\"k\"]}",
        "{\"eq\":[1,2]}",
        "{\"eq\":\"k\"}",
        "{\"eq\":[\"k\",1,2]}",
        "{\"neq\":[]}",
        "{\"in\":[\"k\"]}",
        "{\"in\":[\"k\",\"v\"]}",
        "{\"in\":[1,[]]}",
        "{\"in\":[\"k\",[],[]]}",
        "{\"exists\":5}",
        "{\"exists\":null}",
        "{\"and\":{}}",
        "{\"and\":[5]}",
        "{\"or\":\"x\"}",
        "{\"foo\":1}",
        "{}",
        "[]",
        "5",
        "\"eq\"",
        "null",
        "true",
        "{\"and\":[{\"or\":[{\"nope\":1}]}]}",
        "{\"EQ\":[\"k\",1]}",
        "{\"eq \":[\"k\",1]}",
    ];
    for text in hostile {
        let bare = lgwks_std::json::from_str::<PayloadPredicate>(text);
        ensure(bare.is_err(), &format!("{text} decoded as {bare:?}"))?;
        let wrapped = format!("{{\"kind_glob\":\"a\",\"payload_predicate\":{text}}}");
        let pattern = lgwks_std::json::from_str::<SignalPattern>(&wrapped);
        ensure(
            pattern.is_err() == (text != "null"),
            &format!("{text} inside a pattern gave {pattern:?}"),
        )?;
    }
    for text in [
        "{\"and\":[]}",
        "{\"or\":[]}",
        "{\"exists\":\"\"}",
        "{\"eq\":[\"\",null]}",
    ] {
        let decoded = lgwks_std::json::from_str::<PayloadPredicate>(text);
        ensure(decoded.is_ok(), &format!("{text} must decode: {decoded:?}"))?;
    }
    Ok(())
}

/// Wire text of a predicate nested `depth` levels under `and`.
fn nested_and(depth: usize) -> String {
    format!(
        "{}{{\"exists\":\"k\"}}{}",
        "{\"and\":[".repeat(depth),
        "]}".repeat(depth)
    )
}

/// A widget envelope on `crm.x` over the given payload.
fn probe_envelope(payload: JsonValue) -> Result<SignalEnvelope, String> {
    Ok(SignalEnvelope::new(
        String::from("signal"),
        String::from("crm.x"),
        SourceKind::Widget,
        String::from("source"),
        session("tenant-j")?,
        payload,
        String::from("2026-11-03T00:00:00Z"),
    ))
}

/// Bug caught: predicate nesting that is decoded to unbounded depth (stack
/// exhaustion from a hostile pattern) or that matches past the 64-node
/// budget the validator promises.
#[test]
fn sim_json_nesting_is_bounded_at_decode_and_at_match() -> Result<(), String> {
    let payload = JsonValue::Object(
        [(String::from("k"), JsonValue::Number(1.0))]
            .into_iter()
            .collect(),
    );
    let envelope = probe_envelope(payload)?;
    for depth in 20..=72_usize {
        let wire = format!(
            "{{\"kind_glob\":\"crm.**\",\"payload_predicate\":{}}}",
            nested_and(depth)
        );
        let decoded = lgwks_std::json::from_str::<SignalPattern>(&wire);
        // The JSON reader may stop earlier than 64; it must never go later.
        ensure(
            decoded.is_ok() || depth > 20,
            &format!("nesting {depth}: a shallow predicate was refused"),
        )?;
        ensure(
            !decoded.is_ok() || depth <= 64,
            &format!("nesting {depth}: decoded past the cap of 64"),
        )?;
        if let Ok(pattern) = decoded {
            ensure(
                match_pattern(&envelope, &pattern) == (depth <= 63),
                &format!("nesting {depth}: wire match verdict breaks the 64-node budget"),
            )?;
        }
        let mut built = PayloadPredicate::exists(String::from("k"));
        for _ in 0..depth {
            built = PayloadPredicate::And(vec![built]);
        }
        let direct = SignalPattern::new(String::from("crm.**"), Some(built));
        ensure(
            match_pattern(&envelope, &direct) == (depth <= 63),
            &format!("nesting {depth}: built match verdict breaks the 64-node budget"),
        )?;
    }
    for depth in [200_usize, 5_000, 100_000] {
        let wire = format!(
            "{{\"kind_glob\":\"crm.**\",\"payload_predicate\":{}}}",
            nested_and(depth)
        );
        ensure(
            lgwks_std::json::from_str::<SignalPattern>(&wire).is_err(),
            &format!("nesting {depth} was accepted"),
        )?;
    }
    Ok(())
}

/// Bug caught: a keypath depth bound off by one, so the ninth segment is
/// honoured (or the eighth refused) and one pattern costs unbounded walking.
#[test]
fn sim_json_keypath_depth_bound_is_eight() -> Result<(), String> {
    let mut payload = JsonValue::Number(1.0);
    for level in (0..9).rev() {
        payload = JsonValue::Object([(format!("k{level}"), payload)].into_iter().collect());
    }
    let envelope = probe_envelope(payload)?;
    for depth in 1..=10_usize {
        let path: Vec<String> = (0..depth).map(|level| format!("k{level}")).collect();
        let pattern = SignalPattern::new(
            String::from("crm.**"),
            Some(PayloadPredicate::exists(path.join("."))),
        );
        ensure(
            match_pattern(&envelope, &pattern) == (depth <= 8),
            &format!("keypath depth {depth}: verdict breaks the bound of 8"),
        )?;
        let gapped = SignalPattern::new(
            String::from("crm.**"),
            Some(PayloadPredicate::exists(format!(".{}..", path.join("..")))),
        );
        ensure(
            match_pattern(&envelope, &gapped) == (depth <= 8),
            &format!("gapped keypath depth {depth}: empty segments must not count"),
        )?;
    }
    Ok(())
}
