//! Seeded deterministic replay of the `json` facade over drawn value trees.
//!
//! One seed draws nested JSON documents — objects and arrays to a bounded
//! depth, signed, unsigned and fractional numbers, strings carrying quotes,
//! backslashes, controls and astral letters — and every family asserts what an
//! external interface exchanging JSON sees through the shipped public API:
//!
//! - a tree round-trips through text, bytes, a writer, a reader and the value
//!   bridge, and every encoder emits the same bytes;
//! - compact text holds no raw line break or control, whatever the strings
//!   carried, and pretty text parses to the same tree;
//! - every strict prefix of an object document is refused, trailing
//!   whitespace is accepted and trailing content is refused;
//! - a stray invalid UTF-8 byte anywhere is refused (INV-JSON-UTF8);
//! - a non-finite float encodes as `null`, never as text JSON cannot carry;
//! - `Value`, the crate's own tree (#831), converts to `serde_json::Value` and
//!   back unchanged, writes the text `serde_json`'s tree writes, parses as it
//!   parses (repeated keys keeping the last value), and stores a string
//!   inline exactly when it is at most 22 bytes;
//! - the same seed replays the same documents and distinct seeds diverge.
//!
//! Sources: RFC 8259 (§2 whitespace, §4 member names, §6 numbers with no NaN
//! or Infinity, §7 string escapes, §8.1 UTF-8), INV-JSON-UTF8, and #831.
#![cfg(feature = "json")]

use std::error::Error;

use crate::seeded_bytes::{below, fold_bytes, next_byte};
use crate::seeded_sweep::{
    SWEEP_SEEDS, assert_distinct_seeds_diverge, assert_same_seed_replays, initial_trace, next_seed,
    seeded_stream,
};

use lgwks_std::json::{self, Map, Number, Text, Value};

/// The inline cut of `json::Text`, in bytes (#831).
const INLINE_BYTES: usize = 22;
use lgwks_std::seeded::Seeded;

/// How many documents each family draws per seed.
const DRAWS: usize = 64;

/// How deep a drawn tree may nest.
const DEPTH: usize = 4;

/// The glyphs drawn strings are made of.
const GLYPHS: [char; 12] = [
    'a', 'Z', '9', ' ', '"', '\\', '\n', '\u{1}', '\u{7f}', 'ß', '€', '😀',
];

/// A family's result.
type Outcome = Result<(), Box<dyn Error>>;

/// Up to 24 drawn glyphs, one to four bytes each, so strings land on both
/// sides of the 22-byte inline capacity of `Value`'s strings (#831).
fn drawn_string(state: &mut Seeded) -> String {
    (0..below(state, 25))
        .map(|_| match GLYPHS.get(below(state, GLYPHS.len())) {
            Some(glyph) => *glyph,
            None => 'a',
        })
        .collect()
}

/// A drawn number: signed, unsigned, or a finite fraction.
fn drawn_number(state: &mut Seeded) -> Value {
    match below(state, 3) {
        0 => Value::from(i64::from_le_bytes(next_seed(state).to_le_bytes())),
        1 => Value::from(next_seed(state)),
        _ => {
            let fractions = [-16.0, -0.125, 0.0, 0.5, 1.0e-300, 2.5e300, 31.875];
            let fraction = match fractions.get(below(state, fractions.len())) {
                Some(fraction) => *fraction,
                None => 0.0,
            };
            match Number::from_f64(fraction) {
                Some(number) => Value::Number(number),
                None => Value::Null,
            }
        }
    }
}

/// A drawn tree no deeper than `depth`.
fn drawn_value(state: &mut Seeded, depth: usize) -> Value {
    let kinds = if depth == 0 { 4 } else { 6 };
    match below(state, kinds) {
        0 => Value::Null,
        1 => Value::Bool(next_byte(state) & 1 == 1),
        2 => drawn_number(state),
        3 => Value::from(drawn_string(state)),
        4 => Value::Array(
            (0..below(state, 4))
                .map(|_| drawn_value(state, depth.saturating_sub(1)))
                .collect(),
        ),
        _ => Value::Object(drawn_object(state, depth.saturating_sub(1))),
    }
}

/// A drawn object whose members are no deeper than `depth`.
fn drawn_object(state: &mut Seeded, depth: usize) -> Map {
    (0..below(state, 4))
        .map(|_| (drawn_string(state), drawn_value(state, depth)))
        .collect()
}

