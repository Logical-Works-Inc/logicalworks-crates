//! Protocol types: envelopes, descriptors, attestation evidence.
//!
//! Counterpart of the generated-type behaviors plus `AttestationEvidence`,
//! `AppAttestAttestation`, and producer tests (kernel#850 row 15): envelope
//! round-trips, the `intent.emit` strip, source-kind decode gates,
//! P-256 proof-of-possession, TrustZone quote shape, CBOR error cases, and
//! the fail-closed producer.

#[path = "support/fixtures.rs"]
mod fixtures;

use drishti::canvas_protocol::Capability;
use drishti::native_host::attest::{
    AppAttestError, AppAttestProductionError, BootChallengeReceipt, app_attest_is_supported, apple,
    produce_app_attest_evidence, trustzone,
};
use drishti::native_host::protocol::{
    GovernanceEntry, IntentEnvelope, JsonValue, PredictionEnvelope, SignalEnvelope, SourceKind,
};
use lgwks_deps::p256;
use lgwks_std::json::{Value, json};

/// Envelope fixture.
fn envelope(kind: SourceKind) -> Result<SignalEnvelope, String> {
    Ok(SignalEnvelope::new(
        String::from("signal"),
        String::from("forms.submitted"),
        kind,
        String::from("widget.forms"),
        fixtures::session("tenant")?,
        JsonValue::Object(std::collections::BTreeMap::new()),
        String::from("2026-11-03T00:00:00Z"),
    ))
}

/// Envelopes round-trip through JSON with snake_case keys.
#[test]
fn envelopes_round_trip() -> Result<(), String> {
    let wire = lgwks_std::json::to_string(&envelope(SourceKind::Widget)?)
        .map_err(|error| format!("encode failed: {error}"))?;
    for key in [
        "signal_id",
        "source_kind",
        "source_id",
        "session_invariant",
        "emitted_at",
    ] {
        if !wire.contains(key) {
            return Err(format!("wire must carry {key}"));
        }
    }
    let back: SignalEnvelope =
        lgwks_std::json::from_str(&wire).map_err(|error| format!("decode failed: {error}"))?;
    if back != envelope(SourceKind::Widget)? {
        return Err(String::from("envelope must round-trip"));
    }
    for kind in SourceKind::all() {
        if lgwks_std::json::to_string(&kind).map_err(|error| format!("{error}"))?
            != format!("\"{kind:?}\"").to_lowercase()
        {
            return Err(String::from("source kinds must be lowercase wire strings"));
        }
    }
    Ok(())
}

/// The descriptor strips `intent.emit` as defense-in-depth.
#[test]
fn descriptor_strips_intent_emit() -> Result<(), String> {
    let descriptor = fixtures::widget_descriptor(
        "widget.demo",
        "demo.kind",
        vec![Capability::SignalEmit, Capability::IntentEmit],
    );
    if descriptor.capabilities().contains(&Capability::IntentEmit) {
        return Err(String::from("intent.emit must be stripped"));
    }
    if !descriptor.capabilities().contains(&Capability::SignalEmit) {
        return Err(String::from("granted caps must survive"));
    }
    Ok(())
}

/// Intent envelopes decode only with a user source kind.
#[test]
fn intent_gate_enforces_user_source() -> Result<(), String> {
    let user = json!({
        "signal_id": "s", "kind": "intent.find", "source_kind": "user",
        "source_id": "command-bar:user", "session_invariant": {
            "work_object_id": "w", "session_id": "s",
            "policy_bundle_id": "p", "tenant_id": "t", "user_sub": "u",
        },
        "emitted_at": "2026-11-03T00:00:00Z", "payload": {},
        "raw_text": null, "resolved_kind": null,
    });
    lgwks_std::json::from_value::<IntentEnvelope>(user)
        .map_err(|error| format!("user must decode: {error}"))?;
    let widget = json!({
        "signal_id": "s", "kind": "intent.find", "source_kind": "widget",
        "source_id": "w", "session_invariant": {
            "work_object_id": "w", "session_id": "s",
            "policy_bundle_id": "p", "tenant_id": "t", "user_sub": "u",
        },
        "emitted_at": "2026-11-03T00:00:00Z", "payload": {},
        "raw_text": null, "resolved_kind": null,
    });
    if lgwks_std::json::from_value::<IntentEnvelope>(widget).is_ok() {
        return Err(String::from("widget intent must be rejected"));
    }
    Ok(())
}

