//! Envelope and entry wire sims: seeded signal, intent and prediction
//! envelopes, widget descriptors, attributions and governance entries round
//! trip their wire text bit for bit, refuse every missing or mistyped field,
//! and replay to the same bytes from the same seed.

use std::collections::BTreeMap;

use drishti::canvas_protocol::Capability;
use drishti::native_host::protocol::{
    Attribution, AttributionKind, GovernanceEntry, IntentEnvelope, JsonValue, PredictionEnvelope,
    SignalEnvelope, SourceKind, WidgetDescriptor, WidgetSurface, validate_envelope,
};

use crate::motion_kit::{describe, ensure, with_member};
use crate::protocol_vocab::{closed_set, seeded_uuid};
use crate::rng::{SimRng, TraceHash};

/// A seeded JSON value of bounded depth with exactly representable numbers.
fn random_value(random: &mut SimRng, depth: u8) -> JsonValue {
    let pick = if depth == 0 {
        random.below(4)
    } else {
        random.below(6)
    };
    match pick {
        0 => JsonValue::Null,
        1 => JsonValue::Bool(random.below(2) == 0),
        2 => JsonValue::Number(f64::from(random.below_i32(4001)) / 8.0 - 250.0),
        3 => {
            let length = random.below_len(8);
            JsonValue::String(random.text(length))
        }
        4 => {
            let count = random.below_len(4);
            JsonValue::Array(
                (0..count)
                    .map(|_| random_value(random, depth.saturating_sub(1)))
                    .collect(),
            )
        }
        _ => {
            let count = random.below_len(4);
            let mut map = BTreeMap::new();
            for _ in 0..count {
                let length = random.below_len(5).saturating_add(1);
                map.insert(
                    random.text(length),
                    random_value(random, depth.saturating_sub(1)),
                );
            }
            JsonValue::Object(map)
        }
    }
}

/// A seeded stamp on a fixed day.
fn stamp(random: &mut SimRng) -> String {
    format!(
        "2026-11-03T10:{:02}:{:02}.{:03}Z",
        random.below(60),
        random.below(60),
        random.below(1000)
    )
}

/// A seeded signal envelope.
fn signal(random: &mut SimRng) -> Result<SignalEnvelope, String> {
    let tenant_length = random.below_len(10).saturating_add(1);
    let tenant = random.text(tenant_length);
    let kind = format!("k{}.{}", random.below(100), random.below(100));
    let source = SourceKind::all()[random.below_len(4)];
    let source_id = format!("widget.{}", random.below(1000));
    Ok(SignalEnvelope::new(
        seeded_uuid(random),
        kind,
        source,
        source_id,
        crate::fixtures::session(&tenant)?,
        random_value(random, 3),
        stamp(random),
    ))
}

/// The JSON text of a value.
fn text_of<T: lgwks_std::json::serde::Serialize>(value: &T) -> Result<String, String> {
    lgwks_std::json::to_string(value).map_err(describe)
}

/// Rename a member so the decoder sees it as absent.
fn without(text: &str, key: &str) -> String {
    text.replacen(&format!("\"{key}\":"), &format!("\"z{key}\":"), 1)
}

/// Bug caught: a signal envelope that changes on the wire, writes different
/// text the second time, or fails the format validation it passed before
/// encoding.
#[test]
fn sim_envelope_signal_roundtrips_and_stays_valid() -> Result<(), String> {
    for seed in 0..200_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(700_000));
        let envelope = signal(&mut random)?;
        ensure(
            validate_envelope(&envelope).is_ok(),
            &format!("seed {seed}: a stamped envelope fails validation"),
        )?;
        let text = text_of(&envelope)?;
        let back: SignalEnvelope = lgwks_std::json::from_str(&text).map_err(describe)?;
        ensure(
            back == envelope,
            &format!("seed {seed}: the envelope changed on the wire"),
        )?;
        ensure(
            text_of(&back)? == text && validate_envelope(&back).is_ok(),
            &format!("seed {seed}: the second encoding or its validation differs"),
        )?;
    }
    Ok(())
}

/// A seeded intent envelope with optional text fields.
fn intent(random: &mut SimRng) -> Result<IntentEnvelope, String> {
    let mut envelope = IntentEnvelope::new(
        format!("intent.{}", random.below(50)),
        seeded_uuid(random),
        SourceKind::User,
        format!("user.{}", random.below(50)),
        crate::fixtures::session("tenant-i")?,
        stamp(random),
        random_value(random, 2),
    );
    if random.below(2) == 0 {
        let length = random.below_len(20);
        envelope.set_raw_text(Some(random.text(length)));
    }
    if random.below(2) == 0 {
        envelope.set_resolved_kind(Some(format!("resolved.{}", random.below(9))));
    }
    Ok(envelope)
}

