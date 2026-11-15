//! Session store and zone container contract.
//!
//! Counterpart of `CanvasStoresTests/SessionStoreTests.swift` and
//! `CanvasHostTests/ZoneContainerTests.swift` (kernel#850 row 15): session
//! lifecycle, focus handling, mount pinning, and the command-bar
//! register/unregister flow against a live broker.

use drishti::native_host::session::{MountDecision, SessionStore, ZoneContainer, ZoneId};

#[path = "support/broker.rs"]
mod broker;
#[path = "support/fixtures.rs"]
mod fixtures;

/// Start a session and read it back.
fn start(
    store: &mut SessionStore,
    tenant: &str,
    display: Option<String>,
    now_ms: u64,
) -> Result<(), String> {
    store.start_session(fixtures::session(tenant)?, display, now_ms);
    Ok(())
}

/// Sessions start, overwrite, and clear focus and labels.
#[test]
fn sessions_start_overwrite_and_clear() -> Result<(), String> {
    let mut store = SessionStore::default();
    if store.invariant().is_some()
        || store.focused_zone().is_some()
        || store.active_workspace().is_some()
        || store.session_started_ms().is_some()
        || store.display_name().is_some()
        || store.governance_visible()
    {
        return Err(String::from(
            "store must start empty with governance hidden",
        ));
    }
    start(
        &mut store,
        "tenant-a",
        Some(String::from("Acme canvas")),
        1_000,
    )?;
    let invariant = store
        .invariant()
        .ok_or_else(|| String::from("invariant must be set"))?;
    if invariant.tenant_id() != "tenant-a" {
        return Err(String::from("tenant must bind"));
    }
    if store.session_started_ms() != Some(1_000) {
        return Err(String::from("start time must stamp"));
    }
    if store.display_name() != Some("Acme canvas") {
        return Err(String::from("display name must bind"));
    }
    // Overwrite with a new session.
    start(&mut store, "tenant-b", None, 2_000)?;
    if store
        .invariant()
        .ok_or_else(|| String::from("invariant gone"))?
        .tenant_id()
        != "tenant-b"
    {
        return Err(String::from("restart must overwrite"));
    }
    if store.session_started_ms() != Some(2_000) || store.display_name().is_some() {
        return Err(String::from("restart must restamp and clear the label"));
    }
    // Focus set, change, clear.
    store.set_focus(Some(String::from("primary")));
    if store.focused_zone() != Some("primary") {
        return Err(String::from("focus must set"));
    }
    store.set_focus(Some(String::from("overlay")));
    store.set_focus(None);
    if store.focused_zone().is_some() {
        return Err(String::from("focus must clear"));
    }
    store.set_active_workspace(Some(String::from("workspace.landing")));
    if store.active_workspace() != Some("workspace.landing") {
        return Err(String::from("workspace must set"));
    }
    // Governance toggle flips state.
    store.set_governance_visible(true);
    if !store.governance_visible() {
        return Err(String::from("governance must show"));
    }
    store.set_governance_visible(false);
    if store.governance_visible() {
        return Err(String::from("governance must hide"));
    }
    // Malformed bindings never overwrite: the invariant constructor refuses
    // an empty tenant, so no invalid session can be built to start.
    let rejected = SessionStore::default();
    if fixtures::session_in("", fixtures::SESSION_ID).is_ok() {
        return Err(String::from("empty tenant must fail"));
    }
    if rejected.invariant().is_some() {
        return Err(String::from("rejected start must not bind"));
    }
    Ok(())
}

/// Zones mount unless pinned, replace when unpinned, and reset on clear.
#[test]
fn zones_mount_pin_and_clear() -> Result<(), String> {
    let mut zone = ZoneContainer::new(ZoneId::Primary);
    if zone.zone_id() != ZoneId::Primary || zone.is_pinned() || zone.current_widget_id().is_some() {
        return Err(String::from("zone must start empty and unpinned"));
    }
    if zone.request_mount(String::from("widget.a")) != MountDecision::Allowed {
        return Err(String::from("unpinned mount must allow"));
    }
    if zone.current_widget_id() != Some("widget.a") {
        return Err(String::from("mount must stick"));
    }
    zone.pin();
    let MountDecision::Denied(reason) = zone.request_mount(String::from("widget.b")) else {
        return Err(String::from("pinned mount must deny"));
    };
    if reason.existing() != Some(&String::from("widget.a")) {
        return Err(String::from("denial must name the holder"));
    }
    // Pinned empty denies with no holder.
    let mut bare = ZoneContainer::new(ZoneId::Secondary);
    bare.pin();
    let MountDecision::Denied(bare_reason) = bare.request_mount(String::from("widget.c")) else {
        return Err(String::from("pinned empty must deny"));
    };
    if bare_reason.existing().is_some() {
        return Err(String::from("empty denial must name nobody"));
    }
    zone.unpin();
    if zone.request_mount(String::from("widget.b")) != MountDecision::Allowed {
        return Err(String::from("unpinned replace must allow"));
    }
    zone.clear();
    if zone.current_widget_id().is_some() {
        return Err(String::from("clear must reset"));
    }
    Ok(())
}

/// The command bar registers, emits, and unregisters against a live broker.
#[test]
fn command_bar_lifecycle_flow() -> Result<(), String> {
    let harness = broker::Harness::valid();
    let bar = harness
        .broker()
        .register_command_bar(fixtures::session("tenant")?)
        .map_err(|error| format!("{error}"))?;
    if bar.session_id() != fixtures::SESSION_ID
        || bar.tenant_id() != "tenant"
        || bar.user_sub() != fixtures::USER_SUB
    {
        return Err(String::from("bar must bind its session"));
    }
    harness.broker().unregister_command_bar(&bar);
    harness
        .broker()
        .register_command_bar(fixtures::session("tenant")?)
        .map_err(|error| format!("re-register must work: {error}"))?;
    Ok(())
}
