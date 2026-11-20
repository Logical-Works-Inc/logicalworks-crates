//! Shared fixtures for the host suites: identities, sessions, entries.
//!
//! One home for the UUID constants and small builders every host suite
//! repeats, so the suites cannot drift from each other. Suites include this
//! file with `#[path]`; it is never a test target itself.

use drishti::canvas_protocol::{Capability, SessionInvariant};
use drishti::motion::{
    Easing, MotionGraph, MotionGraphBuilder, NamedCurve, NodeKind, TweenComponent,
};
use drishti::native_host::protocol::{
    Attribution, AttributionKind, GovernanceEntry, WidgetDescriptor, WidgetSurface,
};
use lgwks_std::id::Uuid;

/// Fixed work-object identity: the scope check binds artifact keys to it.
pub(crate) const WORK_OBJECT: &str = "11111111-1111-1111-1111-111111111111";
/// Fixed session identity fixture.
pub(crate) const SESSION_ID: &str = "22222222-2222-2222-2222-222222222222";
/// Fixed policy bundle fixture.
pub(crate) const POLICY_BUNDLE: &str = "33333333-3333-3333-3333-333333333333";
/// Fixed user subject fixture.
pub(crate) const USER_SUB: &str = "44444444-4444-4444-4444-444444444444";

/// Session fixture for one tenant in the fixed session.
pub(crate) fn session(tenant: &str) -> Result<SessionInvariant, String> {
    session_in(tenant, SESSION_ID)
}

/// Session fixture for one tenant in one session.
pub(crate) fn session_in(tenant: &str, session_id: &str) -> Result<SessionInvariant, String> {
    SessionInvariant::new(
        String::from(WORK_OBJECT),
        String::from(session_id),
        String::from(POLICY_BUNDLE),
        String::from(tenant),
        String::from(USER_SUB),
    )
    .map_err(|error| format!("fixture session rejected: {error}"))
}

/// Fixed tenant of the single-tenant governance fixtures.
pub(crate) const TENANT: &str = "tenant-fixture";

/// Governance entry fixture in the fixed tenant.
pub(crate) fn governance_entry(
    signal_id: &str,
    session_id: &str,
    kind: &str,
    recorded_at: &str,
) -> GovernanceEntry {
    governance_entry_in(TENANT, signal_id, session_id, kind, recorded_at)
}

/// Governance entry fixture for one tenant: fixed work object, widget
/// attribution, and description. The store keys on the signal id, so the
/// entry id only needs to be stable for one signal.
pub(crate) fn governance_entry_in(
    tenant: &str,
    signal_id: &str,
    session_id: &str,
    kind: &str,
    recorded_at: &str,
) -> GovernanceEntry {
    let mut entry = GovernanceEntry::new(
        String::from(session_id),
        format!("entry-{signal_id}"),
        String::from(WORK_OBJECT),
        String::from(signal_id),
        String::from(kind),
        String::from("telemetry tap capture"),
        Attribution::new(
            AttributionKind::Widget,
            String::from("widget.demo"),
            None,
            None,
        ),
    );
    entry.set_recorded_at(String::from(recorded_at));
    entry.set_tenant_id(String::from(tenant));
    entry
}

/// Widget descriptor fixture with empty signal patterns.
pub(crate) fn widget_descriptor(
    widget_id: &str,
    kind: &str,
    capabilities: Vec<Capability>,
) -> WidgetDescriptor {
    WidgetDescriptor::new(
        String::from(kind),
        String::from(widget_id),
        String::from("1.0.0"),
        WidgetSurface::Native,
        Vec::new(),
        Vec::new(),
        capabilities,
    )
}

/// All 25 non-instant named easing curves in declaration order, shared by the
/// easing-correctness and float-tolerance suites so both enumerate the same set.
pub(crate) fn all_named_curves() -> Vec<NamedCurve> {
    vec![
        NamedCurve::Linear,
        NamedCurve::QuadIn,
        NamedCurve::QuadOut,
        NamedCurve::QuadInOut,
        NamedCurve::CubicIn,
        NamedCurve::CubicOut,
        NamedCurve::CubicInOut,
        NamedCurve::QuartIn,
        NamedCurve::QuartOut,
        NamedCurve::QuartInOut,
        NamedCurve::QuintIn,
        NamedCurve::QuintOut,
        NamedCurve::QuintInOut,
        NamedCurve::SineIn,
        NamedCurve::SineOut,
        NamedCurve::SineInOut,
        NamedCurve::ExpoIn,
        NamedCurve::ExpoOut,
        NamedCurve::ExpoInOut,
        NamedCurve::CircIn,
        NamedCurve::CircOut,
        NamedCurve::CircInOut,
        NamedCurve::BackIn,
        NamedCurve::BackOut,
        NamedCurve::BackInOut,
    ]
}

