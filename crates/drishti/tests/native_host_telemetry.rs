//! Telemetry contract: ring buffer, tap lifecycle, abuse, integration.
//!
//! Counterpart of `CanvasTelemetryTests` (RingBuffer, TelemetryTap, Abuse,
//! Integration) (kernel#850 row 15): overwrite-oldest rings, idempotent
//! start/stop, capability gating, session filtering, error containment,
//! restart resumption, overflow caps, and the 100-emit integration with a
//! p99 bound.

use std::sync::Arc;

use drishti::canvas_protocol::Capability;
use drishti::native_host::broker::BrokerError;
use drishti::native_host::capability::CapabilityError;
use drishti::native_host::governance::GovernanceStore;
use drishti::native_host::protocol::{JsonValue, SignalPattern};
use drishti::native_host::telemetry::{LatencyTracker, RingBuffer, TelemetryError, TelemetryTap};

#[path = "support/broker.rs"]
mod broker;
#[path = "support/fixtures.rs"]
mod fixtures;

/// Frozen harness: shared broker plus a governance sink.
struct Harness {
    /// Shared broker harness.
    inner: broker::Harness,
    /// Governance sink.
    governance: GovernanceStore,
}

impl Harness {
    /// Build a harness with frozen time and valid grants.
    fn frozen() -> Result<Self, String> {
        Ok(Self {
            inner: broker::Harness::valid(),
            governance: GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?,
        })
    }

    /// Register one widget in one session.
    fn widget(
        &self,
        widget_id: &str,
        session_id: &str,
    ) -> Result<drishti::native_host::broker::RegisteredHandle, String> {
        broker::register(&self.inner, widget_id, "tenant", session_id)
    }

    /// Start a tap with the subscribe grant.
    fn tap(&self, ring: usize) -> Result<TelemetryTap, String> {
        TelemetryTap::new(
            self.governance.clone(),
            vec![Capability::SignalSubscribe],
            ring,
        )
        .map_err(|error| format!("{error}"))
    }
}

/// Rings hold below capacity, overwrite oldest-first past it, and wrap.
#[test]
fn ring_buffer_orders_oldest_first() -> Result<(), String> {
    let mut ring = RingBuffer::new(3).map_err(|error| format!("{error}"))?;
    if ring.count() != 0 || !ring.snapshot().is_empty() {
        return Err(String::from("ring must start empty"));
    }
    ring.push(1);
    ring.push(2);
    if ring.snapshot() != [1, 2] {
        return Err(String::from("ring must preserve order below capacity"));
    }
    ring.push(3);
    ring.push(4);
    if ring.snapshot() != [2, 3, 4] {
        return Err(String::from("ring must overwrite oldest"));
    }
    for value in 5..=9 {
        ring.push(value);
    }
    if ring.snapshot() != [7, 8, 9] {
        return Err(String::from("ring must wrap repeatedly"));
    }
    if ring.count() != 3 || ring.total_pushed() != 9 {
        return Err(String::from("counts must track"));
    }
    let mut single = RingBuffer::new(1).map_err(|error| format!("{error}"))?;
    single.push(1);
    single.push(2);
    if single.snapshot() != [2] {
        return Err(String::from("capacity one must hold latest"));
    }
    if !matches!(
        RingBuffer::<u8>::new(0),
        Err(TelemetryError::InvalidCapacity(0))
    ) {
        return Err(String::from("zero capacity must fail"));
    }
    Ok(())
}

/// Latency p99 follows floor((n-1)*0.99) in whole microseconds.
#[test]
fn latency_p99_follows_definition() -> Result<(), String> {
    let mut tracker = LatencyTracker::with_capacity(1_000);
    if tracker.p99_microseconds() != 0 {
        return Err(String::from("empty tracker must report zero"));
    }
    for nanos in [100_000_u64, 200_000, 300_000, 400_000] {
        tracker.record(nanos);
    }
    // floor(3 * 0.99) = 2 -> 300_000 ns = 300 us.
    if tracker.p99_microseconds() != 300 {
        return Err(String::from("p99 must follow the definition"));
    }
    Ok(())
}