/// Every drawn object document under each seed.
fn sweep(mut check: impl FnMut(u64, &mut Seeded, &Value) -> Outcome) -> Outcome {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..DRAWS {
            let document = Value::Object(drawn_object(&mut state, DEPTH));
            check(seed, &mut state, &document)?;
        }
    }
    Ok(())
}

#[test]
/// A tree round-trips through text and through bytes.
fn sim_a_tree_round_trips_through_text_and_bytes() -> Outcome {
    sweep(|seed, _, document| {
        let text = json::to_string(document)?;
        assert_eq!(
            &json::from_str::<Value>(&text)?,
            document,
            "seed {seed:#018x}: {text}"
        );
        let bytes = json::to_vec(document)?;
        assert_eq!(
            &json::from_slice::<Value>(&bytes)?,
            document,
            "seed {seed:#018x}"
        );
        Ok(())
    })
}

#[test]
/// Every encoder emits the same bytes: text, vector and writer.
fn sim_every_encoder_emits_the_same_bytes() -> Outcome {
    sweep(|seed, _, document| {
        let text = json::to_string(document)?;
        let mut written = Vec::new();
        json::to_writer(&mut written, document)?;
        assert_eq!(
            json::to_vec(document)?,
            text.as_bytes(),
            "seed {seed:#018x}"
        );
        assert_eq!(written, text.as_bytes(), "seed {seed:#018x}");
        Ok(())
    })
}

#[test]
/// A reader and the value bridge decode the tree the text decoder does.
fn sim_the_reader_and_the_value_bridge_agree() -> Outcome {
    sweep(|seed, _, document| {
        let bytes = json::to_vec(document)?;
        let from_reader: Value = json::from_reader(bytes.as_slice())?;
        assert_eq!(&from_reader, document, "seed {seed:#018x}");
        let bridged: Value = json::from_value(json::to_value(document)?)?;
        assert_eq!(&bridged, document, "seed {seed:#018x}");
        Ok(())
    })
}

#[test]
/// Compact text holds no raw control, and pretty text parses to the same tree.
fn sim_compact_text_holds_no_raw_control() -> Outcome {
    sweep(|seed, _, document| {
        let compact = json::to_string(document)?;
        // RFC 8259 §7 requires escaping U+0000 through U+001F; DEL is the one
        // control a string may carry raw.
        assert!(
            compact.chars().all(|glyph| glyph >= ' '),
            "seed {seed:#018x}: a raw C0 control in {compact:?}"
        );
        let pretty = json::to_string_pretty(document)?;
        assert_eq!(
            &json::from_str::<Value>(&pretty)?,
            document,
            "seed {seed:#018x}"
        );
        Ok(())
    })
}

#[test]
/// Every strict prefix of an object document is refused.
fn sim_every_strict_prefix_is_refused() -> Outcome {
    sweep(|seed, state, document| {
        let text = json::to_string(document)?;
        let boundaries: Vec<usize> = (0..text.len())
            .filter(|cut| text.is_char_boundary(*cut))
            .collect();
        let cut = boundaries
            .get(below(state, boundaries.len()))
            .copied()
            .ok_or("a boundary")?;
        let prefix = text.get(..cut).ok_or("a char boundary")?;
        assert!(
            json::from_str::<Value>(prefix).is_err(),
            "seed {seed:#018x}: the prefix {prefix:?} parsed"
        );
        Ok(())
    })
}

#[test]
/// Trailing whitespace is accepted and trailing content is refused.
fn sim_trailing_content_is_refused_and_whitespace_is_not() -> Outcome {
    sweep(|seed, state, document| {
        let text = json::to_string(document)?;
        let padding = [" ", "\n", "\r\n\t "];
        let padded = format!(
            "{text}{}",
            padding.get(below(state, padding.len())).ok_or("pad")?
        );
        assert_eq!(
            &json::from_str::<Value>(&padded)?,
            document,
            "seed {seed:#018x}"
        );
        let junk = ["x", "{}", "1", ",", "null"];
        let trailing = format!(
            "{text} {}",
            junk.get(below(state, junk.len())).ok_or("junk")?
        );
        assert!(
            json::from_str::<Value>(&trailing).is_err(),
            "seed {seed:#018x}: trailing content parsed: {trailing:?}"
        );
        Ok(())
    })
}