/// Bug caught: an intent envelope that loses its optional raw text or resolved
/// kind, invents one when the member is absent, or lets a non-user origin
/// through the decode gate.
#[test]
fn sim_envelope_intent_keeps_optional_text_and_user_gate() -> Result<(), String> {
    for seed in 0..200_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(710_000));
        let envelope = intent(&mut random)?;
        let text = text_of(&envelope)?;
        let back: IntentEnvelope = lgwks_std::json::from_str(&text).map_err(describe)?;
        ensure(
            back == envelope
                && back.raw_text() == envelope.raw_text()
                && back.resolved_kind() == envelope.resolved_kind()
                && back.source_kind() == SourceKind::User,
            &format!("seed {seed}: the intent changed on the wire"),
        )?;
        let absent: IntentEnvelope =
            lgwks_std::json::from_str(&without(&without(&text, "raw_text"), "resolved_kind"))
                .map_err(describe)?;
        ensure(
            absent.raw_text().is_none() && absent.resolved_kind().is_none(),
            &format!("seed {seed}: an absent optional member decoded as present"),
        )?;
        for other in ["widget", "ml", "system"] {
            let forged = with_member(&text, "source_kind", &format!("\"{other}\""))?;
            ensure(
                lgwks_std::json::from_str::<IntentEnvelope>(&forged).is_err(),
                &format!("seed {seed}: an intent claiming to be {other} decoded"),
            )?;
        }
    }
    Ok(())
}

/// A prediction envelope with the given confidence.
fn prediction(confidence: f64) -> Result<PredictionEnvelope, String> {
    let mut envelope = PredictionEnvelope::new(
        String::from("11111111-1111-1111-1111-111111111111"),
        String::from("predict.next"),
        SourceKind::Ml,
        String::from("model.a"),
        String::from("2026-11-03T10:00:00.000Z"),
        crate::fixtures::session("tenant-p")?,
        JsonValue::Null,
    );
    envelope.set_model_version(String::from("v1"));
    envelope.set_confidence(confidence);
    envelope.set_reasoning(String::from("because"));
    envelope.set_view_directive(Some(JsonValue::Bool(true)));
    Ok(envelope)
}

