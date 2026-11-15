//! Seeded telemetry pressure: high-rate seeded emits against small rings
//! with stop/restart churn; rings cap, counts grow, governance stays
//! consistent, and no ghost write survives a stop.
//!
//! Same seed replays the same trace hash.

use drishti::canvas_protocol::Capability;
use drishti::native_host::broker::RegisteredHandle;
use drishti::native_host::governance::GovernanceStore;
use drishti::native_host::protocol::{JsonValue, WidgetDescriptor, WidgetSurface};
use drishti::native_host::telemetry::TelemetryTap;

use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// Frozen harness with governance.
struct Harness {
    /// Shared broker harness.
    inner: crate::broker_shared::Harness,
    /// Governance sink.
    governance: GovernanceStore,
}

impl Harness {
    /// Build the harness.
    fn build() -> Result<Self, String> {
        Ok(Self {
            inner: crate::broker_shared::Harness::valid(),
            governance: GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?,
        })
    }
}

/// Fixed session fixture (valid UUID, enforced by the constructor).
const SESSION_A: &str = crate::fixtures::SESSION_ID;
/// Second session fixture for filter tests.
const SESSION_B: &str = crate::broker_shared::SESSION_ONE;

/// Pressure widget descriptor over the shared kind and capabilities.
fn pressure_descriptor(widget_id: String) -> WidgetDescriptor {
    WidgetDescriptor::new(
        String::from("pressure.kind"),
        widget_id,
        String::from("1.0.0"),
        WidgetSurface::Native,
        Vec::new(),
        Vec::new(),
        vec![Capability::SignalEmit, Capability::SignalSubscribe],
    )
}

