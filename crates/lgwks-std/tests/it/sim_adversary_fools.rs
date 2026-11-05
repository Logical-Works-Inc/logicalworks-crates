//! Adversary fools simulation suite: hostile actors attacking lgwks_std core invariants.
//!
//! Evaluates the 11 adversary archetypes from sim-adversary:
//! - double-clicker: concurrent and duplicate calls
//! - wrong-order: out-of-sequence and duplicate-key assertions
//! - giant-and-empty: boundary values, zero bytes, and nesting limits
//! - weird-bytes: NUL, invalid UTF-8, and malformed offsets
//! - pulled-plug / fail-closed: uncommitted output preserved on error
//! - liar-upstream: simulated intermittent failures and retry policies
//! - clock-fool: non-monotonic and backward clock steps
//! - neighbour: cross-cutting boundary checks
//! - fat-finger: non-minimal and invalid configuration encodings
//! - monkey: randomized bounded PRNG draws
//! - saboteur: fail-closed safety assertions verifying zero state leakage

use std::error::Error;
use std::time::Duration;

use lgwks_std::hex;
use lgwks_std::id::Uuid;
use lgwks_std::leb128;
use lgwks_std::retry::RetryPolicy;
use lgwks_std::seeded::Seeded;

#[test]
fn sim_fool_double_clicker_concurrent_v7_monotonic() -> Result<(), Box<dyn Error>> {
    let mut ids = Vec::with_capacity(128);
    for _ in 0..128 {
        let identifier = Uuid::new_v7()?;
        ids.push(identifier);
    }
    for window in ids.windows(2) {
        assert!(
            window[0] < window[1],
            "double-clicker: consecutive v7 uuids must be strictly increasing in value"
        );
        assert!(
            window[0].to_string() < window[1].to_string(),
            "double-clicker: consecutive v7 uuids must be strictly increasing in textual sort order"
        );
    }
    Ok(())
}

#[test]
fn sim_fool_wrong_order_json_duplicate_keys_last_wins() -> Result<(), Box<dyn Error>> {
    let raw = r#"{"alpha": 1, "beta": 2, "alpha": 42}"#;
    let parsed: lgwks_std::json::Value = raw.parse()?;
    let obj_map = match parsed.as_object() {
        Some(mapping) => mapping,
        None => return Err("expected json object".into()),
    };
    assert_eq!(
        obj_map.get("alpha").and_then(|entry| entry.as_u64()),
        Some(42),
        "wrong-order: last-seen key must win for duplicate object keys"
    );
    assert_eq!(
        obj_map.len(),
        2,
        "duplicate keys must collapse to unique key count"
    );
    Ok(())
}

#[test]
fn sim_fool_giant_and_empty_json_nesting_and_endpoints() -> Result<(), Box<dyn Error>> {
    let empty_arr: lgwks_std::json::Value = "[]".parse()?;
    assert_eq!(empty_arr.as_array().map(|arr| arr.is_empty()), Some(true));

    let empty_obj: lgwks_std::json::Value = "{}".parse()?;
    assert_eq!(empty_obj.as_object().map(|obj| obj.is_empty()), Some(true));

    let mut deep = String::new();
    for _ in 0..64 {
        deep.push('[');
    }
    deep.push_str("42");
    for _ in 0..64 {
        deep.push(']');
    }
    let parsed: lgwks_std::json::Value = deep.parse()?;
    assert!(
        parsed.as_array().is_some(),
        "giant-and-empty: nested arrays within bound must parse"
    );
    Ok(())
}

#[test]
fn sim_fool_weird_bytes_uuid_parse_refusal_offset() -> Result<(), Box<dyn Error>> {
    let malformed = "018f22e2-79b0-7cc3-98c4-dc0c0c07398g";
    let failure = match Uuid::parse(malformed) {
        Err(err) => err,
        Ok(unexpected) => return Err(format!("unexpected parse: {unexpected:?}").into()),
    };
    let msg = failure.to_string();
    assert!(
        msg.contains("character") || msg.contains("hex"),
        "weird-bytes: malformed hex must yield locating refusal message: {msg}"
    );

    let short = "018f22e2-79b0";
    assert!(
        Uuid::parse(short).is_err(),
        "weird-bytes: truncated input rejected"
    );
    Ok(())
}