/// Constructed intent envelopes retain raw text and resolved kind through
/// JSON.
#[test]
fn intent_envelope_construction_round_trips() -> Result<(), String> {
    let mut envelope = IntentEnvelope::new(
        String::from("intent.find"),
        String::from("signal"),
        SourceKind::User,
        String::from("command-bar:user"),
        fixtures::session("tenant")?,
        String::from("2026-11-03T00:00:00Z"),
        JsonValue::Object(std::collections::BTreeMap::new()),
    );
    envelope.set_raw_text(Some(String::from("/find foo")));
    envelope.set_resolved_kind(Some(String::from("intent.find")));
    if envelope.raw_text() != Some(&String::from("/find foo"))
        || envelope.resolved_kind() != Some(&String::from("intent.find"))
    {
        return Err(String::from("intent text must bind"));
    }
    let wire = lgwks_std::json::to_string(&envelope).map_err(|error| format!("{error}"))?;
    let back: IntentEnvelope =
        lgwks_std::json::from_str(&wire).map_err(|error| format!("{error}"))?;
    if back != envelope {
        return Err(String::from("intent envelope must round-trip"));
    }
    Ok(())
}

/// Prediction envelopes decode only with an ML source kind.
#[test]
fn prediction_gate_enforces_ml_source() -> Result<(), String> {
    let ml = json!({
        "signal_id": "s", "kind": "ml.suggest", "source_kind": "ml",
        "source_id": "tenant", "session_invariant": {
            "work_object_id": "w", "session_id": "s",
            "policy_bundle_id": "p", "tenant_id": "t", "user_sub": "u",
        },
        "emitted_at": "2026-11-03T00:00:00Z", "payload": {},
        "model_version": "m1", "confidence": 0.9,
        "reasoning": "why", "view_directive": null,
    });
    lgwks_std::json::from_value::<PredictionEnvelope>(ml)
        .map_err(|error| format!("ml must decode: {error}"))?;
    let user = json!({
        "signal_id": "s", "kind": "ml.suggest", "source_kind": "user",
        "source_id": "u", "session_invariant": {
            "work_object_id": "w", "session_id": "s",
            "policy_bundle_id": "p", "tenant_id": "t", "user_sub": "u",
        },
        "emitted_at": "2026-11-03T00:00:00Z", "payload": {},
        "model_version": "m1", "confidence": 0.9,
        "reasoning": "why", "view_directive": null,
    });
    if lgwks_std::json::from_value::<PredictionEnvelope>(user).is_ok() {
        return Err(String::from("user prediction must be rejected"));
    }
    Ok(())
}

/// Constructed prediction envelopes retain model detail through JSON.
#[test]
fn prediction_envelope_construction_round_trips() -> Result<(), String> {
    let mut envelope = PredictionEnvelope::new(
        String::from("signal"),
        String::from("ml.suggest"),
        SourceKind::Ml,
        String::from("tenant"),
        String::from("2026-11-03T00:00:00Z"),
        fixtures::session("tenant")?,
        JsonValue::Object(std::collections::BTreeMap::new()),
    );
    envelope.set_model_version(String::from("m1"));
    envelope.set_confidence(0.9);
    envelope.set_reasoning(String::from("why"));
    envelope.set_view_directive(None);
    if envelope.model_version() != "m1"
        || envelope.confidence().to_bits() != 0.9_f64.to_bits()
        || envelope.reasoning() != "why"
        || envelope.view_directive().is_some()
    {
        return Err(String::from("prediction detail must bind"));
    }
    let wire = lgwks_std::json::to_string(&envelope).map_err(|error| format!("{error}"))?;
    let back: PredictionEnvelope =
        lgwks_std::json::from_str(&wire).map_err(|error| format!("{error}"))?;
    if back != envelope {
        return Err(String::from("prediction envelope must round-trip"));
    }
    Ok(())
}

/// Governance entries carry mandatory attribution.
#[test]
fn governance_entries_carry_attribution() -> Result<(), String> {
    let entry = fixtures::governance_entry(
        "signal",
        "session",
        "forms.submitted",
        "2026-11-03T00:00:00Z",
    );
    let wire = lgwks_std::json::to_string(&entry).map_err(|error| format!("{error}"))?;
    let back: GovernanceEntry =
        lgwks_std::json::from_str(&wire).map_err(|error| format!("{error}"))?;
    if back != entry {
        return Err(String::from("governance entry must round-trip"));
    }
    Ok(())
}

/// Deterministic device key for attestation tests (below the curve order).
fn device_key() -> Result<p256::ecdsa::SigningKey, String> {
    p256::ecdsa::SigningKey::from_slice(&[0x42; 32])
        .map_err(|error| format!("fixture key rejected: {error}"))
}

/// Deterministic 32-byte challenge.
fn challenge(byte: u8) -> Vec<u8> {
    vec![byte; 32]
}

