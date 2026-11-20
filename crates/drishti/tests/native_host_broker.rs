//! Broker contract: registration, stamping, fan-out, rate limits, taps.
//!
//! Counterpart of `CanvasBrokerTests` (Broker, Abuse, Fuzz, CommandBar),
//! `CanvasCommandBarTests/CommandBarTests`, and the re-homed issues
//! (tenant-scoped subscriptions, command-bar capability gate, audit log).
//! Time, identity, grant verification, and audit are injected and frozen;
//! the only wall-clock read is a generous fan-out latency bound.

#[path = "support/broker.rs"]
mod broker;
#[path = "support/fixtures.rs"]
mod fixtures;

use std::sync::{Arc, Mutex};

use drishti::canvas_protocol::Capability;
use drishti::native_host::broker::{
    BrokerError, GrantVerificationResult, MlHandle, RegisteredHandle, SignedMlGrant, TapListener,
};
use drishti::native_host::protocol::{JsonValue, SignalPattern, SourceKind};
use lgwks_std::id::Uuid;

use broker::{
    Harness, SESSION_ONE, SESSION_SHARED, SESSION_TWO, collector, lock_vec, own_subscriptions,
    register, register_listen, session, subscribe_all, tap_own, untap_ok, widget,
};
use fixtures::SESSION_ID;

/// Registration succeeds and stamping is authoritative.
#[test]
fn register_and_stamp_authoritatively() -> Result<(), String> {
    let harness = Harness::valid();
    let handle = register(&harness, "widget.a", "tenant", SESSION_ID)?;
    let envelope = harness
        .broker()
        .emit(&handle, "demo.event", JsonValue::String(String::from("hi")))
        .map_err(|error| format!("emit failed: {error}"))?;
    if envelope.kind() != "demo.event" {
        return Err(String::from("kind must round-trip"));
    }
    if envelope.source_kind() != SourceKind::Widget {
        return Err(String::from("widget emit must stamp widget origin"));
    }
    if envelope.source_id() != "widget.a" {
        return Err(String::from("source id must come from registration"));
    }
    if envelope.session_invariant() != &session("tenant", SESSION_ID)? {
        return Err(String::from("session must come from registration"));
    }
    if envelope.payload() != &JsonValue::String(String::from("hi")) {
        return Err(String::from("payload must round-trip"));
    }
    if lgwks_std::id::Uuid::parse(envelope.signal_id()).is_err() {
        return Err(String::from("signal id must be a UUID"));
    }
    if envelope.emitted_at().is_empty() || !envelope.emitted_at().ends_with('Z') {
        return Err(String::from("stamp must carry ISO time"));
    }
    // One audit entry with the same signal id.
    let audited = harness.audited();
    if audited.len() != 1 || audited.first().map(String::as_str) != Some(envelope.signal_id()) {
        return Err(String::from("audit must record the emit"));
    }
    Ok(())
}

/// ML-only and privileged capabilities fail registration with elevation.
/// `intent.emit` never reaches the broker: construction strips it first, so
/// the elevation gate covers the ML-only set here and the strip is asserted
/// separately.
#[test]
fn privileged_capabilities_fail_registration() -> Result<(), String> {
    let harness = Harness::valid();
    for capability in [Capability::TapeRead, Capability::ViewInject] {
        let refused = harness.broker().register(
            widget("widget.priv", vec![capability.clone()], false),
            session("tenant", SESSION_ID)?,
        );
        if refused != Err(BrokerError::CapabilityElevation(capability)) {
            return Err(String::from("privileged cap must elevate"));
        }
    }
    // Stripped at construction: registering with only `intent.emit` leaves
    // no elevation to report, and the grant set stays empty.
    let stripped = widget("widget.strip", vec![Capability::IntentEmit], false);
    if stripped.capabilities().contains(&Capability::IntentEmit) {
        return Err(String::from("intent.emit must be stripped"));
    }
    harness
        .broker()
        .register(stripped, session("tenant", SESSION_ID)?)
        .map_err(|error| format!("stripped widget must register: {error}"))?;
    // Pinning is orthogonal to registration: a pinned widget registers and
    // keeps its flag.
    let mut pinned = widget("widget.pinned", vec![Capability::SignalEmit], false);
    pinned.set_pinned(true);
    if !pinned.pinned() {
        return Err(String::from("pin flag must set"));
    }
    harness
        .broker()
        .register(pinned, session("tenant", SESSION_ID)?)
        .map_err(|error| format!("pinned widget must register: {error}"))?;
    Ok(())
}

