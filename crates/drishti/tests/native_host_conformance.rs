//! Protocol conformance: every absorbed fixture drives its validator.
//!
//! Counterpart of `CanvasProtocolTests/ConformanceTests.swift` plus the
//! fixture-integrity sweep (kernel#850 row 15): valid envelopes decode and
//! validate, invalid envelopes fail at decode or validation, patterns decide
//! as labeled, capabilities assert as labeled, invariants bind UUIDs, and
//! every JSON file under `assets` parses.

use std::path::{Path, PathBuf};

use drishti::canvas_protocol::{Capability, SessionInvariant};
use drishti::native_host::capability::assert_capability;
use drishti::native_host::pattern::match_pattern;
use drishti::native_host::protocol::{
    JsonValue, SignalEnvelope, SignalPattern, validate_envelope, validate_session_invariant,
};
use lgwks_std::json::Value;

/// Absorbed protocol tree.
fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/canvas-protocol")
}

/// Read a file or fail with its path.
fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

/// Sorted JSON files in a directory.
fn fixture_files(relative: &str) -> Result<Vec<PathBuf>, String> {
    let directory = assets().join(relative);
    let mut files = Vec::new();
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| format!("cannot list {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("bad entry: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            files.push(path);
        }
    }
    files.sort();
    (!files.is_empty())
        .then_some(())
        .ok_or_else(|| format!("no fixtures in {}", directory.display()))?;
    Ok(files)
}

/// Read and parse one JSON fixture.
fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = read_file(path)?;
    lgwks_std::json::from_slice(&bytes)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))
}

/// Parse a wire capability string.
fn parse_capability(wire: &str) -> Result<Capability, String> {
    use std::str::FromStr as _;
    Capability::from_str(wire).map_err(|error| format!("unknown capability {wire}: {error:?}"))
}

/// Labeled boolean expectation shared by the capability and invariant suites.
fn want_bool(document: &Value, path: &Path) -> Result<bool, String> {
    match document
        .get("expected")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{} must carry expected", path.display()))?
    {
        "ok" => Ok(true),
        "err" => Ok(false),
        _ => Err(format!("{} has unknown expectation", path.display())),
    }
}

/// Valid envelopes decode and validate.
#[test]
fn valid_envelopes_decode_and_validate() -> Result<(), String> {
    for path in fixture_files("conformance/valid_envelopes")? {
        let document = read_json(&path)?;
        let envelope: SignalEnvelope = lgwks_std::json::from_value(document)
            .map_err(|error| format!("{} must decode: {error}", path.display()))?;
        validate_envelope(&envelope)
            .map_err(|error| format!("{} must validate: {error}", path.display()))?;
    }
    Ok(())
}

/// Invalid envelopes fail at decode or at validation.
#[test]
fn invalid_envelopes_fail() -> Result<(), String> {
    for path in fixture_files("conformance/invalid_envelopes")? {
        let document = read_json(&path)?;
        let input = document
            .get("input")
            .ok_or_else(|| format!("{} must carry input", path.display()))?;
        let decoded: Result<SignalEnvelope, _> = lgwks_std::json::from_value(input.clone());
        // Either gate may refuse; the refusal reason must exist and is kept.
        let rejection = match decoded {
            Err(error) => error.to_string(),
            Ok(envelope) => validate_envelope(&envelope)
                .err()
                .map(|error| error.to_string())
                .ok_or_else(|| format!("{} must not validate", path.display()))?,
        };
        if rejection.is_empty() {
            return Err(format!("{} rejected without a reason", path.display()));
        }
    }
    Ok(())
}

/// Minimal envelope shape for pattern fixtures (the matcher only sees kind
/// and payload, like the kernel).
#[derive(lgwks_std::json::serde::Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct MatchEnvelope {
    /// Signal kind.
    kind: String,
    /// Payload, defaulting to null when absent.
    payload: Option<JsonValue>,
}

/// Dummy session: the matcher never reads it.
fn dummy_session() -> Result<SessionInvariant, String> {
    SessionInvariant::new(
        String::from("11111111-1111-1111-1111-111111111111"),
        String::from("22222222-2222-2222-2222-222222222222"),
        String::from("33333333-3333-3333-3333-333333333333"),
        String::from("tenant"),
        String::from("44444444-4444-4444-4444-444444444444"),
    )
    .map_err(|error| format!("fixture session rejected: {error}"))
}