/// Receipt fixture.
fn receipt() -> Result<BootChallengeReceipt, String> {
    BootChallengeReceipt::new(
        &challenge(0x11),
        1_000,
        2_000,
        &challenge(0x22),
        1_500,
        &challenge(0x33),
    )
    .map_err(|error| format!("{error}"))
}

/// Receipts refuse non-32-byte challenges.
#[test]
fn receipts_refuse_bad_challenges() -> Result<(), String> {
    if BootChallengeReceipt::new(&[0u8; 31], 0, 0, &[0u8; 32], 0, &[0u8; 32]).is_ok() {
        return Err(String::from("31-byte challenge must fail"));
    }
    receipt()?;
    Ok(())
}

/// Apple packages sign the issued challenge; verification proves possession.
#[test]
fn apple_package_signs_issued_challenge() -> Result<(), String> {
    use p256::ecdsa::signature::Verifier as _;
    let signing = device_key()?;
    let verifying = signing.verifying_key();
    let point = compressed_point(verifying)?;
    let package = apple::make_package(
        vec![0x30, 0x03, 0x01, 0x02],
        vec![vec![0x30, 0x03, 0x01, 0x03]],
        &receipt()?,
        point.as_slice(),
        &signing,
    )
    .map_err(|error| format!("{error}"))?;
    if package.challenge() != challenge(0x11).as_slice() {
        return Err(String::from("package must carry the issued challenge"));
    }
    let signature = p256::ecdsa::Signature::from_der(package.challenge_pop_signature_der())
        .map_err(|error| format!("signature must parse: {error}"))?;
    verifying
        .verify(&package.challenge(), &signature)
        .map_err(|error| format!("possession must verify: {error}"))?;
    // A bad key shape is refused before any crypto runs.
    if apple::make_package(vec![1], vec![], &receipt()?, &[0x04; 33], &signing).is_ok() {
        return Err(String::from("uncompressed key must fail"));
    }
    Ok(())
}

/// Compressed SEC1 point for a verifying key.
fn compressed_point(key: &p256::ecdsa::VerifyingKey) -> Result<Vec<u8>, String> {
    let point = key.to_sec1_point(true);
    Ok(point.as_bytes().to_vec())
}

/// TrustZone quotes carry the DER shape and verify.
#[test]
fn trustzone_quote_shape_verifies() -> Result<(), String> {
    let signing = device_key()?;
    let verifying = signing.verifying_key();
    let spki = vec![0x30, 0x59, 0x30, 0x13];
    let quote = trustzone::make_quote("oem-demo", &challenge(0x44), &spki, &signing)
        .map_err(|error| format!("{error}"))?;
    // Outer SEQ { OCTET body, OCTET signature }.
    if quote.len() < 2 || quote[0] != 0x30 {
        return Err(String::from("quote must be a sequence"));
    }
    let point = compressed_point(verifying)?;
    let package = trustzone::make_package(
        "oem-demo",
        &receipt()?,
        &spki,
        point.as_slice(),
        vec![vec![0x30]],
        &signing,
    )
    .map_err(|error| format!("{error}"))?;
    if package.challenge_receipt() != &receipt()? {
        return Err(String::from("package must carry the receipt"));
    }
    if trustzone::make_quote("oem-demo", &[0x01; 31], &spki, &signing).is_ok() {
        return Err(String::from("short challenge must fail"));
    }
    Ok(())
}

/// Minimal CBOR attestation object for parser tests.
fn attestation_cbor() -> Result<Vec<u8>, String> {
    // {"attStmt": {"x5c": [<3 bytes>, <2 bytes>], "receipt": <1 byte>},
    //  "authData": <2 bytes>, "fmt": "apple-appattest"}
    let mut bytes = vec![0xa3];
    bytes.extend_from_slice(&encode_text("attStmt")?);
    bytes.push(0xa2);
    bytes.extend_from_slice(&encode_text("x5c")?);
    bytes.push(0x82);
    bytes.extend_from_slice(&encode_bytes(&[1_u8, 2_u8, 3_u8])?);
    bytes.extend_from_slice(&encode_bytes(&[4_u8, 5_u8])?);
    bytes.extend_from_slice(&encode_text("receipt")?);
    bytes.extend_from_slice(&encode_bytes(&[6_u8])?);
    bytes.extend_from_slice(&encode_text("authData")?);
    bytes.extend_from_slice(&encode_bytes(&[7_u8, 8_u8])?);
    bytes.extend_from_slice(&encode_text("fmt")?);
    bytes.extend_from_slice(&encode_text("apple-appattest")?);
    Ok(bytes)
}