/// Endpoint tolerance shared by the easing-correctness and float-tolerance suites.
pub(crate) const EASING_TOLERANCE: f32 = 1e-5;

/// Turn a failed condition into the test's error. The message closure runs
/// only on failure, so a passing check formats nothing; every suite uses this
/// instead of a bare early `Err` return so each failure leaves one shape.
pub(crate) fn ensure(cond: bool, msg: impl FnOnce() -> String) -> Result<(), String> {
    cond.then_some(()).ok_or_else(msg)
}

/// Require bit-identical `f32` values, naming the probe on mismatch.
pub(crate) fn require_same_bits(actual: f32, expected: f32, what: &str) -> Result<(), String> {
    ensure(actual.to_bits() == expected.to_bits(), || {
        format!("{what}: got {actual}, want {expected}")
    })
}

/// Build a one-tween graph over a caller-chosen node id, handle, and duration.
pub(crate) fn tween_graph(
    node_id: Uuid,
    handle: &str,
    duration_ms: f32,
) -> Result<(MotionGraph, Uuid), String> {
    let tween = TweenComponent::new(
        0.0,
        1.0,
        0.0,
        duration_ms,
        Easing::named(NamedCurve::Linear),
    )
    .map_err(|error| format!("valid tween rejected: {error}"))?;
    let graph = MotionGraphBuilder::new()
        .add_node(
            node_id,
            String::from(handle),
            String::from("opacity"),
            NodeKind::Tween(tween),
        )
        .build();
    Ok((graph, node_id))
}

/// Fixture validity: every shared identity parses, both session constructors
/// agree on the fixed session, and the entry/descriptor fixtures carry the
/// identities they claim. This test lives with the helpers so every suite
/// that includes them also proves them.
#[test]
fn shared_fixtures_are_valid() -> Result<(), String> {
    for identity in [WORK_OBJECT, SESSION_ID, POLICY_BUNDLE, USER_SUB] {
        ensure(lgwks_std::id::Uuid::parse(identity).is_ok(), || {
            format!("fixture identity must parse: {identity}")
        })?;
    }
    ensure(
        session("tenant")? == session_in("tenant", SESSION_ID)?,
        || String::from("session constructors must agree"),
    )?;
    let entry = governance_entry(
        "signal",
        SESSION_ID,
        "forms.submitted",
        "2026-11-03T00:00:00Z",
    );
    ensure(
        entry.signal_id() == "signal"
            && entry.session_id() == SESSION_ID
            && entry.work_object_id() == WORK_OBJECT
            && entry.event_type() == "forms.submitted"
            && entry.recorded_at() == "2026-11-03T00:00:00Z"
            && entry.tenant_id() == TENANT
            && governance_entry_in("other", "signal", SESSION_ID, "k", "").tenant_id() == "other",
        || String::from("governance fixture must carry its identities"),
    )?;
    let descriptor = widget_descriptor("widget.fixture", "fixture.kind", Vec::new());
    ensure(
        descriptor.widget_id() == "widget.fixture"
            && descriptor.kind() == "fixture.kind"
            && descriptor.emits().is_empty()
            && descriptor.listens().is_empty(),
        || String::from("descriptor fixture must carry its identities"),
    )?;
    ensure(all_named_curves().len() == 25, || {
        String::from("named curve fixture set drifted")
    })?;
    require_same_bits(0.0, 0.0, "fixture bit comparison")?;
    ensure(EASING_TOLERANCE.to_bits() == 1e-5_f32.to_bits(), || {
        String::from("endpoint tolerance drifted")
    })?;
    let node = lgwks_std::id::Uuid::parse("99999999-9999-9999-9999-999999999999")
        .map_err(|error| format!("fixture node identity rejected: {error}"))?;
    let (graph, returned) = tween_graph(node, "fixture.tween", 100.0)?;
    ensure(returned == node && graph.node_count() == 1, || {
        String::from("tween graph fixture must bind one node")
    })
}

/// `ensure` passes a held condition without building its message, and turns a
/// failed one into exactly the message the closure builds.
#[test]
fn ensure_builds_its_message_only_on_failure() -> Result<(), String> {
    let built = std::cell::Cell::new(0_u32);
    let held = ensure(true, || {
        built.set(built.get().saturating_add(1));
        String::from("held")
    });
    ensure(held == Ok(()) && built.get() == 0, || {
        String::from("a held condition must not build its message")
    })?;
    let failed = ensure(false, || {
        built.set(built.get().saturating_add(1));
        String::from("failed")
    });
    ensure(
        failed == Err(String::from("failed")) && built.get() == 1,
        || String::from("a failed condition must build its message exactly once"),
    )
}