/// Patterns decide exactly as labeled.
#[test]
fn patterns_decide_as_labeled() -> Result<(), String> {
    for path in fixture_files("conformance/pattern_match")? {
        let document = read_json(&path)?;
        let pattern: SignalPattern = lgwks_std::json::from_value(
            document
                .get("pattern")
                .cloned()
                .ok_or_else(|| format!("{} must carry pattern", path.display()))?,
        )
        .map_err(|error| format!("{} pattern must decode: {error}", path.display()))?;
        let envelope: MatchEnvelope = lgwks_std::json::from_value(
            document
                .get("envelope")
                .cloned()
                .ok_or_else(|| format!("{} must carry envelope", path.display()))?,
        )
        .map_err(|error| format!("{} envelope must decode: {error}", path.display()))?;
        let expected = document
            .get("expected")
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("{} must carry expected", path.display()))?;
        let full = SignalEnvelope::new(
            String::from("signal"),
            envelope.kind,
            drishti::native_host::protocol::SourceKind::Widget,
            String::from("source"),
            dummy_session()?,
            match envelope.payload {
                Some(payload) => payload,
                // Documented: absent payloads default to null.
                None => JsonValue::Null,
            },
            String::from("2026-11-03T00:00:00Z"),
        );
        if match_pattern(&full, &pattern) != expected {
            return Err(format!("{} decided wrong", path.display()));
        }
    }
    Ok(())
}

/// Capability fixtures drive the guard with the labeled outcome.
#[test]
fn capabilities_assert_as_labeled() -> Result<(), String> {
    for path in fixture_files("conformance/capability")? {
        let document = read_json(&path)?;
        let have = document
            .get("have")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{} must carry have", path.display()))?;
        let mut granted = Vec::with_capacity(have.len());
        for item in have {
            let wire = item
                .as_str()
                .ok_or_else(|| format!("{} grants must be strings", path.display()))?;
            granted.push(parse_capability(wire)?);
        }
        let need = document
            .get("need")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{} must carry need", path.display()))?;
        let want = want_bool(&document, &path)?;
        let outcome = assert_capability(&granted, parse_capability(need)?).is_ok();
        if outcome != want {
            return Err(format!("{} decided wrong", path.display()));
        }
    }
    Ok(())
}

/// Invariant fixtures bind UUIDs exactly as labeled.
#[test]
fn invariants_bind_as_labeled() -> Result<(), String> {
    for path in fixture_files("conformance/invariant")? {
        let document = read_json(&path)?;
        let input = document
            .get("input")
            .ok_or_else(|| format!("{} must carry input", path.display()))?;
        let want = want_bool(&document, &path)?;
        let decoded: Result<SessionInvariant, _> = lgwks_std::json::from_value(input.clone());
        let (outcome, reason) = match decoded {
            Ok(session) => (validate_session_invariant(&session).is_ok(), String::new()),
            Err(error) => (false, error.to_string()),
        };
        if outcome != want {
            return Err(format!("{} decided wrong ({reason})", path.display()));
        }
    }
    Ok(())
}

/// Every absorbed JSON file parses: schemas, motion envelopes, rejected
/// fixtures, workspaces, and timelines.
#[test]
fn every_absorbed_json_parses() -> Result<(), String> {
    let roots = [
        "schema",
        "conformance",
        "workspaces",
        "../canvas-motion/preloader-golden",
    ];
    let mut count = 0_usize;
    for root in roots {
        let directory = assets().join(root);
        let mut stack = vec![directory];
        while let Some(current) = stack.pop() {
            let entries = std::fs::read_dir(&current)
                .map_err(|error| format!("cannot list {}: {error}", current.display()))?;
            for entry in entries {
                let entry = entry.map_err(|error| format!("bad entry: {error}"))?;
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|extension| extension.to_str()) == Some("json")
                {
                    read_json(&path)?;
                    count = count.saturating_add(1);
                }
            }
        }
    }
    if count < 150 {
        return Err(format!("expected 150+ JSON files, found {count}"));
    }
    Ok(())
}