/// Bug caught: a confidence that rounds on the wire, or a not-a-number or
/// infinite confidence that comes back as a different finite number.
#[test]
fn sim_envelope_prediction_confidence_is_exact_or_refused() -> Result<(), String> {
    let mut values = vec![
        0.0,
        1.0,
        0.5,
        0.1,
        1e-300,
        f64::MAX,
        f64::MIN_POSITIVE,
        -1.0,
        1.000_000_000_000_000_2,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    let mut random = SimRng::seed(720_000);
    for _ in 0..200 {
        values.push(f64::from_bits(random.next_u64()));
    }
    for confidence in values {
        let envelope = prediction(confidence)?;
        let outcome = lgwks_std::json::to_string(&envelope)
            .map_err(describe)
            .and_then(|text| {
                lgwks_std::json::from_str::<PredictionEnvelope>(&text).map_err(describe)
            });
        if confidence.is_finite() {
            let back = outcome?;
            ensure(
                back.confidence().to_bits() == confidence.to_bits(),
                &format!("{confidence:e}: came back as {:e}", back.confidence()),
            )?;
        } else if let Ok(back) = outcome {
            ensure(
                !back.confidence().is_finite(),
                &format!(
                    "{confidence:e}: came back as the finite {:e}",
                    back.confidence()
                ),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a prediction envelope that decodes with a required member
/// missing or mistyped, so a model peer can omit its own provenance.
#[test]
fn sim_envelope_prediction_refuses_missing_and_mistyped_members() -> Result<(), String> {
    let text = text_of(&prediction(0.25)?)?;
    ensure(
        lgwks_std::json::from_str::<PredictionEnvelope>(&text).is_ok(),
        "the clean prediction must decode",
    )?;
    for key in [
        "signal_id",
        "kind",
        "source_id",
        "source_kind",
        "session_invariant",
        "payload",
        "emitted_at",
        "model_version",
        "confidence",
        "reasoning",
    ] {
        ensure(
            lgwks_std::json::from_str::<PredictionEnvelope>(&without(&text, key)).is_err(),
            &format!("a prediction without {key} decoded"),
        )?;
    }
    for (key, value) in [
        ("confidence", "\"high\""),
        ("confidence", "true"),
        ("confidence", "null"),
        ("model_version", "5"),
        ("model_version", "null"),
        ("emitted_at", "7"),
        ("signal_id", "[]"),
        ("source_kind", "\"widget\""),
        ("source_kind", "\"ML\""),
        ("source_kind", "1"),
        ("reasoning", "false"),
    ] {
        let mutated = with_member(&text, key, value)?;
        ensure(
            lgwks_std::json::from_str::<PredictionEnvelope>(&mutated).is_err(),
            &format!("{key} = {value} decoded"),
        )?;
    }
    let absent = without(&text, "view_directive");
    let back: PredictionEnvelope = lgwks_std::json::from_str(&absent).map_err(describe)?;
    ensure(
        back.view_directive().is_none(),
        "an absent view directive stays absent",
    )
}

/// Bug caught: a descriptor that lets `intent.emit` through, reorders or drops
/// other capabilities, or flips a mount flag on the wire.
#[test]
fn sim_envelope_descriptor_strips_intent_emit_and_roundtrips() -> Result<(), String> {
    for seed in 0..200_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(730_000));
        let all = Capability::all();
        let count = random.below_len(10);
        let asked: Vec<Capability> = (0..count)
            .map(|_| all[random.below_len(all.len())].clone())
            .collect();
        let want: Vec<Capability> = asked
            .iter()
            .filter(|capability| **capability != Capability::IntentEmit)
            .cloned()
            .collect();
        let surface = [
            WidgetSurface::Web,
            WidgetSurface::Native,
            WidgetSurface::Both,
        ][random.below_len(3)];
        let mut descriptor = WidgetDescriptor::new(
            String::from("kind-a"),
            format!("widget.d{seed}"),
            String::from("1.2.3"),
            surface,
            Vec::new(),
            Vec::new(),
            asked,
        );
        ensure(
            descriptor.capabilities() == want && !descriptor.self_listen() && !descriptor.pinned(),
            &format!("seed {seed}: the constructor kept intent.emit or set a flag"),
        )?;
        descriptor.set_self_listen(random.below(2) == 0);
        descriptor.set_pinned(random.below(2) == 0);
        let text = text_of(&descriptor)?;
        let back: WidgetDescriptor = lgwks_std::json::from_str(&text).map_err(describe)?;
        ensure(
            back == descriptor
                && back.self_listen() == descriptor.self_listen()
                && back.pinned() == descriptor.pinned()
                && back.surface() == surface,
            &format!("seed {seed}: the descriptor changed on the wire"),
        )?;
    }
    Ok(())
}

/// A seeded governance entry.
fn entry(random: &mut SimRng) -> GovernanceEntry {
    let kind = [
        AttributionKind::User,
        AttributionKind::Widget,
        AttributionKind::Ml,
        AttributionKind::System,
    ][random.below_len(4)];
    let model = (kind == AttributionKind::Ml).then(|| format!("v{}", random.below(9)));
    let confidence =
        (kind == AttributionKind::Ml).then(|| f64::from(random.below_i32(101)) / 100.0);
    let mut entry = GovernanceEntry::new(
        seeded_uuid(random),
        format!("entry-{}", random.below(1000)),
        seeded_uuid(random),
        seeded_uuid(random),
        format!("kind.{}", random.below(20)),
        String::from("seeded"),
        Attribution::new(
            kind,
            format!("source.{}", random.below(50)),
            model,
            confidence,
        ),
    );
    entry.set_recorded_at(stamp(random));
    entry.set_tenant_id(format!("tenant-{}", random.below(5)));
    entry
}

/// Bug caught: a governance entry that changes on the wire, decodes with a
/// required member missing, or invents a tenant when the member is absent.
#[test]
fn sim_envelope_governance_entry_wire_defaults_and_refusals() -> Result<(), String> {
    for seed in 0..150_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(740_000));
        let original = entry(&mut random);
        let text = text_of(&original)?;
        let back: GovernanceEntry = lgwks_std::json::from_str(&text).map_err(describe)?;
        ensure(
            back == original,
            &format!("seed {seed}: the entry changed on the wire"),
        )?;
        let unscoped: GovernanceEntry =
            lgwks_std::json::from_str(&without(&text, "tenant_id")).map_err(describe)?;
        ensure(
            unscoped.tenant_id().is_empty() && unscoped.signal_id() == original.signal_id(),
            &format!("seed {seed}: a missing tenant must decode empty, never another tenant"),
        )?;
        for key in [
            "entry_id",
            "session_id",
            "work_object_id",
            "signal_id",
            "event_type",
            "description",
            "attribution",
            "recorded_at",
        ] {
            ensure(
                lgwks_std::json::from_str::<GovernanceEntry>(&without(&text, key)).is_err(),
                &format!("seed {seed}: an entry without {key} decoded"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: an attribution that drops its model detail, turns a missing
/// confidence into zero, or accepts an unknown kind.
#[test]
fn sim_envelope_attribution_keeps_optional_model_detail() -> Result<(), String> {
    for seed in 0..100_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(750_000));
        let model = (random.below(2) == 0).then(|| format!("v{}", random.below(9)));
        let confidence = (random.below(2) == 0).then(|| f64::from(random.below_i32(1001)) / 1000.0);
        let attribution = Attribution::new(
            AttributionKind::Ml,
            String::from("model"),
            model,
            confidence,
        );
        let text = text_of(&attribution)?;
        let back: Attribution = lgwks_std::json::from_str(&text).map_err(describe)?;
        ensure(
            back == attribution
                && back.model_version() == attribution.model_version()
                && back.confidence().map(f64::to_bits)
                    == attribution.confidence().map(f64::to_bits),
            &format!("seed {seed}: the attribution changed on the wire"),
        )?;
        let bare: Attribution =
            lgwks_std::json::from_str(&without(&without(&text, "model_version"), "confidence"))
                .map_err(describe)?;
        ensure(
            bare.model_version().is_none() && bare.confidence().is_none(),
            &format!("seed {seed}: absent model detail decoded as present"),
        )?;
        for bad in ["\"admin\"", "\"User\"", "0", "null"] {
            ensure(
                lgwks_std::json::from_str::<Attribution>(&with_member(&text, "kind", bad)?)
                    .is_err(),
                &format!("seed {seed}: an attribution of kind {bad} decoded"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a source or attribution kind vocabulary that accepts a
/// re-cased or padded name, or a duplicate in the closed set.
#[test]
fn sim_envelope_source_vocabularies_are_closed() -> Result<(), String> {
    closed_set(&SourceKind::all(), &["widget", "ml", "system", "user"])?;
    closed_set(
        &[
            AttributionKind::User,
            AttributionKind::Widget,
            AttributionKind::Ml,
            AttributionKind::System,
        ],
        &["user", "widget", "ml", "system"],
    )?;
    closed_set(
        &[
            WidgetSurface::Web,
            WidgetSurface::Native,
            WidgetSurface::Both,
        ],
        &["web", "native", "both"],
    )
}

/// Bug caught: the estate-value conversion and the serde decode reading the
/// same text differently (a boolean as a number, a large integer rounded one
/// way and not the other), so two readers of one payload disagree. Unlike the
/// `native_json` roundtrip, the text here comes from the crate's own writer
/// and a table of hostile literals (integers past 2^53, `-0`, `1e308`), and
/// the oracle is agreement between the two readers.
#[test]
fn sim_envelope_value_conversion_agrees_with_decode() -> Result<(), String> {
    for seed in 0..300_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(760_000));
        let value = random_value(&mut random, 3);
        let text = text_of(&value)?;
        let estate: lgwks_std::json::Value = lgwks_std::json::from_str(&text).map_err(describe)?;
        let decoded: JsonValue = lgwks_std::json::from_str(&text).map_err(describe)?;
        ensure(
            JsonValue::from_json(&estate) == value && decoded == value,
            &format!("seed {seed}: {text} reads differently through the two paths"),
        )?;
    }
    for text in [
        "true",
        "false",
        "null",
        "1",
        "-0",
        "1.0",
        "9007199254740993",
        "18446744073709551615",
        "-9223372036854775808",
        "1e308",
        "\"1\"",
        "[true,1,\"1\",null]",
        "{\"a\":{\"b\":[]}}",
    ] {
        let estate = lgwks_std::json::from_str::<lgwks_std::json::Value>(text);
        let decoded = lgwks_std::json::from_str::<JsonValue>(text);
        match (estate, decoded) {
            (Ok(raw), Ok(typed)) => ensure(
                JsonValue::from_json(&raw) == typed,
                &format!("{text}: the two readers disagree"),
            )?,
            (Err(_), Err(_)) => {}
            (raw, typed) => return Err(format!("{text}: one reader refused: {raw:?} / {typed:?}")),
        }
    }
    ensure(
        lgwks_std::json::from_str::<JsonValue>("true").map_err(describe)? == JsonValue::Bool(true)
            && lgwks_std::json::from_str::<JsonValue>("1").map_err(describe)?
                == JsonValue::Number(1.0),
        "a boolean is never a number",
    )
}

/// Bug caught: encoded envelope text that depends on anything but the seed
/// (hash-map order, an address, the clock), so two replays of one seed write
/// different bytes.
#[test]
fn sim_envelope_text_replays_identically_per_seed() -> Result<(), String> {
    let run = |seed: u64| -> Result<u64, String> {
        let mut random = SimRng::seed(seed);
        let mut trace = TraceHash::new();
        for _ in 0..40 {
            trace.mix_str(&text_of(&signal(&mut random)?)?);
            trace.mix_str(&text_of(&intent(&mut random)?)?);
            trace.mix_str(&text_of(&entry(&mut random))?);
        }
        Ok(trace.finish())
    };
    for seed in [770_000_u64, 770_001, 770_002] {
        ensure(
            run(seed)? == run(seed)?,
            &format!("seed {seed}: two replays differ"),
        )?;
    }
    ensure(
        run(770_000)? != run(770_001)?,
        "different seeds must write different text",
    )
}
