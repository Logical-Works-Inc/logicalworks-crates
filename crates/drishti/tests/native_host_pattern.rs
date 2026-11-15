//! Pattern matcher contract.
//!
//! Counterpart of `CanvasProtocolTests/PatternMatcherTests.swift`
//! (kernel#850 row 15): kind globs, predicates, validation bounds, and
//! numeric/boolean type discipline.

use drishti::native_host::pattern::{match_pattern, validate_pattern};
use drishti::native_host::protocol::{JsonValue, PayloadPredicate, SignalEnvelope, SignalPattern};
use lgwks_std::json::json;

#[path = "support/fixtures.rs"]
mod fixtures;

/// Envelope fixture with a fixed session; only kind and payload matter to
/// the matcher.
fn envelope(kind: &str, payload: JsonValue) -> Result<SignalEnvelope, String> {
    Ok(SignalEnvelope::new(
        String::from("signal"),
        String::from(kind),
        drishti::native_host::protocol::SourceKind::Widget,
        String::from("widget"),
        fixtures::session("tenant")?,
        payload,
        String::from("2026-11-03T00:00:00Z"),
    ))
}

/// Glob-only pattern fixture.
fn globbed(glob: &str) -> SignalPattern {
    SignalPattern::new(String::from(glob), None)
}

/// Assert one match vector.
fn assert_match(
    pattern: &SignalPattern,
    kind: &str,
    payload: JsonValue,
    want: bool,
) -> Result<(), String> {
    let got = match_pattern(&envelope(kind, payload)?, pattern);
    (got == want).then_some(()).ok_or_else(|| {
        format!(
            "match(kind={kind:?}, glob={:?}) = {got}, want {want}",
            pattern.kind_glob()
        )
    })
}

/// Assert one null-payload kind-glob vector.
fn assert_glob(glob: &str, kind: &str, want: bool) -> Result<(), String> {
    assert_match(&globbed(glob), kind, JsonValue::Null, want)
}

/// Exact, single-wildcard, trailing-double-star, and deep matches behave.
#[test]
fn kind_globs_match() -> Result<(), String> {
    assert_glob("forms.submitted", "forms.submitted", true)?;
    assert_glob("forms.submitted", "forms.other", false)?;
    assert_match(
        &globbed("forms.*"),
        "forms.submitted",
        JsonValue::Null,
        true,
    )?;
    assert_match(&globbed("forms.*"), "forms.a.b", JsonValue::Null, false)?;
    assert_match(&globbed("forms.**"), "forms.a.b.c", JsonValue::Null, true)?;
    assert_match(&globbed("forms.**"), "forms", JsonValue::Null, true)?;
    assert_match(&globbed("a.**"), "a.b.c", JsonValue::Null, true)?;
    assert_match(&globbed("a.**"), "a.b.d.e", JsonValue::Null, true)?;
    assert_match(&globbed("**"), "anything.at.all", JsonValue::Null, true)?;
    assert_match(&globbed("*"), "single", JsonValue::Null, true)?;
    assert_match(&globbed("*"), "two.segments", JsonValue::Null, false)?;
    Ok(())
}

/// Predicates filter on payload values with strict type discipline.
#[test]
fn predicates_filter() -> Result<(), String> {
    let payload = JsonValue::from_json(&json!({
        "status": "ok",
        "count": 3,
        "flag": true,
        "nested": { "deep": "yes" },
    }));
    let eq = SignalPattern::new(
        String::from("**"),
        Some(PayloadPredicate::eq(
            String::from("status"),
            JsonValue::String(String::from("ok")),
        )),
    );
    assert_match(&eq, "any.kind", payload.clone(), true)?;
    let missing = SignalPattern::new(
        String::from("**"),
        Some(PayloadPredicate::eq(
            String::from("absent"),
            JsonValue::String(String::from("ok")),
        )),
    );
    assert_match(&missing, "any.kind", payload.clone(), false)?;
    // Numbers and booleans never cross: count 3 != true, flag true != 1.
    let cross = SignalPattern::new(
        String::from("**"),
        Some(PayloadPredicate::eq(
            String::from("count"),
            JsonValue::Bool(true),
        )),
    );
    assert_match(&cross, "any.kind", payload.clone(), false)?;
    let cross_number = SignalPattern::new(
        String::from("**"),
        Some(PayloadPredicate::eq(
            String::from("flag"),
            JsonValue::Number(1.0),
        )),
    );
    assert_match(&cross_number, "any.kind", payload.clone(), false)?;
    // Ints and doubles compare by value.
    let number = SignalPattern::new(
        String::from("**"),
        Some(PayloadPredicate::eq(
            String::from("count"),
            JsonValue::Number(3.0),
        )),
    );
    assert_match(&number, "any.kind", payload.clone(), true)?;
    // Nested keypaths traverse; exists ignores values.
    let nested = SignalPattern::new(
        String::from("**"),
        Some(PayloadPredicate::exists(String::from("nested.deep"))),
    );
    assert_match(&nested, "any.kind", payload.clone(), true)?;
    // And/or combine; neq is false on missing keys.
    let combined = SignalPattern::new(
        String::from("**"),
        Some(PayloadPredicate::And(vec![
            PayloadPredicate::exists(String::from("status")),
            PayloadPredicate::Or(vec![
                PayloadPredicate::eq(
                    String::from("status"),
                    JsonValue::String(String::from("nope")),
                ),
                PayloadPredicate::neq(String::from("count"), JsonValue::Number(4.0)),
            ]),
        ])),
    );
    assert_match(&combined, "any.kind", payload.clone(), true)?;
    let neq_missing = SignalPattern::new(
        String::from("**"),
        Some(PayloadPredicate::neq(
            String::from("absent"),
            JsonValue::String(String::from("x")),
        )),
    );
    assert_match(&neq_missing, "any.kind", payload, false)?;
    Ok(())
}

/// Invalid patterns never match, whatever the envelope.
#[test]
fn invalid_patterns_never_match() -> Result<(), String> {
    for bad in ["", "a.**.b"] {
        let pattern = globbed(bad);
        if validate_pattern(&pattern) {
            return Err(format!("{bad:?} must be invalid"));
        }
        if match_pattern(&envelope("a.b", JsonValue::Null)?, &pattern) {
            return Err(format!("{bad:?} must never match"));
        }
    }
    let seventeen = (0..17)
        .map(|index| format!("s{index}"))
        .collect::<Vec<_>>()
        .join(".");
    if validate_pattern(&globbed(&seventeen)) {
        return Err(String::from("17 segments must be invalid"));
    }
    Ok(())
}