#[test]
/// A stray invalid UTF-8 byte anywhere is refused.
fn sim_a_stray_invalid_byte_is_refused() -> Outcome {
    sweep(|seed, state, document| {
        let mut bytes = json::to_vec(document)?;
        let at = below(state, bytes.len().saturating_add(1));
        let poison = [0xFF_u8, 0xC0, 0x80, 0xF8];
        bytes.insert(
            at,
            poison
                .get(below(state, poison.len()))
                .copied()
                .ok_or("a poison byte")?,
        );
        assert!(
            json::from_slice::<Value>(&bytes).is_err(),
            "seed {seed:#018x}: a stray byte at {at} decoded"
        );
        Ok(())
    })
}

#[test]
/// A non-finite float encodes as `null`, never as text JSON cannot carry.
fn sim_a_non_finite_float_encodes_as_null() -> Outcome {
    sweep(|seed, state, _| {
        let floats = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY];
        let float = floats
            .get(below(state, floats.len()))
            .copied()
            .ok_or("a float")?;
        assert_eq!(
            json::to_string(&float)?,
            "null",
            "seed {seed:#018x}: {float}"
        );
        assert_eq!(
            json::to_value(&float)?,
            Value::Null,
            "seed {seed:#018x}: {float}"
        );
        assert!(
            Number::from_f64(float).is_none(),
            "seed {seed:#018x}: {float}"
        );
        Ok(())
    })
}

#[test]
/// A tree converts to `serde_json`'s tree and back unchanged, and both trees
/// write one text.
fn sim_a_tree_round_trips_through_the_serde_json_tree() -> Outcome {
    sweep(|seed, _, document| {
        let theirs = serde_json::Value::from(document.clone());
        assert_eq!(
            serde_json::to_string(&theirs)?,
            json::to_string(document)?,
            "seed {seed:#018x}: both trees write one text"
        );
        assert_eq!(
            &Value::from(theirs),
            document,
            "seed {seed:#018x}: the round trip is the identity"
        );
        let text = json::to_string_pretty(document)?;
        let parsed: serde_json::Value = serde_json::from_str(&text)?;
        assert_eq!(
            json::from_str::<Value>(&text)?,
            Value::from(parsed),
            "seed {seed:#018x}: both parsers read one tree from {text}"
        );
        Ok(())
    })
}

/// The texts the direct reader must read as `serde_json` does: numbers at and
/// past every 64-bit edge, leading zeros, signs, fractions and exponents,
/// escapes and surrogates, controls, literals, nesting at and past 128 levels,
/// and broken punctuation (RFC 8259 §§2, 6, 7).
fn edge_texts() -> Vec<String> {
    let mut texts: Vec<String> = [
        "0",
        "-0",
        "-0.0",
        "01",
        "-01",
        "1.",
        ".5",
        "1e5",
        "1E+2",
        "-1e-2",
        "1e400",
        "-",
        "+1",
        "18446744073709551615",
        "18446744073709551616",
        "-9223372036854775808",
        "-9223372036854775809",
        "1.5.3",
        "1-2",
        r#""é😀""#,
        r#""\ud800""#,
        r#""\x""#,
        "\"a\u{1}b\"",
        r#""\"\\\/\b\f\n\r\t""#,
        "nul",
        "truex",
        "[1,]",
        "[,1]",
        "{\"a\":1,}",
        "{\"a\" 1}",
        "{1:2}",
        "{\"a\":1}{",
        " \t\n\r[ 1 , {\"b\" : [] } ]\r\n",
        "",
        "  ",
        "[",
        "{\"k\":\"v\",\"k\":2}",
    ]
    .iter()
    .map(|text| (*text).to_owned())
    .collect();
    for depth in [127_usize, 128, 129, 200] {
        texts.push(format!("{}{}", "[".repeat(depth), "]".repeat(depth)));
    }
    texts
}

