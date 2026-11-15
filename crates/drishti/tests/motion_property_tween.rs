//! Tween purity: NaN and infinity never enter a tween, and evaluation is a
//! pure function of (from, to, time, easing).
//!
//! Counterpart of `motion-core/tests/property_tween_purity.rs` (kernel#850
//! row 15).

use drishti::motion::easing::{Easing, NamedCurve};
use drishti::motion::error::MotionErrorCode;
use drishti::motion::node::tween::TweenComponent;
use proptest::prelude::*;

/// Small easing pool for generated tweens.
fn arb_named_easing() -> impl Strategy<Value = Easing> {
    prop_oneof![
        Just(Easing::named(NamedCurve::Linear)),
        Just(Easing::named(NamedCurve::CubicIn)),
        Just(Easing::named(NamedCurve::CubicOut)),
        Just(Easing::named(NamedCurve::CubicInOut)),
        Just(Easing::named(NamedCurve::ExpoOut)),
        Just(Easing::named(NamedCurve::SineInOut)),
    ]
}

/// Valid tween parameter tuples; construction happens in the test bodies.
fn arb_tween_params() -> impl Strategy<Value = (f32, f32, f32, f32, Easing)> {
    (
        -1000.0_f32..1000.0_f32,
        -1000.0_f32..1000.0_f32,
        0.0_f32..10000.0_f32,
        1.0_f32..10000.0_f32,
        arb_named_easing(),
    )
}

/// Build a tween or fail the case.
fn make_tween(
    from: f32,
    to: f32,
    start_ms: f32,
    duration_ms: f32,
    easing: Easing,
) -> Result<TweenComponent, TestCaseError> {
    TweenComponent::new(from, to, start_ms, duration_ms, easing)
        .map_err(|error| TestCaseError::fail(format!("valid tween rejected: {error}")))
}

/// Assert one constructor rejection with the invalid-value code.
fn assert_rejected(
    result: Result<TweenComponent, drishti::motion::MotionError>,
) -> Result<(), String> {
    let error = result
        .err()
        .ok_or_else(|| String::from("non-finite field must be rejected"))?;
    (error.code() == MotionErrorCode::PatchInvalid)
        .then_some(())
        .ok_or_else(|| format!("wrong code: {}", error.code()))
}

/// NaN and infinite durations are rejected.
#[test]
fn rejects_non_finite_duration() -> Result<(), String> {
    let easing = || Easing::named(NamedCurve::Linear);
    assert_rejected(TweenComponent::new(0.0, 1.0, 0.0, f32::NAN, easing()))?;
    assert_rejected(TweenComponent::new(0.0, 1.0, 0.0, f32::INFINITY, easing()))?;
    assert_rejected(TweenComponent::new(
        0.0,
        1.0,
        0.0,
        f32::NEG_INFINITY,
        easing(),
    ))?;
    Ok(())
}

/// NaN and infinite endpoints are rejected.
#[test]
fn rejects_non_finite_endpoints() -> Result<(), String> {
    let easing = || Easing::named(NamedCurve::Linear);
    assert_rejected(TweenComponent::new(f32::NAN, 1.0, 0.0, 600.0, easing()))?;
    assert_rejected(TweenComponent::new(0.0, f32::NAN, 0.0, 600.0, easing()))?;
    assert_rejected(TweenComponent::new(
        f32::INFINITY,
        1.0,
        0.0,
        600.0,
        easing(),
    ))?;
    assert_rejected(TweenComponent::new(
        0.0,
        f32::INFINITY,
        0.0,
        600.0,
        easing(),
    ))?;
    Ok(())
}

proptest! {
    #[test]
    fn evaluation_is_deterministic(
        (from, to, start_ms, duration_ms, easing) in arb_tween_params(),
        time in 0.0_f32..=1.0_f32,
    ) {
        let tween = make_tween(from, to, start_ms, duration_ms, easing)?;
        let first = tween.value_at(time);
        let second = tween.value_at(time);
        prop_assert_eq!(first.to_bits(), second.to_bits());
    }

    #[test]
    fn returns_from_at_zero(
        (from, to, start_ms, duration_ms, easing) in arb_tween_params(),
    ) {
        let tween = make_tween(from, to, start_ms, duration_ms, easing)?;
        let value = tween.value_at(0.0);
        prop_assert!((value - from).abs() < 1e-5);
    }

    #[test]
    fn returns_to_at_one(
        (from, to, start_ms, duration_ms, easing) in arb_tween_params(),
    ) {
        let tween = make_tween(from, to, start_ms, duration_ms, easing)?;
        let value = tween.value_at(1.0);
        prop_assert!((value - to).abs() < 1e-5);
    }

    #[test]
    fn output_stays_finite(
        (from, to, start_ms, duration_ms, easing) in arb_tween_params(),
        time in 0.0_f32..=1.0_f32,
    ) {
        let tween = make_tween(from, to, start_ms, duration_ms, easing)?;
        prop_assert!(tween.value_at(time).is_finite());
    }
}