/// CBOR text string encoder for fixtures.
fn encode_text(text: &str) -> Result<Vec<u8>, String> {
    let length = u8::try_from(text.len()).map_err(|error| format!("fixture too long: {error}"))?;
    let mut out = vec![0x60_u8.wrapping_add(length)];
    out.extend_from_slice(text.as_bytes());
    Ok(out)
}

/// CBOR byte string encoder for fixtures.
fn encode_bytes(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let length = u8::try_from(bytes.len()).map_err(|error| format!("fixture too long: {error}"))?;
    let mut out = vec![0x40_u8.wrapping_add(length)];
    out.extend_from_slice(bytes);
    Ok(out)
}

/// The parser extracts the chain, auth data, and receipt.
#[test]
fn attestation_parses_chain_and_fields() -> Result<(), String> {
    let parsed = drishti::native_host::attest::AppAttestAttestation::parse(&attestation_cbor()?)
        .map_err(|error| format!("{error}"))?;
    if parsed.chain().len() != 2 || parsed.leaf_cert_der() != [1_u8, 2_u8, 3_u8] {
        return Err(String::from("leaf must be first"));
    }
    if parsed.intermediate_certs_der() != [vec![4_u8, 5_u8]] {
        return Err(String::from("intermediates must follow"));
    }
    if parsed.auth_data() != [7_u8, 8_u8] || parsed.receipt() != Some(&vec![6_u8]) {
        return Err(String::from("fields must extract"));
    }
    Ok(())
}

/// Every malformed shape fails with its own case.
#[test]
fn attestation_rejects_malformed_shapes() -> Result<(), String> {
    use drishti::native_host::attest::AppAttestAttestation as Parse;
    // Truncated.
    if Parse::parse(&[0xa3, 0x01]) != Err(AppAttestError::Truncated) {
        return Err(String::from("truncation must fail"));
    }
    // Non-map root.
    if Parse::parse(&[0x82, 0x01, 0x02]) != Err(AppAttestError::NotAMap) {
        return Err(String::from("non-map must fail"));
    }
    // Missing attStmt / x5c / authData.
    let bare = vec![0xa0];
    if Parse::parse(&bare) != Err(AppAttestError::MissingKey(String::from("attStmt"))) {
        return Err(String::from("missing attStmt must fail"));
    }
    // Empty chain.
    let mut empty_chain = vec![0xa1];
    empty_chain.extend_from_slice(&encode_text("attStmt")?);
    empty_chain.push(0xa1);
    empty_chain.extend_from_slice(&encode_text("x5c")?);
    empty_chain.push(0x80);
    if Parse::parse(&empty_chain) != Err(AppAttestError::EmptyCertChain) {
        return Err(String::from("empty chain must fail"));
    }
    // Non-bytes chain entry.
    let mut bad_entry = vec![0xa1];
    bad_entry.extend_from_slice(&encode_text("attStmt")?);
    bad_entry.push(0xa1);
    bad_entry.extend_from_slice(&encode_text("x5c")?);
    bad_entry.push(0x81);
    bad_entry.extend_from_slice(&encode_text("nope")?);
    if Parse::parse(&bad_entry) != Err(AppAttestError::UnexpectedType(String::from("x5c entry"))) {
        return Err(String::from("non-bytes entry must fail"));
    }
    // Unsupported major type (simple value 20).
    if Parse::parse(&[0xf4]) != Err(AppAttestError::UnsupportedMajorType(7)) {
        return Err(String::from("major type 7 must fail"));
    }
    Ok(())
}

/// The producer is fail-closed off-device.
#[test]
fn producer_is_fail_closed() -> Result<(), String> {
    if app_attest_is_supported() {
        return Err(String::from("unsupported here"));
    }
    if produce_app_attest_evidence(&receipt()?)
        != Err(AppAttestProductionError::AppAttestUnavailable)
    {
        return Err(String::from("production must refuse"));
    }
    Ok(())
}

/// Estate JSON [`Value`] drives payload fixtures through the crate boundary.
#[test]
fn estate_json_value_drives_payloads() -> Result<(), String> {
    let payload: Value =
        lgwks_std::json::from_str("{\"tick\":1}").map_err(|error| format!("{error}"))?;
    let envelope = SignalEnvelope::new(
        String::from("signal"),
        String::from("tick"),
        SourceKind::Widget,
        String::from("widget.forms"),
        fixtures::session("tenant")?,
        JsonValue::from_json(&payload),
        String::from("2026-11-03T00:00:00Z"),
    );
    if envelope.payload()
        != &JsonValue::Object(
            [(String::from("tick"), JsonValue::Number(1.0))]
                .into_iter()
                .collect(),
        )
    {
        return Err(String::from("estate JSON must convert exactly"));
    }
    Ok(())
}