/// Emit one seeded tick and hash the marker. Returns whether the tap owner
/// (always in `SESSION_A`) is in scope to hear it: the tap is scoped to its
/// owner's session, so ticks from the other session must never be recorded.
fn emit_tick(
    harness: &Harness,
    widgets: &[RegisteredHandle],
    random: &mut SimRng,
    marker: u64,
    trace: &mut TraceHash,
) -> Result<u64, String> {
    let position = random.below_len(widgets.len());
    let handle = widgets
        .get(position)
        .ok_or_else(|| String::from("widget index out of range"))?;
    harness
        .inner
        .broker()
        .emit(handle, "tick", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    trace.mix_u64(marker);
    Ok(u64::from(handle.session_id() == SESSION_A))
}

/// The tap owner is the first widget, registered in `SESSION_A`.
fn owner_of(widgets: &[RegisteredHandle]) -> Result<&RegisteredHandle, String> {
    widgets
        .first()
        .ok_or_else(|| String::from("a tap owner must exist"))
}

/// Run one pressure campaign from `seed` and hash the trace.
fn run_pressure(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::build()?;
    let session = |id: &str| crate::fixtures::session_in("tenant", id);
    let mut widgets = Vec::new();
    for index in 0..4_usize {
        let handle = harness
            .inner
            .broker()
            .register(
                pressure_descriptor(format!("widget.pressure-{index}")),
                session(if index.wrapping_rem(2) == 0 {
                    SESSION_A
                } else {
                    SESSION_B
                })?,
            )
            .map_err(|error| format!("{error}"))?;
        widgets.push(handle);
    }
    let tap = TelemetryTap::new(
        harness.governance.clone(),
        vec![Capability::SignalSubscribe],
        random.below_len(8).saturating_add(1),
    )
    .map_err(|error| format!("{error}"))?;
    tap.start(harness.inner.broker(), owner_of(&widgets)?)
        .map_err(|error| format!("{error}"))?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    for step in 0..300_usize {
        // Churn: advance time, stop, restart.
        if step == 100 {
            harness.inner.time().set(10_000);
            tap.stop(harness.inner.broker());
            trace.mix_str("stop");
        }
        if step == 150 {
            tap.start(harness.inner.broker(), owner_of(&widgets)?)
                .map_err(|error| format!("{error}"))?;
            trace.mix_str("restart");
        }
        let position = random.below_len(widgets.len());
        let handle = widgets
            .get(position)
            .ok_or_else(|| String::from("widget index out of range"))?;
        match harness.inner.broker().emit(handle, "tick", JsonValue::Null) {
            Ok(_) => trace.mix_u64(1),
            Err(error) => trace.mix_str(&format!("{error:?}")),
        }
        if step.wrapping_rem(50) == 0 {
            let metrics = tap.metrics();
            trace.mix_u64(metrics.emits_observed());
            trace.mix_u64(metrics.append_errors());
            trace.mix_len(tap.snapshot(None).len());
        }
    }
    trace.mix_u64(
        harness
            .governance
            .count("tenant", SESSION_A)
            .map_err(|error| format!("{error}"))?,
    );
    let foreign = harness
        .governance
        .count("tenant", SESSION_B)
        .map_err(|error| format!("{error}"))?;
    ensure(foreign == 0, || {
        format!("a tap must never record another session: {foreign}")
    })?;
    trace.mix_u64(foreign);
    Ok(trace.finish())
}

/// Same seed replays the same hash, churn included.
#[test]
fn sim_telemetry_pressure_replays_identically() -> Result<(), String> {
    for seed in [71_u64, 73, 909] {
        let (first, second) = (run_pressure(seed)?, run_pressure(seed)?);
        ensure(first == second, || format!("seed {seed} diverged"))?;
    }
    let (near, far) = (run_pressure(71)?, run_pressure(72)?);
    ensure(near != far, || String::from("distinct seeds must diverge"))
}

/// Register one widget per session and start a tap with `ring_capacity`.
fn setup_freeze(
    harness: &Harness,
    ring_capacity: usize,
) -> Result<(TelemetryTap, Vec<RegisteredHandle>), String> {
    let session = |id: &str| crate::fixtures::session_in("tenant", id);
    let mut widgets = Vec::new();
    for index in 0..2_usize {
        let session_id = if index == 0 { SESSION_A } else { SESSION_B };
        let handle = harness
            .inner
            .broker()
            .register(
                pressure_descriptor(format!("widget.freeze-{index}")),
                session(session_id)?,
            )
            .map_err(|error| format!("{error}"))?;
        widgets.push(handle);
    }
    let tap = TelemetryTap::new(
        harness.governance.clone(),
        vec![Capability::SignalSubscribe],
        ring_capacity,
    )
    .map_err(|error| format!("{error}"))?;
    tap.start(harness.inner.broker(), owner_of(&widgets)?)
        .map_err(|error| format!("{error}"))?;
    Ok((tap, widgets))
}

/// Count stored rows across both sessions.
fn stored_rows(harness: &Harness) -> Result<u64, String> {
    let first = harness
        .governance
        .count("tenant", SESSION_A)
        .map_err(|error| format!("{error}"))?;
    let second = harness
        .governance
        .count("tenant", SESSION_B)
        .map_err(|error| format!("{error}"))?;
    Ok(first.saturating_add(second))
}

/// Run one stop-freeze campaign from `seed`: emits before a stop are counted
/// and stored, a stopped tap writes nothing, and a restart resumes counting
/// without losing the earlier rows.
fn run_stop_freeze(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::build()?;
    let (tap, widgets) = setup_freeze(&harness, 8)?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut heard = 0_u64;
    for _ in 0..30 {
        heard = heard.saturating_add(emit_tick(&harness, &widgets, &mut random, 1, &mut trace)?);
    }
    let observed = tap.metrics().emits_observed();
    ensure(observed == heard, || {
        format!("tap must observe exactly its own session, observed {observed} of {heard}")
    })?;
    let stored = stored_rows(&harness)?;
    trace.mix_u64(stored);
    tap.stop(harness.inner.broker());
    trace.mix_str("stop");
    for _ in 0..30 {
        emit_tick(&harness, &widgets, &mut random, 2, &mut trace)?;
    }
    ensure(tap.metrics().emits_observed() == observed, || {
        String::from("stopped tap must observe nothing")
    })?;
    ensure(stored_rows(&harness)? == stored, || {
        String::from("stopped tap must write nothing")
    })?;
    tap.start(harness.inner.broker(), owner_of(&widgets)?)
        .map_err(|error| format!("{error}"))?;
    trace.mix_str("restart");
    let mut resumed = 0_u64;
    for _ in 0..10 {
        resumed =
            resumed.saturating_add(emit_tick(&harness, &widgets, &mut random, 3, &mut trace)?);
    }
    ensure(
        tap.metrics().emits_observed() == observed.saturating_add(resumed),
        || String::from("restart must resume counting"),
    )?;
    let grown = stored_rows(&harness)?;
    ensure(grown >= stored, || {
        String::from("restart must keep earlier rows")
    })?;
    trace.mix_u64(grown);
    Ok(trace.finish())
}

/// Same seed replays the same stop-freeze hash with identical counts.
#[test]
fn sim_telemetry_stop_freezes_writes() -> Result<(), String> {
    for seed in [83_u64, 89, 1_213] {
        let (first, second) = (run_stop_freeze(seed)?, run_stop_freeze(seed)?);
        ensure(first == second, || format!("seed {seed} diverged"))?;
    }
    let (near, far) = (run_stop_freeze(83)?, run_stop_freeze(84)?);
    ensure(near != far, || String::from("distinct seeds must diverge"))
}

/// Run one ring-cap campaign from `seed`: twenty frozen emits against a
/// three-slot ring observe every emit while the snapshot stays capped.
fn run_ring_caps(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::build()?;
    let (tap, widgets) = setup_freeze(&harness, 3)?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut heard = 0_u64;
    for _ in 0..20 {
        heard = heard.saturating_add(emit_tick(&harness, &widgets, &mut random, 1, &mut trace)?);
    }
    ensure(tap.metrics().emits_observed() == heard, || {
        String::from("tap must observe every emit in its own session")
    })?;
    trace.mix_u64(tap.metrics().emits_observed());
    let held = tap.snapshot(None).len();
    ensure(held <= 3, || format!("ring must cap at 3, held {held}"))?;
    trace.mix_len(held);
    Ok(trace.finish())
}

/// Same seed replays the same ring-cap hash.
#[test]
fn sim_telemetry_ring_caps_replay_identically() -> Result<(), String> {
    for seed in [91_u64, 97, 1_331] {
        let (first, second) = (run_ring_caps(seed)?, run_ring_caps(seed)?);
        ensure(first == second, || format!("seed {seed} diverged"))?;
    }
    let (near, far) = (run_ring_caps(91)?, run_ring_caps(92)?);
    ensure(near != far, || String::from("distinct seeds must diverge"))
}