/// Duplicate widget ids fail, and unregister frees the id.
#[test]
fn duplicate_source_ids_fail_until_unregistered() -> Result<(), String> {
    let harness = Harness::valid();
    let handle = register(&harness, "widget.dup", "tenant", SESSION_ID)?;
    let refused = harness.broker().register(
        widget("widget.dup", vec![Capability::SignalEmit], false),
        session("tenant", SESSION_ID)?,
    );
    if refused != Err(BrokerError::DuplicateSourceId(String::from("widget.dup"))) {
        return Err(String::from("duplicate id must fail"));
    }
    harness.broker().unregister(&handle);
    register(&harness, "widget.dup", "tenant", SESSION_ID)?;
    Ok(())
}

/// Unknown handles and malformed kinds fail closed.
#[test]
fn bad_handles_and_kinds_fail() -> Result<(), String> {
    let harness = Harness::valid();
    let forged = RegisteredHandle::new(
        Uuid::from(999_u128.to_le_bytes()),
        String::from("session"),
        String::from("widget.ghost"),
        String::from("tenant"),
        String::from("user"),
    );
    if harness.broker().emit(&forged, "kind", JsonValue::Null) != Err(BrokerError::InvalidHandle) {
        return Err(String::from("forged handle must be invalid"));
    }
    if harness
        .broker()
        .subscribe(
            &forged,
            SignalPattern::new(String::from("**"), None),
            collector(&Arc::new(Mutex::new(Vec::new()))),
        )
        .is_ok()
    {
        return Err(String::from("forged subscribe must be refused"));
    }
    let handle = register(&harness, "widget.k", "tenant", SESSION_ID)?;
    if harness.broker().emit(&handle, "", JsonValue::Null) != Err(BrokerError::MalformedKind) {
        return Err(String::from("empty kind must be malformed"));
    }
    Ok(())
}

