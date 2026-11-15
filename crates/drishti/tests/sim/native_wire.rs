//! Wire sims for the native-host envelope: identifiers, kinds, timestamps and
//! decode gates are fed fat-finger and weird-byte input, and every verdict is
//! compared with a rule written from the documentation, not from the code.

use drishti::canvas_protocol::SessionInvariant;
use drishti::native_host::protocol::{
    EnvelopeError, IntentEnvelope, JsonValue, PredictionEnvelope, SignalEnvelope, SourceKind,
    validate_envelope, validate_session_invariant,
};

use crate::fixtures::{POLICY_BUNDLE, SESSION_ID, USER_SUB, WORK_OBJECT};
use crate::motion_kit::{describe, ensure};
use crate::rng::SimRng;

/// A signal id the validator accepts.
const GOOD_ID: &str = "018f5b3a-4c1d-7000-8e2f-1a2b3c4d5e6f";
/// A stamp the validator accepts.
const GOOD_AT: &str = "2026-05-25T12:00:00Z";
/// A kind the validator accepts.
const GOOD_KIND: &str = "crm.opportunity.updated";
/// Lowercase hexadecimal digits.
const HEX: &[u8] = b"0123456789abcdef";

/// Session wire text with each field named; the constructor is bypassed the
/// way a hostile peer bypasses it.
fn wire_session(work: &str, session: &str, policy: &str, tenant: &str, user: &str) -> String {
    format!(
        "{{\"work_object_id\":\"{work}\",\"session_id\":\"{session}\",\"policy_bundle_id\":\"{policy}\",\"tenant_id\":\"{tenant}\",\"user_sub\":\"{user}\"}}"
    )
}

/// A session decoded from wire text.
fn forged(
    work: &str,
    session: &str,
    policy: &str,
    tenant: &str,
    user: &str,
) -> Result<SessionInvariant, String> {
    lgwks_std::json::from_str(&wire_session(work, session, policy, tenant, user)).map_err(describe)
}

/// A widget envelope over explicit id, kind, stamp and session.
fn envelope(id: &str, kind: &str, emitted_at: &str, session: SessionInvariant) -> SignalEnvelope {
    SignalEnvelope::new(
        String::from(id),
        String::from(kind),
        SourceKind::Widget,
        String::from("source"),
        session,
        JsonValue::Null,
        String::from(emitted_at),
    )
}

/// A seeded run of lowercase hex digits.
fn hex_run(random: &mut SimRng, count: usize) -> Vec<char> {
    (0..count)
        .map(|_| char::from(HEX[random.below_len(HEX.len())]))
        .collect()
}

/// A well-formed hyphenated identifier from the seed.
fn random_uuid(random: &mut SimRng) -> Vec<char> {
    let mut chars = Vec::new();
    for (index, count) in [8_usize, 4, 4, 4, 12].into_iter().enumerate() {
        if index > 0 {
            chars.push('-');
        }
        chars.extend(hex_run(random, count));
    }
    chars
}

/// The documented shape: 36 characters, hyphens after groups of 8-4-4-4-12,
/// lowercase hexadecimal elsewhere.
fn shape_ok(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    chars.len() == 36
        && chars.iter().enumerate().all(|(index, found)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *found == '-'
            } else {
                found.is_ascii_digit() || ('a'..='f').contains(found)
            }
        })
}

/// A well-formed identifier with one seeded fat-finger fault, or none.
fn fat_fingered(random: &mut SimRng) -> String {
    let mut chars = random_uuid(random);
    let position = random.below_len(chars.len());
    match random.below(10) {
        1 => chars.retain(|found| *found != '-'),
        2 => {
            if let Some(slot) = chars.get_mut(position) {
                *slot = 'g';
            }
        }
        3 => {
            chars.pop();
        }
        4 => chars.push('a'),
        5 => {
            chars.insert(0, '{');
            chars.push('}');
        }
        6 => {
            for (offset, found) in "urn:uuid:".chars().enumerate() {
                chars.insert(offset, found);
            }
        }
        7 => {
            if let Some(slot) = chars.get_mut(8) {
                *slot = '_';
            }
        }
        8 => {
            if let Some(slot) = chars.get_mut(position) {
                *slot = '\u{e9}';
            }
        }
        9 => chars.clear(),
        _ => {}
    }
    chars.into_iter().collect()
}