/// `text.parse::<Value>()` returns what `json::from_str::<Value>` returns: the
/// same tree, or a refusal with the same message, line and column.
fn assert_reads_as_serde(text: &str) -> Outcome {
    let direct = text.parse::<Value>();
    let through_serde = json::from_str::<Value>(text);
    match (direct, through_serde) {
        (Ok(ours), Ok(theirs)) => assert_eq!(ours, theirs, "{text:?} reads one tree"),
        (Err(ours), Err(theirs)) => assert_eq!(
            ours.to_string(),
            theirs.to_string(),
            "{text:?} is refused with serde_json's error"
        ),
        (ours, theirs) => assert_eq!(
            ours.is_ok(),
            theirs.is_ok(),
            "{text:?}: direct {ours:?}, serde {theirs:?}"
        ),
    }
    Ok(())
}

#[test]
/// The direct reader behind `parse::<Value>()` reads every drawn document, its
/// pretty form, every prefix cut and every edge text exactly as `serde_json`
/// does (#831): one tree, or `serde_json`'s refusal.
fn sim_the_direct_reader_reads_as_serde_json_reads() -> Outcome {
    for text in edge_texts() {
        assert_reads_as_serde(&text)?;
    }
    sweep(|seed, state, document| {
        let compact = json::to_string(document)?;
        let pretty = json::to_string_pretty(document)?;
        assert_eq!(
            &compact.parse::<Value>()?,
            document,
            "seed {seed:#018x}: {compact}"
        );
        assert_reads_as_serde(&pretty)?;
        let boundaries: Vec<usize> = (0..compact.len())
            .filter(|cut| compact.is_char_boundary(*cut))
            .collect();
        let cut = boundaries
            .get(below(state, boundaries.len()))
            .copied()
            .ok_or("a boundary")?;
        assert_reads_as_serde(compact.get(..cut).ok_or("a char boundary")?)?;
        assert_reads_as_serde(&repeated_member_text(state)?)?;
        Ok(())
    })
}

/// Object text with members in drawn order and keys drawn from three names,
/// so keys repeat: text no `Value` writes but a peer may send.
fn repeated_member_text(state: &mut Seeded) -> Result<String, Box<dyn Error>> {
    let names = ["id", "a-member-name-past-the-cut", "ß"];
    let mut members = Vec::new();
    for _ in 0..below(state, 8) {
        let name = names.get(below(state, names.len())).ok_or("a name")?;
        let member = json::to_string(&drawn_value(state, 1))?;
        members.push(format!("{}:{member}", json::to_string(name)?));
    }
    Ok(format!("{{{}}}", members.join(",")))
}

#[test]
/// Repeated keys read as `serde_json` reads them: the last value kept, keys
/// in ascending order, each key once.
fn sim_a_repeated_key_keeps_its_last_value() -> Outcome {
    sweep(|seed, state, _| {
        let text = repeated_member_text(state)?;
        let ours: Value = json::from_str(&text)?;
        let theirs: serde_json::Value = serde_json::from_str(&text)?;
        let keys: Vec<&str> = ours.as_object().ok_or("an object")?.keys().collect();
        assert!(
            keys.windows(2)
                .all(|pair| matches!(*pair, [left, right] if left < right)),
            "seed {seed:#018x}: keys strictly ascend in {text}"
        );
        assert_eq!(ours, Value::from(theirs), "seed {seed:#018x}: {text}");
        Ok(())
    })
}

#[test]
/// A drawn string on either side of the 22-byte inline cut reads back whole,
/// as text and as a parsed value; which side it is stored on is held by
/// `json::text::tests`.
fn sim_a_string_is_inline_exactly_up_to_the_capacity() -> Outcome {
    sweep(|seed, state, _| {
        let drawn = drawn_string(state);
        let text = Text::from(drawn.as_str());
        assert_eq!(text.as_str(), drawn, "seed {seed:#018x}: reads back whole");
        assert_eq!(
            text.len(),
            drawn.len(),
            "seed {seed:#018x}: keeps its length"
        );
        let parsed: Value = json::from_str(&json::to_string(&drawn)?)?;
        assert_eq!(parsed, drawn, "seed {seed:#018x}: parses whole");
        Ok(())
    })
}