/// Self-delivery follows self_listen; cross-widget delivery always works.
#[test]
fn self_listen_gates_self_delivery() -> Result<(), String> {
    let harness = Harness::valid();
    let quiet = register(&harness, "widget.quiet", "tenant", SESSION_ID)?;
    let loud = harness
        .broker()
        .register(
            widget(
                "widget.loud",
                vec![Capability::SignalEmit, Capability::SignalSubscribe],
                true,
            ),
            session("tenant", SESSION_ID)?,
        )
        .map_err(|error| format!("{error}"))?;
    let quiet_got = subscribe_all(&harness, &quiet)?;
    let loud_got = subscribe_all(&harness, &loud)?;
    harness
        .broker()
        .emit(&quiet, "ping", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    if !lock_vec(&quiet_got).is_empty() {
        return Err(String::from("quiet widget must not hear itself"));
    }
    if lock_vec(&loud_got).len() != 1 {
        return Err(String::from("loud widget must hear others"));
    }
    harness
        .broker()
        .emit(&loud, "pong", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    if lock_vec(&loud_got).len() != 2 {
        return Err(String::from("self_listen must hear itself"));
    }
    Ok(())
}

/// Sessions isolate: same kind in another session never delivers.
#[test]
fn sessions_isolate_delivery() -> Result<(), String> {
    let harness = Harness::valid();
    let first = register(&harness, "widget.one", "tenant", SESSION_ONE)?;
    let second = register(&harness, "widget.two", "tenant", SESSION_TWO)?;
    let got = subscribe_all(&harness, &first)?;
    harness
        .broker()
        .emit(&second, "ping", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    if !lock_vec(&got).is_empty() {
        return Err(String::from("cross-session delivery must not happen"));
    }
    // The inspector lists the caller's own session only, labeled by scope.
    let listed = own_subscriptions(&harness, &first)?;
    if listed.len() != 1 {
        return Err(String::from("inspector must list one subscription"));
    }
    let Some(only) = listed.first() else {
        return Err(String::from("inspector must list one subscription"));
    };
    if only.session_id() != SESSION_ONE || only.tenant_id() != "tenant" {
        return Err(String::from("inspector must label session scope"));
    }
    Ok(())
}

/// Tenants isolate even when the session id matches (tenant-scoped
/// subscriptions).
#[test]
fn tenants_isolate_matching_session_ids() -> Result<(), String> {
    let harness = Harness::valid();
    let alpha = register_listen(&harness, "widget.alpha", "tenant-alpha", SESSION_SHARED)?;
    let beta = register_listen(&harness, "widget.beta", "tenant-beta", SESSION_SHARED)?;
    let got = subscribe_all(&harness, &alpha)?;
    harness
        .broker()
        .emit(&beta, "ping", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    if !lock_vec(&got).is_empty() {
        return Err(String::from("cross-tenant delivery must not happen"));
    }
    harness
        .broker()
        .emit(&alpha, "ping", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    if lock_vec(&got).len() != 1 {
        return Err(String::from("same-tenant delivery must work"));
    }
    Ok(())
}

/// Frozen clock: 250 emits allow 200 and limit 50.
#[test]
fn token_bucket_limits_frozen_bursts() -> Result<(), String> {
    let harness = Harness::valid();
    let handle = register(&harness, "widget.burst", "tenant", SESSION_ID)?;
    let mut limited = 0_usize;
    for _ in 0..250 {
        if harness.broker().emit(&handle, "tick", JsonValue::Null) == Err(BrokerError::RateLimited)
        {
            limited = limited.saturating_add(1);
        }
    }
    if limited != 50 {
        return Err(format!("250 frozen emits must limit 50, limited {limited}"));
    }
    // One second refills 100.
    harness.time().set(1_000);
    let mut refilled = 0_usize;
    for _ in 0..100 {
        if harness
            .broker()
            .emit(&handle, "tick", JsonValue::Null)
            .is_ok()
        {
            refilled = refilled.saturating_add(1);
        }
    }
    if refilled != 100 {
        return Err(format!("one second must refill 100, refilled {refilled}"));
    }
    Ok(())
}

/// A 500-burst limits exactly 300.
#[test]
fn large_burst_limits_remainder() -> Result<(), String> {
    let harness = Harness::valid();
    let handle = register(&harness, "widget.flood", "tenant", SESSION_ID)?;
    let mut limited = 0_usize;
    for _ in 0..500 {
        if harness.broker().emit(&handle, "tick", JsonValue::Null) == Err(BrokerError::RateLimited)
        {
            limited = limited.saturating_add(1);
        }
    }
    if limited != 300 {
        return Err(format!(
            "500 frozen emits must limit 300, limited {limited}"
        ));
    }
    Ok(())
}

/// Unregister retires the handle; re-registration gets a fresh bucket.
#[test]
fn unregister_retires_and_refreshes() -> Result<(), String> {
    let harness = Harness::valid();
    let handle = register(&harness, "widget.ephemeral", "tenant", SESSION_ID)?;
    harness.broker().unregister(&handle);
    if harness.broker().emit(&handle, "tick", JsonValue::Null) != Err(BrokerError::InvalidHandle) {
        return Err(String::from("retired handle must be invalid"));
    }
    if harness
        .broker()
        .subscribe(
            &handle,
            SignalPattern::new(String::from("**"), None),
            collector(&Arc::new(Mutex::new(Vec::new()))),
        )
        .is_ok()
    {
        return Err(String::from("retired subscribe must be refused"));
    }
    // Exhaust, re-register, burst again.
    let handle = register(&harness, "widget.cycle", "tenant", SESSION_ID)?;
    for _ in 0..200 {
        harness
            .broker()
            .emit(&handle, "tick", JsonValue::Null)
            .map_err(|error| format!("{error}"))?;
    }
    harness.broker().unregister(&handle);
    let handle = register(&harness, "widget.cycle", "tenant", SESSION_ID)?;
    harness
        .broker()
        .emit(&handle, "tick", JsonValue::Null)
        .map_err(|error| format!("fresh bucket must allow: {error}"))?;
    Ok(())
}

/// Clone a mutex-guarded log, recovering from poisoning like the listeners.
fn snapshot_log<T: Clone>(slot: &Arc<Mutex<Vec<T>>>) -> Vec<T> {
    match slot.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// Taps observe every emit with match counts and redacted payloads.
#[test]
fn taps_observe_with_counts_and_redaction() -> Result<(), String> {
    let harness = Harness::valid();
    let first = register_listen(&harness, "widget.tap-one", "tenant", SESSION_ID)?;
    let second = register_listen(&harness, "widget.tap-two", "tenant", SESSION_ID)?;
    subscribe_all(&harness, &first)?;
    subscribe_all(&harness, &second)?;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_listener: TapListener = {
        let seen = Arc::clone(&seen);
        Arc::new(
            move |event: drishti::native_host::broker::TapEvent| match seen.lock() {
                Ok(mut guard) => guard.push(event),
                Err(poisoned) => poisoned.into_inner().push(event),
            },
        )
    };
    let tap = tap_own(&harness, &first, seen_listener)?;
    harness
        .broker()
        .emit(&first, "ping", JsonValue::String(String::from("secret")))
        .map_err(|error| format!("{error}"))?;
    let events = snapshot_log(&seen);
    if events.len() != 1 {
        return Err(String::from("tap must observe the emit"));
    }
    let Some(event) = events.first() else {
        return Err(String::from("tap must observe the emit"));
    };
    if event.matched_subscriber_count() != 2 {
        return Err(String::from("tap must count both subscribers"));
    }
    if event.envelope().payload() != &JsonValue::Null {
        return Err(String::from("tap payload must be redacted"));
    }
    if event.rate_limited_count() != 0 {
        return Err(String::from("no throttle must report zero"));
    }
    untap_ok(&harness, &first, &tap)?;
    harness
        .broker()
        .emit(&first, "ping", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    let events = snapshot_log(&seen);
    if events.len() != 1 {
        return Err(String::from("untapped listener must stay silent"));
    }
    Ok(())
}

/// Session-filtered taps hear only their session.
#[test]
fn taps_filter_by_session() -> Result<(), String> {
    let harness = Harness::valid();
    let first = register(&harness, "widget.s1", "tenant", SESSION_ONE)?;
    let second = register(&harness, "widget.s2", "tenant", SESSION_TWO)?;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let listener: TapListener = {
        let seen = Arc::clone(&seen);
        Arc::new(
            move |event: drishti::native_host::broker::TapEvent| match seen.lock() {
                Ok(mut guard) => {
                    guard.push(event.envelope().session_invariant().session_id().to_owned())
                }
                Err(poisoned) => poisoned
                    .into_inner()
                    .push(event.envelope().session_invariant().session_id().to_owned()),
            },
        )
    };
    tap_own(&harness, &first, listener)?;
    harness
        .broker()
        .emit(&first, "ping", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    harness
        .broker()
        .emit(&second, "ping", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    let seen = snapshot_log(&seen);
    if seen != [String::from(SESSION_ONE)] {
        return Err(String::from("session tap must filter"));
    }
    Ok(())
}

/// ML peers register on verified grants and stamp ML origin.
#[test]
fn ml_peers_stamp_ml_origin() -> Result<(), String> {
    let harness = Harness::valid();
    let grant = SignedMlGrant::new(
        Uuid::from(7_u128.to_le_bytes()),
        String::from("tenant"),
        vec![Capability::TapeRead],
        9_999_999,
        0,
        vec![9_u8; 64],
    );
    let peer = harness
        .broker()
        .register_ml_peer(grant, session("tenant", SESSION_ID)?)
        .map_err(|error| format!("{error}"))?;
    let envelope = harness
        .broker()
        .emit_prediction(&peer, "ml.suggest", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    if envelope.source_kind() != SourceKind::Ml || envelope.source_id() != "tenant" {
        return Err(String::from("prediction must stamp ML origin"));
    }
    // Forged peers fail; unverified grants fail; cap-less grants fail.
    let forged = MlHandle::new(Uuid::from(8_u128.to_le_bytes()), String::from("tenant"));
    if harness
        .broker()
        .emit_prediction(&forged, "ml.suggest", JsonValue::Null)
        != Err(BrokerError::InvalidHandle)
    {
        return Err(String::from("forged peer must be invalid"));
    }
    let denied = Harness::with_verdict(GrantVerificationResult::InvalidSignature);
    let grant = SignedMlGrant::new(
        Uuid::from(9_u128.to_le_bytes()),
        String::from("tenant"),
        vec![Capability::TapeRead],
        9_999_999,
        0,
        Vec::new(),
    );
    if denied
        .broker()
        .register_ml_peer(grant, session("tenant", SESSION_ID)?)
        != Err(BrokerError::MlGrantInvalid(
            GrantVerificationResult::InvalidSignature,
        ))
    {
        return Err(String::from("bad signature must fail"));
    }
    let capless = SignedMlGrant::new(
        Uuid::from(10_u128.to_le_bytes()),
        String::from("tenant"),
        vec![Capability::SignalEmit],
        9_999_999,
        0,
        Vec::new(),
    );
    if harness
        .broker()
        .register_ml_peer(capless, session("tenant", SESSION_ID)?)
        != Err(BrokerError::MlGrantInvalid(
            GrantVerificationResult::InvalidGrant,
        ))
    {
        return Err(String::from("cap-less grant must fail"));
    }
    harness.broker().unregister_ml_peer(&peer);
    if harness
        .broker()
        .emit_prediction(&peer, "ml.suggest", JsonValue::Null)
        != Err(BrokerError::InvalidHandle)
    {
        return Err(String::from("retired peer must be invalid"));
    }
    Ok(())
}

/// A hundred subscribers all receive within the latency bound.
#[test]
fn hundred_subscriber_fanout_completes() -> Result<(), String> {
    let harness = Harness::valid();
    let emitter = register(&harness, "widget.fan", "tenant", SESSION_ID)?;
    let mut slots = Vec::new();
    for index in 0..100 {
        let handle = register(
            &harness,
            &format!("widget.fan-{index}"),
            "tenant",
            SESSION_ID,
        )?;
        slots.push(subscribe_all(&harness, &handle)?);
    }
    let started = std::time::Instant::now();
    harness
        .broker()
        .emit(&emitter, "ping", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    if started.elapsed().as_secs() >= 1 {
        return Err(String::from(
            "100-subscriber fanout must finish in a second",
        ));
    }
    for slot in &slots {
        if lock_vec(slot).len() != 1 {
            return Err(String::from("every subscriber must receive"));
        }
    }
    Ok(())
}

/// Seeded fuzz: 500 emits across 10 widgets only ever succeed or limit.
#[test]
fn seeded_fuzz_emits_never_break_contract() -> Result<(), String> {
    let harness = Harness::valid();
    let mut handles = Vec::new();
    for index in 0..10 {
        handles.push(register(
            &harness,
            &format!("widget.fuzz-{index}"),
            "tenant",
            SESSION_ID,
        )?);
    }
    let mut state: u64 = 0x1234_5678_9abc_def0;
    let mut step = || {
        state ^= state.wrapping_shl(13);
        state ^= state.wrapping_shr(7);
        state ^= state.wrapping_shl(17);
        state
    };
    let mut succeeded = 0_usize;
    let mut limited = 0_usize;
    for _ in 0..500 {
        let widget_index = usize::try_from(step().wrapping_rem(10))
            .map_err(|error| format!("index overflow: {error}"))?;
        let kind = match step().wrapping_rem(6) {
            0 => String::from(""),
            1 => String::from(".leading"),
            2 => String::from("trailing."),
            3 => String::from("a..b"),
            4 => String::from("with space"),
            _ => format!("fuzz.kind-{}", step().wrapping_rem(100)),
        };
        let handle = handles
            .get(widget_index)
            .ok_or_else(|| String::from("fuzz widget index out of range"))?;
        match harness.broker().emit(handle, &kind, JsonValue::Number(1.0)) {
            Ok(_) => succeeded = succeeded.saturating_add(1),
            Err(BrokerError::RateLimited | BrokerError::MalformedKind) => {
                limited = limited.saturating_add(1);
            }
            Err(error) => return Err(format!("unexpected fuzz outcome: {error}")),
        }
    }
    if succeeded.saturating_add(limited) != 500 {
        return Err(String::from("every fuzz emit must resolve"));
    }
    Ok(())
}

/// Sequential register/emit/unregister across 50 widgets resolves exactly.
///
/// The kernel fuzzed this concurrently; this tree bans `std::thread::spawn`
/// (unjoined threads hide panics), so the same interleaving runs
/// deterministically in one thread: register all, emit on all, retire all,
/// then prove every handle is invalid and no subscription leaks.
#[test]
fn sequential_widgets_resolve_exactly() -> Result<(), String> {
    let harness = Harness::valid();
    let mut handles = Vec::new();
    for index in 0..50 {
        handles.push(register(
            &harness,
            &format!("widget.race-{index}"),
            "tenant",
            SESSION_ID,
        )?);
    }
    for handle in &handles {
        for _ in 0..4 {
            harness
                .broker()
                .emit(handle, "tick", JsonValue::Null)
                .map_err(|error| format!("{error}"))?;
        }
    }
    for handle in &handles {
        harness.broker().unregister(handle);
    }
    for handle in &handles {
        if harness.broker().emit(handle, "tick", JsonValue::Null) != Err(BrokerError::InvalidHandle)
        {
            return Err(String::from("retired handle must be invalid"));
        }
    }
    let survivor = register(&harness, "widget.survivor", "tenant", SESSION_ID)?;
    if !own_subscriptions(&harness, &survivor)?.is_empty() {
        return Err(String::from("no subscription must leak"));
    }
    Ok(())
}