/// Bug caught: a signal id check that admits the simple, braced or urn forms
/// or a one-character slip, so two spellings of one identity coexist.
#[test]
fn sim_wire_signal_id_follows_the_hyphenated_shape() -> Result<(), String> {
    let session = forged(WORK_OBJECT, SESSION_ID, POLICY_BUNDLE, "tenant-w", USER_SUB)?;
    let mut accepted = 0_usize;
    let mut refused = 0_usize;
    for seed in 0..400_u64 {
        let mut random = SimRng::seed(seed);
        let text = fat_fingered(&mut random);
        let verdict = validate_envelope(&envelope(&text, GOOD_KIND, GOOD_AT, session.clone()));
        if shape_ok(&text) {
            accepted = accepted.saturating_add(1);
            ensure(
                verdict == Ok(()),
                &format!("seed {seed}: a well-formed id {text:?} was refused: {verdict:?}"),
            )?;
        } else {
            refused = refused.saturating_add(1);
            ensure(
                verdict == Err(EnvelopeError::InvalidSignalId),
                &format!("seed {seed}: a malformed id {text:?} gave {verdict:?}"),
            )?;
        }
    }
    ensure(
        accepted > 10 && refused > 100,
        "the corpus must hold both classes",
    )
}

/// Bug caught: one of the four session identifiers skipped by the check, or
/// the wrong field named, so a forged binding is blamed on the wrong party.
#[test]
fn sim_wire_each_session_uuid_field_is_checked_and_named() -> Result<(), String> {
    let names = [
        "work_object_id",
        "session_id",
        "policy_bundle_id",
        "user_sub",
    ];
    let good = [WORK_OBJECT, SESSION_ID, POLICY_BUNDLE, USER_SUB];
    for seed in 0..200_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(10_000));
        let slot = random.below_len(4);
        let text = fat_fingered(&mut random);
        let mut fields = good.map(String::from);
        fields[slot].clone_from(&text);
        let session = forged(&fields[0], &fields[1], &fields[2], "tenant-w", &fields[3])?;
        let verdict = validate_session_invariant(&session);
        if shape_ok(&text) {
            ensure(
                verdict == Ok(()),
                &format!("seed {seed}: field {slot} well-formed but {verdict:?}"),
            )?;
        } else {
            ensure(
                verdict == Err(EnvelopeError::InvalidInvariant(names[slot])),
                &format!("seed {seed}: field {slot} {text:?} gave {verdict:?}"),
            )?;
        }
    }
    Ok(())
}

/// The documented kind rule, phrased by segments: 1 to 128 scalar values, no
/// empty segment, no control character.
fn kind_is_valid(kind: &str) -> bool {
    (1..=128).contains(&kind.chars().count())
        && kind
            .split('.')
            .all(|part| !part.is_empty() && !part.chars().any(char::is_control))
}

/// Bug caught: kind validation that counts bytes instead of scalars, lets an
/// empty segment through, or accepts a C0 or C1 control inside a segment.
#[test]
fn sim_wire_kind_matches_the_segment_rule() -> Result<(), String> {
    const PIECES: &[&str] = &[
        "a",
        "bc",
        "",
        "\u{e9}",
        "\u{1f980}",
        "x\ny",
        "tab\t",
        "nul\0",
        "\u{85}",
        "\u{200b}",
        "a b",
        "..",
    ];
    let session = forged(WORK_OBJECT, SESSION_ID, POLICY_BUNDLE, "tenant-w", USER_SUB)?;
    let mut kinds = vec![
        "\u{e9}".repeat(128),
        "\u{e9}".repeat(129),
        "a".repeat(128),
        "a".repeat(129),
        vec!["a"; 64].join("."),
        vec!["a"; 65].join("."),
        "a.".repeat(64),
        String::from("."),
        String::from(".a"),
        String::from("a."),
        String::from("a..b"),
    ];
    for seed in 0..400_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(20_000));
        let count = random.below_len(5).saturating_add(1);
        let mut parts = Vec::new();
        for _ in 0..count {
            if let Some(piece) = random.pick_str(PIECES) {
                parts.push(piece);
            }
        }
        kinds.push(parts.join(if random.below(4) == 0 { "" } else { "." }));
    }
    let mut accepted = 0_usize;
    for kind in &kinds {
        let verdict = validate_envelope(&envelope(GOOD_ID, kind, GOOD_AT, session.clone()));
        if kind_is_valid(kind) {
            accepted = accepted.saturating_add(1);
            ensure(
                verdict == Ok(()),
                &format!("kind {kind:?} refused: {verdict:?}"),
            )?;
        } else {
            ensure(
                verdict == Err(EnvelopeError::InvalidKind),
                &format!("kind {kind:?} gave {verdict:?}"),
            )?;
        }
    }
    ensure(accepted > 20, "the corpus must hold accepted kinds")
}