#[test]
/// The inline boundary at its declared length: 22 bytes (inline), 23 (heap)
/// and 23 whose last character straddles the cut each parse, read back and
/// key an object whole.
fn the_inline_boundary_holds_at_its_declared_length() -> Outcome {
    let at = "k".repeat(INLINE_BYTES);
    let past = "k".repeat(INLINE_BYTES.saturating_add(1));
    let straddle = format!("{}€", "k".repeat(INLINE_BYTES.saturating_sub(2)));
    assert_eq!(
        straddle.len(),
        23,
        "twenty ASCII bytes and a three-byte euro"
    );
    for text in [&at, &past, &straddle] {
        let parsed: Value = json::from_str(&json::to_string(text)?)?;
        let Value::String(ref stored) = parsed else {
            return Err("a string parses as a string".into());
        };
        assert_eq!(
            stored.as_str(),
            text.as_str(),
            "{} bytes read back",
            text.len()
        );
        let mut members = Map::new();
        members.insert(text.as_str(), Value::Null);
        assert!(members.contains_key(text), "{} bytes is a key", text.len());
    }
    Ok(())
}

#[test]
/// `Map::extend` keeps exactly what inserting each member in turn keeps: keys
/// ascending, each once, a repeated key holding its last value, whether the
/// keys arrive sorted, reversed or drawn with repeats, into an empty map or
/// one already holding members. A `BTreeMap` of the same members is the
/// model (#831: the map replaces a `BTreeMap`).
fn sim_map_extend_matches_insert_one_by_one() {
    for seed in SWEEP_SEEDS {
        let mut state = seeded_stream(seed);
        for _ in 0..DRAWS {
            let held = below(&mut state, 12);
            let added = below(&mut state, 40);
            let span = below(&mut state, 24).saturating_add(1);
            let draw = |state: &mut Seeded, count: usize, order: usize| -> Vec<(String, String)> {
                let mut members: Vec<(String, String)> = (0..count)
                    .map(|index| (format!("k{:02}", below(state, span)), format!("v{index}")))
                    .collect();
                match order {
                    0 => members.sort(),
                    1 => members.sort_by(|left, right| right.cmp(left)),
                    _ => {}
                }
                members
            };
            let held_order = below(&mut state, 3);
            let added_order = below(&mut state, 3);
            let before = draw(&mut state, held, held_order);
            let after = draw(&mut state, added, added_order);
            let mut inserted = Map::new();
            let mut model = std::collections::BTreeMap::new();
            for pair in before.iter().chain(after.iter()) {
                inserted.insert(pair.0.as_str(), Value::String(Text::from(pair.1.as_str())));
                model.insert(pair.0.clone(), pair.1.clone());
            }
            let member = |pair: &(String, String)| {
                (pair.0.clone(), Value::String(Text::from(pair.1.as_str())))
            };
            let mut extended: Map = before.iter().map(member).collect();
            extended.extend(after.iter().map(member));
            assert_eq!(
                extended, inserted,
                "seed {seed:#018x}: extend must keep what member-by-member insert keeps"
            );
            let seen: Vec<(&str, &str)> = extended
                .iter()
                .filter_map(|(key, value)| match *value {
                    Value::String(ref text) => Some((key, text.as_str())),
                    _ => None,
                })
                .collect();
            let wanted: Vec<(&str, &str)> = model
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();
            assert_eq!(
                seen, wanted,
                "seed {seed:#018x}: keys ascend once each, the last value of a repeat winning"
            );
            let twice = extended.clone();
            extended.extend(Vec::<(&str, Value)>::new());
            assert_eq!(
                extended, twice,
                "seed {seed:#018x}: an empty extend changes nothing"
            );
        }
    }
}

/// The replay trace of every document drawn under `seed`.
fn document_trace(seed: u64) -> u64 {
    let mut state = seeded_stream(seed);
    let mut trace = initial_trace();
    for _ in 0..DRAWS {
        let document = Value::Object(drawn_object(&mut state, DEPTH));
        match json::to_vec(&document) {
            Ok(bytes) => fold_bytes(&mut trace, &bytes),
            Err(_) => fold_bytes(&mut trace, b"refused"),
        }
    }
    trace
}

#[test]
/// The replay oracle: one seed, one document trace.
fn sim_the_same_seed_replays_the_same_documents() {
    for seed in SWEEP_SEEDS {
        assert_same_seed_replays(document_trace, seed);
    }
}

#[test]
/// The divergence oracle: a sweep that ignored its seed would pass the replay.
fn sim_distinct_seeds_draw_different_documents() {
    assert_distinct_seeds_diverge(document_trace, SWEEP_SEEDS[0], SWEEP_SEEDS[1]);
}