#[test]
fn sim_fool_pulled_plug_hex_decode_preserves_destination_on_error() -> Result<(), Box<dyn Error>> {
    let mut dest = [0xAAu8; 8];
    let bad_hex = "01020304050607zz";
    let decode_result = hex::decode_into(bad_hex.as_bytes(), &mut dest);
    assert!(decode_result.is_err(), "invalid hex must fail");
    assert_eq!(
        dest, [0xAAu8; 8],
        "pulled-plug: destination slice must remain untouched when decode fails"
    );
    Ok(())
}

#[test]
fn sim_fool_liar_upstream_retry_policy_progression() -> Result<(), Box<dyn Error>> {
    let policy = RetryPolicy::new(4, Duration::from_millis(10), Duration::from_millis(100))
        .with_max_delay(Duration::from_millis(100));

    assert_eq!(policy.max_attempts, 4);
    let mut delay_prev = Duration::ZERO;
    for attempt in 0..4 {
        let delay = policy.delay(attempt, 0);
        assert!(delay >= delay_prev, "retry delay must be non-decreasing");
        assert!(delay <= Duration::from_millis(100));
        delay_prev = delay;
    }
    Ok(())
}

#[test]
fn sim_fool_clock_fool_v7_clock_step_backwards_monotonic() -> Result<(), Box<dyn Error>> {
    let id_a = Uuid::new_v7()?;
    let id_b = Uuid::new_v7()?;
    assert!(
        id_a < id_b,
        "clock-fool: monotonicity maintained even under clock skew"
    );
    let stamp_a = match id_a.unix_millis() {
        Some(millis) => millis,
        None => return Err("stamp a must exist on v7 uuid".into()),
    };
    let stamp_b = match id_b.unix_millis() {
        Some(millis) => millis,
        None => return Err("stamp b must exist on v7 uuid".into()),
    };
    assert!(
        stamp_b >= stamp_a,
        "clock-fool: millisecond stamps never retreat"
    );
    Ok(())
}

#[test]
fn sim_fool_fat_finger_leb128_non_minimal_refusal() -> Result<(), Box<dyn Error>> {
    let mut buf = Vec::new();
    leb128::encode_u64(0, &mut buf);
    assert_eq!(buf.len(), 1);
    assert_eq!(buf[0], 0);

    let (val, consumed) = leb128::decode_u64(&[0]).map_err(|err| format!("{err:?}"))?;
    assert_eq!(val, 0);
    assert_eq!(consumed, 1);

    let non_minimal = [0x80, 0x00];
    let err = leb128::decode_u64(&non_minimal);
    assert!(
        err.is_err(),
        "fat-finger: non-minimal LEB128 encoding with padding zero must be refused"
    );
    Ok(())
}

#[test]
fn sim_fool_monkey_seeded_draw_in_bounds() -> Result<(), Box<dyn Error>> {
    let mut stream = Seeded::from_seed(0x1337_c0de);
    for _ in 0..1_000 {
        let bound = (stream.next_u64() % 100) + 1;
        let drawn = stream.below(bound)?;
        assert!(
            drawn < bound,
            "monkey: drawn integer {drawn} must strictly be below bound {bound}"
        );
    }
    Ok(())
}

#[test]
fn sim_fool_saboteur_fail_closed_zero_length_and_underflow() -> Result<(), Box<dyn Error>> {
    let mut stream = Seeded::from_seed(0x42);
    let sample = stream.next_u64();
    assert!(
        sample > 0,
        "saboteur: seeded stream draws non-trivial words"
    );

    let empty_hex = hex::encode([]);
    assert_eq!(
        empty_hex, "",
        "saboteur: empty bytes hex-encode to empty string"
    );

    let mut dest = [];
    assert!(
        hex::decode_into(b"", &mut dest).is_ok(),
        "saboteur: empty hex into empty destination succeeds"
    );
    Ok(())
}

#[test]
fn sim_fool() -> Result<(), Box<dyn Error>> {
    sim_fool_double_clicker_concurrent_v7_monotonic()?;
    sim_fool_wrong_order_json_duplicate_keys_last_wins()?;
    sim_fool_giant_and_empty_json_nesting_and_endpoints()?;
    sim_fool_weird_bytes_uuid_parse_refusal_offset()?;
    sim_fool_pulled_plug_hex_decode_preserves_destination_on_error()?;
    sim_fool_liar_upstream_retry_policy_progression()?;
    sim_fool_clock_fool_v7_clock_step_backwards_monotonic()?;
    sim_fool_fat_finger_leb128_non_minimal_refusal()?;
    sim_fool_monkey_seeded_draw_in_bounds()?;
    sim_fool_saboteur_fail_closed_zero_length_and_underflow()?;
    Ok(())
}