/// Days in a month by the Gregorian rule, phrased independently of the code.
fn days_in(year: u64, month: u64) -> u64 {
    let leap =
        (year.wrapping_rem(4) == 0 && year.wrapping_rem(100) != 0) || year.wrapping_rem(400) == 0;
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Bug caught: a calendar check that admits February 30, a 24th hour, a leap
/// second, month zero or a year before the epoch as a valid stamp.
#[test]
fn sim_wire_stamp_follows_the_calendar() -> Result<(), String> {
    let session = forged(WORK_OBJECT, SESSION_ID, POLICY_BUNDLE, "tenant-w", USER_SUB)?;
    let mut accepted = 0_usize;
    let mut refused = 0_usize;
    for seed in 0..1_500_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(30_000));
        let year = random.below(150).saturating_add(1_960);
        let month = random.below(14);
        let day = random.below(33);
        let hour = random.below(26);
        let minute = random.below(63);
        let second = random.below(63);
        let stamp = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z");
        let want = year >= 1_970
            && (1..=12).contains(&month)
            && (1..=days_in(year, month)).contains(&day)
            && hour <= 23
            && minute <= 59
            && second <= 59;
        let verdict = validate_envelope(&envelope(GOOD_ID, GOOD_KIND, &stamp, session.clone()));
        if want {
            accepted = accepted.saturating_add(1);
            ensure(verdict == Ok(()), &format!("{stamp}: refused: {verdict:?}"))?;
        } else {
            refused = refused.saturating_add(1);
            ensure(
                verdict == Err(EnvelopeError::InvalidTimestamp),
                &format!("{stamp}: admitted or misnamed: {verdict:?}"),
            )?;
        }
    }
    ensure(
        accepted > 30 && refused > 30,
        "the corpus must hold both classes",
    )
}

