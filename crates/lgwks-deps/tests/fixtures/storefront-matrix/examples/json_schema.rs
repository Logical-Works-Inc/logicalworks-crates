//! A tool's input schema (JSON Schema 2020-12) admits a good call and refuses bad ones.
//!
//! `sim <first> <count>` replays seeded hostile calls instead, and every
//! verdict is checked against a reference model of the schema written here,
//! never against the validator's own output.

#[path = "support/sim.rs"]
mod sim;

use lgwks_std::json::{Map, Value, json};
use lgwks_std::seeded::Seeded;

/// The longest path the schema admits: the bound the journey drives past.
const MAX_PATH: usize = 256;

fn main() {
    let passed = match sim::seeds() {
        Some(range) => simulate(range),
        None => journey(),
    };
    if !passed {
        std::process::exit(1);
    }
}

fn schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "action": {"enum": ["read", "write"]},
            "path": {"type": "string", "minLength": 1, "maxLength": MAX_PATH}
        },
        "required": ["action", "path"],
        "additionalProperties": false
    })
}

fn journey() -> bool {
    let Ok(validator) = lgwks_deps::jsonschema::validator_for(&schema().into()) else {
        return false;
    };
    let longest = "p".repeat(MAX_PATH);
    let past = "p".repeat(MAX_PATH.saturating_add(1));
    let good: [Value; 2] = [
        json!({"action": "read", "path": "/notes"}),
        json!({"action": "write", "path": longest}),
    ];
    let bad: [Value; 5] = [
        json!({"action": "delete", "path": "/notes"}),
        json!({"action": "read"}),
        json!({"action": "read", "path": ""}),
        json!({"action": "read", "path": "/notes", "extra": true}),
        json!({"action": "read", "path": past}),
    ];
    good.into_iter()
        .all(|call| validator.is_valid(&call.into()))
        && bad
            .into_iter()
            .all(|call| !validator.is_valid(&call.into()))
}

/// One member of a hostile call, as the reference model sees it.
#[derive(Clone)]
enum Field {
    Absent,
    Text(String),
    Number(i64),
    Null,
    Flag,
    List,
}

/// A hostile call: the top-level shape, two members and a stowaway key.
struct Call {
    object: bool,
    action: Field,
    path: Field,
    extra: bool,
}

/// The schema, restated: an object whose only members are `action` (exactly
/// "read" or "write") and `path` (a string of 1 to 256 code points, which is
/// how JSON Schema counts length, not bytes).
fn model_admits(call: &Call) -> bool {
    let action = matches!(&call.action, Field::Text(text) if text == "read" || text == "write");
    let path = matches!(&call.path, Field::Text(text)
        if (1..=MAX_PATH).contains(&text.chars().count()));
    call.object && action && path && !call.extra
}

fn to_value(field: &Field) -> Option<Value> {
    match field {
        Field::Absent => None,
        Field::Text(text) => Some(json!(text)),
        Field::Number(number) => Some(json!(number)),
        Field::Null => Some(Value::Null),
        Field::Flag => Some(json!(true)),
        Field::List => Some(json!(["read"])),
    }
}

fn to_call(call: &Call) -> Value {
    let mut members = Map::new();
    for (key, field) in [("action", &call.action), ("path", &call.path)] {
        if let Some(value) = to_value(field) {
            members.insert(key, value);
        }
    }
    if call.extra {
        members.insert("extra", json!(true));
    }
    if call.object {
        Value::Object(members)
    } else {
        Value::Array(vec![Value::Object(members)])
    }
}

/// The weird-bytes fool's path: NUL, a right-to-left override, a zero-width
/// space, traversal, a percent-encoded NUL, and multi-byte runs at 255, 256
/// and 257 code points, where a byte count and a code-point count disagree.
fn draw_text(rng: &mut Seeded) -> Option<String> {
    const ODD: [&str; 7] = [
        "",
        "\u{0}",
        "\u{202E}gpj.exe",
        "\u{200B}",
        "../../etc",
        "%00",
        "/notes",
    ];
    const RUNS: [&str; 3] = ["p", "\u{e9}", "\u{1F980}"];
    if rng.below(2).ok()? == 0 {
        return ODD
            .get(rng.index(ODD.len()).ok()?)
            .map(|text| (*text).to_owned());
    }
    let run = RUNS.get(rng.index(RUNS.len()).ok()?)?;
    let count = match rng.below(4).ok()? {
        0 => MAX_PATH.saturating_sub(1),
        1 => MAX_PATH,
        2 => MAX_PATH.saturating_add(1),
        _ => rng.index(MAX_PATH.saturating_mul(2)).ok()?,
    };
    Some(run.repeat(count))
}

/// The fat-finger fool's action: the two admitted words, their near misses,
/// and every wrong type.
fn draw_action(rng: &mut Seeded) -> Option<Field> {
    const WORDS: [&str; 7] = ["read", "write", "Read", "delete", "", "read\u{0}", " read"];
    let field = match rng.below(8).ok()? {
        0 => Field::Absent,
        1 => Field::Number(i64::MIN),
        2 => Field::Null,
        3 => Field::List,
        _ => Field::Text((*WORDS.get(rng.index(WORDS.len()).ok()?)?).to_owned()),
    };
    Some(field)
}

fn draw_path(rng: &mut Seeded) -> Option<Field> {
    let field = match rng.below(8).ok()? {
        0 => Field::Absent,
        1 => Field::Number(i64::MAX),
        2 => Field::Flag,
        _ => Field::Text(draw_text(rng)?),
    };
    Some(field)
}

fn simulate(range: std::ops::Range<u64>) -> bool {
    let Ok(validator) = lgwks_deps::jsonschema::validator_for(&schema().into()) else {
        return false;
    };
    let mut trace = sim::Trace::new();
    for seed in range {
        let mut rng = Seeded::from_seed(seed);
        for _ in 0..32 {
            let Some(call) = draw_call(&mut rng) else {
                return false;
            };
            let expected = model_admits(&call);
            let value = to_call(&call);
            if validator.is_valid(&value.into()) != expected {
                return false;
            }
            trace.record(if expected { "admitted" } else { "refused" }, seed);
        }
    }
    trace.report()
}

fn draw_call(rng: &mut Seeded) -> Option<Call> {
    let object = rng.below(16).ok()? != 0;
    let action = draw_action(rng)?;
    let path = draw_path(rng)?;
    let extra = rng.below(8).ok()? == 0;
    Some(Call {
        object,
        action,
        path,
        extra,
    })
}
