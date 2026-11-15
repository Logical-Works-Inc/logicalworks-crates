//! Capability guard contract.
//!
//! Counterpart of `CanvasProtocolTests/CapabilityTests.swift` (kernel#850 row
//! 15): ML-only and privileged sets, assertion outcomes, and wire strings.

use drishti::canvas_protocol::Capability;
use drishti::native_host::capability::{
    CapabilityError, assert_capability, is_ml_only, is_privileged,
};

/// Assert the ML-only set: tape.read and view.inject only.
#[test]
fn ml_only_set_is_exact() -> Result<(), String> {
    for capability in [Capability::TapeRead, Capability::ViewInject] {
        if !is_ml_only(&capability) {
            return Err(format!("{capability} must be ML-only"));
        }
    }
    for capability in [
        Capability::SignalEmit,
        Capability::SignalSubscribe,
        Capability::IntentEmit,
        Capability::MotionSchedule,
        Capability::Shred,
    ] {
        if is_ml_only(&capability) {
            return Err(format!("{capability} must not be ML-only"));
        }
    }
    Ok(())
}

/// Assert the privileged set: intent.emit, tape.read, view.inject.
#[test]
fn privileged_set_is_exact() -> Result<(), String> {
    for capability in [
        Capability::IntentEmit,
        Capability::TapeRead,
        Capability::ViewInject,
    ] {
        if !is_privileged(&capability) {
            return Err(format!("{capability} must be privileged"));
        }
    }
    for capability in [
        Capability::SignalEmit,
        Capability::SignalSubscribe,
        Capability::MotionSchedule,
        Capability::MotionObserve,
        Capability::MotionPatch,
        Capability::MotionPluginRegister,
        Capability::MotionReplay,
        Capability::Shred,
        Capability::Rtbf,
        Capability::RemoteCompute,
    ] {
        if is_privileged(&capability) {
            return Err(format!("{capability} must not be privileged"));
        }
    }
    // intent.emit is privileged without being ML-only.
    if is_ml_only(&Capability::IntentEmit) {
        return Err(String::from("intent.emit must not be ML-only"));
    }
    Ok(())
}

/// Assertion succeeds when granted, fails closed otherwise, including empty.
#[test]
fn assertion_follows_the_granted_set() -> Result<(), String> {
    assert_capability(&[Capability::SignalEmit], Capability::SignalEmit)
        .map_err(|error| format!("granted must succeed: {error}"))?;
    let Err(error) = assert_capability(&[Capability::SignalEmit], Capability::TapeRead) else {
        return Err(String::from("missing grant must fail"));
    };
    if error != CapabilityError::NotGranted(Capability::TapeRead) {
        return Err(format!("wrong error: {error}"));
    }
    let Err(error) = assert_capability(&[], Capability::SignalSubscribe) else {
        return Err(String::from("empty grants must fail"));
    };
    if error != CapabilityError::NotGranted(Capability::SignalSubscribe) {
        return Err(format!("wrong error: {error}"));
    }
    Ok(())
}

/// Wire strings match the schema vocabulary on both surfaces.
#[test]
fn wire_strings_match_schema() -> Result<(), String> {
    let cases = [
        (Capability::SignalEmit, "signal.emit"),
        (Capability::SignalSubscribe, "signal.subscribe"),
        (Capability::TapeRead, "tape.read"),
        (Capability::ViewInject, "view.inject"),
        (Capability::IntentEmit, "intent.emit"),
    ];
    for (capability, wire) in cases {
        if capability.to_string() != wire {
            return Err(format!("display drift for {wire}"));
        }
        let encoded = lgwks_std::json::to_string(&capability)
            .map_err(|error| format!("encode failed: {error}"))?;
        if encoded != format!("\"{wire}\"") {
            return Err(format!("serde drift for {wire}: {encoded}"));
        }
    }
    Ok(())
}