/// Bug caught: a suffix check that admits a local offset, a lowercase z or
/// no zone at all, so one instant has several wire spellings.
#[test]
fn sim_wire_stamp_zone_and_fraction_suffixes() -> Result<(), String> {
    let session = forged(WORK_OBJECT, SESSION_ID, POLICY_BUNDLE, "tenant-w", USER_SUB)?;
    let cases = [
        ("2026-05-25T12:00:00Z", true),
        ("2026-05-25T12:00:00.1Z", true),
        ("2026-05-25T12:00:00.123Z", true),
        ("2026-05-25T12:00:00.123456789Z", true),
        ("2026-05-25T12:00:00.Z", false),
        ("2026-05-25T12:00:00.1234567890Z", false),
        ("2026-05-25T12:00:00z", false),
        ("2026-05-25T12:00:00", false),
        ("2026-05-25T12:00:00+00:00", false),
        ("2026-05-25T12:00:00+05:00", false),
        ("2026-05-25T12:00:00-05:00", false),
        ("2026-05-25 12:00:00Z", false),
        ("2026-05-25T12:00:00ZZ", false),
        (" 2026-05-25T12:00:00Z", false),
        ("2026-05-25T12:00:00Z\n", false),
        ("2026-05-25T12:00:00\u{ff1a}Z", false),
        ("", false),
        ("Z", false),
    ];
    for (stamp, want) in cases {
        let verdict = validate_envelope(&envelope(GOOD_ID, GOOD_KIND, stamp, session.clone()));
        ensure(
            verdict.is_ok() == want,
            &format!("{stamp:?}: verdict {verdict:?} but the rule says {want}"),
        )?;
        if !want {
            ensure(
                verdict == Err(EnvelopeError::InvalidTimestamp),
                &format!("{stamp:?}: wrong error {verdict:?}"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: the validator reporting a later fault before an earlier one,
/// so a client fixing errors one by one is sent round in circles.
#[test]
fn sim_wire_first_error_follows_the_documented_order() -> Result<(), String> {
    let expected = [
        Err(EnvelopeError::InvalidSignalId),
        Err(EnvelopeError::InvalidKind),
        Err(EnvelopeError::InvalidTimestamp),
        Err(EnvelopeError::InvalidInvariant("work_object_id")),
        Err(EnvelopeError::InvalidInvariant("session_id")),
        Err(EnvelopeError::InvalidInvariant("policy_bundle_id")),
        Err(EnvelopeError::InvalidInvariant("user_sub")),
        Err(EnvelopeError::InvalidInvariant("tenant_id")),
        Ok(()),
    ];
    for seed in 0..40_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(40_000));
        let garbage = random.text(6);
        for (stage, want) in expected.iter().enumerate() {
            let pick = |index: usize, good: &str, bad: &str| {
                if stage > index {
                    String::from(good)
                } else {
                    String::from(bad)
                }
            };
            let session = forged(
                &pick(3, WORK_OBJECT, &garbage),
                &pick(4, SESSION_ID, &garbage),
                &pick(5, POLICY_BUNDLE, &garbage),
                &pick(7, "tenant-w", ""),
                &pick(6, USER_SUB, &garbage),
            )?;
            let verdict = validate_envelope(&envelope(
                &pick(0, GOOD_ID, &garbage),
                &pick(1, GOOD_KIND, ""),
                &pick(2, GOOD_AT, &garbage),
                session,
            ));
            ensure(
                verdict == *want,
                &format!("seed {seed} stage {stage}: got {verdict:?}, want {want:?}"),
            )?;
        }
    }
    Ok(())
}

/// One named wire field and its JSON text.
type Field = (&'static str, String);

/// The seven required envelope fields.
fn signal_fields(source_kind: &str) -> Vec<Field> {
    vec![
        ("signal_id", format!("\"{GOOD_ID}\"")),
        ("kind", format!("\"{GOOD_KIND}\"")),
        ("source_kind", String::from(source_kind)),
        ("source_id", String::from("\"src\"")),
        (
            "session_invariant",
            wire_session(WORK_OBJECT, SESSION_ID, POLICY_BUNDLE, "tenant-w", USER_SUB),
        ),
        ("emitted_at", format!("\"{GOOD_AT}\"")),
        ("payload", String::from("{\"stage\":\"negotiation\"}")),
    ]
}

/// Fields joined as a JSON object, optionally dropping or replacing one.
fn object(fields: &[Field], target: Option<usize>, replacement: Option<&str>) -> String {
    let parts: Vec<String> = fields
        .iter()
        .enumerate()
        .filter_map(|(index, field)| {
            if target == Some(index) {
                replacement.map(|text| format!("\"{}\":{text}", field.0))
            } else {
                Some(format!("\"{}\":{}", field.0, field.1))
            }
        })
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// The three wire shapes under test.
#[derive(Clone, Copy)]
enum Shape {
    /// Stamped signal envelope.
    Signal,
    /// User intent envelope.
    Intent,
    /// ML prediction envelope.
    Prediction,
}

/// Whether the text decodes as the shape.
fn decodes(shape: Shape, text: &str) -> bool {
    match shape {
        Shape::Signal => lgwks_std::json::from_str::<SignalEnvelope>(text).is_ok(),
        Shape::Intent => lgwks_std::json::from_str::<IntentEnvelope>(text).is_ok(),
        Shape::Prediction => lgwks_std::json::from_str::<PredictionEnvelope>(text).is_ok(),
    }
}

/// Bug caught: a source-kind gate that admits a near miss (`User`, `USER`,
/// padding, a non-string), so a widget can pose as the user or an ML peer.
#[test]
fn sim_wire_source_kind_gates_admit_exactly_one_origin() -> Result<(), String> {
    let spellings = [
        "\"widget\"",
        "\"ml\"",
        "\"system\"",
        "\"user\"",
        "\"User\"",
        "\"USER\"",
        "\"Ml\"",
        "\"\"",
        "\"admin\"",
        "\"ml \"",
        "\"user\\u0000\"",
        "null",
        "5",
        "true",
        "[\"user\"]",
        "{}",
    ];
    for spelling in spellings {
        let mut intent = signal_fields(spelling);
        intent.push(("raw_text", String::from("null")));
        let mut prediction = signal_fields(spelling);
        prediction.push(("model_version", String::from("\"m1\"")));
        prediction.push(("confidence", String::from("0.5")));
        prediction.push(("reasoning", String::from("\"why\"")));
        let signal = decodes(Shape::Signal, &object(&signal_fields(spelling), None, None));
        let from_user = decodes(Shape::Intent, &object(&intent, None, None));
        let from_ml = decodes(Shape::Prediction, &object(&prediction, None, None));
        ensure(
            signal == ["\"widget\"", "\"ml\"", "\"system\"", "\"user\""].contains(&spelling),
            &format!("{spelling}: signal decode {signal} disagrees with the four origins"),
        )?;
        ensure(
            from_user == (spelling == "\"user\""),
            &format!("{spelling}: intent decode {from_user}"),
        )?;
        ensure(
            from_ml == (spelling == "\"ml\""),
            &format!("{spelling}: prediction decode {from_ml}"),
        )?;
    }
    let text = object(&signal_fields("\"widget\""), None, None);
    let decoded: SignalEnvelope = lgwks_std::json::from_str(&text).map_err(describe)?;
    ensure(
        decoded.signal_id() == GOOD_ID
            && decoded.kind() == GOOD_KIND
            && decoded.source_kind() == SourceKind::Widget
            && decoded.emitted_at() == GOOD_AT
            && validate_envelope(&decoded) == Ok(()),
        "a decoded envelope must keep its fields and validate",
    )
}

/// Bug caught: a missing or mistyped field decoding into a default, so a
/// truncated or hand-edited envelope is treated as a complete one.
#[test]
fn sim_wire_missing_and_mistyped_fields_are_refused() -> Result<(), String> {
    let mut intent = signal_fields("\"user\"");
    intent.push(("raw_text", String::from("null")));
    intent.push(("resolved_kind", String::from("null")));
    let mut prediction = signal_fields("\"ml\"");
    prediction.push(("model_version", String::from("\"m1\"")));
    prediction.push(("confidence", String::from("0.5")));
    prediction.push(("reasoning", String::from("\"why\"")));
    prediction.push(("view_directive", String::from("null")));
    let optional = ["raw_text", "resolved_kind", "view_directive"];
    let anything = ["payload", "view_directive"];
    let shapes = [
        (Shape::Signal, signal_fields("\"widget\"")),
        (Shape::Intent, intent),
        (Shape::Prediction, prediction),
    ];
    for (shape, fields) in shapes {
        ensure(
            decodes(shape, &object(&fields, None, None)),
            "the unmutated envelope must decode",
        )?;
        for (index, field) in fields.iter().enumerate() {
            let key = field.0;
            ensure(
                decodes(shape, &object(&fields, Some(index), None)) == optional.contains(&key),
                &format!("removing {key}: decode verdict disagrees with its optionality"),
            )?;
            ensure(
                decodes(shape, &object(&fields, Some(index), Some("[]")))
                    == anything.contains(&key),
                &format!("{key} as an array: decode verdict disagrees with its type"),
            )?;
            ensure(
                decodes(shape, &object(&fields, Some(index), Some("null")))
                    == (optional.contains(&key) || key == "payload"),
                &format!("{key} as null: decode verdict disagrees with its type"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a session that decodes with a hole (missing member) and then
/// validates, so a binding without a tenant or user is admitted.
#[test]
fn sim_wire_session_with_a_hole_never_validates() -> Result<(), String> {
    let members = [
        ("work_object_id", WORK_OBJECT),
        ("session_id", SESSION_ID),
        ("policy_bundle_id", POLICY_BUNDLE),
        ("tenant_id", "tenant-w"),
        ("user_sub", USER_SUB),
    ];
    for hole in 0..members.len() {
        let parts: Vec<String> = members
            .iter()
            .enumerate()
            .filter(|entry| entry.0 != hole)
            .map(|entry| format!("\"{}\":\"{}\"", entry.1.0, entry.1.1))
            .collect();
        let text = format!("{{{}}}", parts.join(","));
        if let Ok(session) = lgwks_std::json::from_str::<SessionInvariant>(&text) {
            ensure(
                validate_session_invariant(&session).is_err(),
                &format!("a session missing member {hole} decoded and validated"),
            )?;
        }
    }
    let whole = wire_session(WORK_OBJECT, SESSION_ID, POLICY_BUNDLE, "tenant-w", USER_SUB);
    let session: SessionInvariant = lgwks_std::json::from_str(&whole).map_err(describe)?;
    ensure(
        validate_session_invariant(&session) == Ok(()),
        "the whole session must validate",
    )
}