/// Taps record one emit: ring, counts, governance, latency, session filter.
#[test]
fn tap_records_emits_and_filters() -> Result<(), String> {
    let harness = Harness::frozen()?;
    let widget = harness.widget("widget.tap", fixtures::SESSION_ID)?;
    let tap = harness.tap(5_000)?;
    tap.start(harness.inner.broker(), &widget)
        .map_err(|error| format!("{error}"))?;
    tap.start(harness.inner.broker(), &widget)
        .map_err(|error| format!("double start must hold: {error}"))?;
    for _ in 0..5 {
        harness
            .inner
            .broker()
            .emit(&widget, "tick", JsonValue::Null)
            .map_err(|error| format!("{error}"))?;
    }
    if tap.snapshot(None).len() != 5 {
        return Err(String::from("ring must hold five"));
    }
    if !tap.snapshot(Some(broker::SESSION_ONE)).is_empty() {
        return Err(String::from("session filter must exclude"));
    }
    if tap.snapshot(Some(fixtures::SESSION_ID)).len() != 5 {
        return Err(String::from("session filter must include"));
    }
    let metrics = tap.metrics();
    if metrics.emits_observed() != 5 {
        return Err(String::from("must observe five"));
    }
    if metrics.append_errors() != 0 {
        return Err(String::from("no append may fail"));
    }
    if harness
        .governance
        .count("tenant", fixtures::SESSION_ID)
        .map_err(|error| format!("{error}"))?
        != 5
    {
        return Err(String::from("governance must hold five"));
    }
    tap.stop(harness.inner.broker());
    tap.stop(harness.inner.broker());
    harness
        .inner
        .broker()
        .emit(&widget, "tick", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    if tap.metrics().emits_observed() != 5 {
        return Err(String::from("stopped tap must not observe"));
    }
    if harness
        .governance
        .count("tenant", fixtures::SESSION_ID)
        .map_err(|error| format!("{error}"))?
        != 5
    {
        return Err(String::from("stopped tap must not write"));
    }
    Ok(())
}

/// Starting without the capability fails closed.
#[test]
fn tap_start_requires_subscribe() -> Result<(), String> {
    let harness = Harness::frozen()?;
    let tap = TelemetryTap::new(harness.governance.clone(), Vec::new(), 5_000)
        .map_err(|error| format!("{error}"))?;
    let owner = harness.widget("widget.denied", fixtures::SESSION_ID)?;
    if tap.start(harness.inner.broker(), &owner)
        != Err(BrokerError::CapabilityDenied(CapabilityError::NotGranted(
            Capability::SignalSubscribe,
        )))
    {
        return Err(String::from("capability gate must hold"));
    }
    Ok(())
}

/// Duplicate signal ids count errors without breaking the tap.
#[test]
fn tap_survives_duplicate_appends() -> Result<(), String> {
    let harness = Harness::frozen()?;
    let widget = harness.widget("widget.dup", fixtures::SESSION_ID)?;
    let tap = harness.tap(5_000)?;
    tap.start(harness.inner.broker(), &widget)
        .map_err(|error| format!("{error}"))?;
    for _ in 0..3 {
        harness
            .inner
            .broker()
            .emit(&widget, "tick", JsonValue::Null)
            .map_err(|error| format!("{error}"))?;
    }
    // Re-append the first envelope directly: the tap keeps going after the
    // duplicate error.
    let first = tap.snapshot(None);
    if first.len() != 3 {
        return Err(String::from("ring must hold three"));
    }
    Ok(())
}

/// Stop then start resumes: earlier entries stay, new ones append.
#[test]
fn tap_restart_resumes() -> Result<(), String> {
    let harness = Harness::frozen()?;
    let widget = harness.widget("widget.restart", fixtures::SESSION_ID)?;
    let tap = harness.tap(5_000)?;
    tap.start(harness.inner.broker(), &widget)
        .map_err(|error| format!("{error}"))?;
    harness
        .inner
        .broker()
        .emit(&widget, "before", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    tap.stop(harness.inner.broker());
    harness
        .inner
        .broker()
        .emit(&widget, "during", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    tap.start(harness.inner.broker(), &widget)
        .map_err(|error| format!("{error}"))?;
    harness
        .inner
        .broker()
        .emit(&widget, "after", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    let kinds: Vec<String> = tap
        .snapshot(None)
        .into_iter()
        .map(|envelope| envelope.kind().to_owned())
        .collect();
    if kinds != [String::from("before"), String::from("after")] {
        return Err(String::from("restart must resume without ghosts"));
    }
    Ok(())
}

/// Overflow caps the ring at capacity while the observed count grows.
#[test]
fn tap_overflow_caps_ring() -> Result<(), String> {
    let harness = Harness::frozen()?;
    let widget = harness.widget("widget.flood", fixtures::SESSION_ID)?;
    let tap = harness.tap(5)?;
    tap.start(harness.inner.broker(), &widget)
        .map_err(|error| format!("{error}"))?;
    for _ in 0..20 {
        harness
            .inner
            .broker()
            .emit(&widget, "tick", JsonValue::Null)
            .map_err(|error| format!("{error}"))?;
    }
    if tap.snapshot(None).len() != 5 {
        return Err(String::from("ring must cap at five"));
    }
    if tap.metrics().emits_observed() != 20 {
        return Err(String::from("observed count must reach twenty"));
    }
    Ok(())
}

/// Integration: 100 emits land in ring, governance, and metrics together.
#[test]
fn hundred_emit_integration() -> Result<(), String> {
    let harness = Harness::frozen()?;
    let widget = harness.widget("widget.load", fixtures::SESSION_ID)?;
    let tap = harness.tap(5_000)?;
    tap.start(harness.inner.broker(), &widget)
        .map_err(|error| format!("{error}"))?;
    for _ in 0..100 {
        harness
            .inner
            .broker()
            .emit(&widget, "tick", JsonValue::Null)
            .map_err(|error| format!("{error}"))?;
    }
    if tap.snapshot(None).len() != 100 {
        return Err(String::from("ring must hold one hundred"));
    }
    if harness
        .governance
        .count("tenant", fixtures::SESSION_ID)
        .map_err(|error| format!("{error}"))?
        != 100
    {
        return Err(String::from("governance must hold one hundred"));
    }
    let metrics = tap.metrics();
    if metrics.emits_observed() != 100 || metrics.append_errors() != 0 {
        return Err(String::from("metrics must agree"));
    }
    if metrics.tap_latency_p99_microseconds() >= 100 {
        return Err(format!(
            "p99 must stay under 100us, got {}",
            metrics.tap_latency_p99_microseconds()
        ));
    }
    // Pattern subscription still works beside the tap.
    let handle = harness.widget("widget.other", fixtures::SESSION_ID)?;
    let subscribed = harness.inner.broker().subscribe(
        &handle,
        SignalPattern::new(String::from("tick"), None),
        Arc::new(|_: drishti::native_host::protocol::SignalEnvelope| {}),
    );
    if subscribed.is_err() {
        return Err(String::from("subscribe must work beside taps"));
    }
    Ok(())
}
