//! Session and zone sims: random operation streams are run against a model
//! written from the documented rules, so a pinned zone that changes hands, or
//! a session switch that inherits the previous view, shows up as drift.

use drishti::canvas_protocol::SessionInvariant;
use drishti::native_host::session::{
    MountDecision, MountDenialReason, SessionStore, ZoneContainer, ZoneId,
};

use crate::fixtures::{SESSION_ID, session_in};
use crate::motion_kit::ensure;
use crate::rng::SimRng;

/// The reference zone: a pin flag and the mounted widget.
struct ZoneModel {
    /// Whether the zone refuses mounts.
    pinned: bool,
    /// Widget on the zone.
    current: Option<String>,
}

impl ZoneModel {
    /// The documented answer to a mount request, applied to the model.
    fn mount(&mut self, widget: &str) -> MountDecision {
        if self.pinned {
            return MountDecision::Denied(MountDenialReason::zone_pinned(self.current.clone()));
        }
        self.current = Some(String::from(widget));
        MountDecision::Allowed
    }
}

/// Bug caught: a pinned zone that lets a mount replace its widget, a denial
/// that names the wrong holder, or a clear that drops the pin, so a widget
/// the user pinned is displaced by another.
#[test]
fn sim_session_zone_container_follows_the_model() -> Result<(), String> {
    let zones = [
        ZoneId::Header,
        ZoneId::Primary,
        ZoneId::Secondary,
        ZoneId::Tertiary,
        ZoneId::Footer,
        ZoneId::Overlay,
    ];
    for seed in 0..120_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(220_000));
        let zone = zones[random.below_len(zones.len())];
        let mut real = ZoneContainer::new(zone);
        let mut model = ZoneModel {
            pinned: false,
            current: None,
        };
        let mut denials = 0_usize;
        for step in 0..200_usize {
            match random.below(5) {
                0 | 1 => {
                    let widget = format!("widget-{}", random.below(4));
                    let got = real.request_mount(widget.clone());
                    let want = model.mount(&widget);
                    if want != MountDecision::Allowed {
                        denials = denials.saturating_add(1);
                    }
                    ensure(
                        got == want,
                        &format!("seed {seed} step {step}: mount gave {got:?}, model {want:?}"),
                    )?;
                }
                2 => {
                    real.pin();
                    model.pinned = true;
                }
                3 => {
                    real.unpin();
                    model.pinned = false;
                }
                _ => {
                    real.clear();
                    model.current = None;
                }
            }
            ensure(
                real.zone_id() == zone
                    && real.is_pinned() == model.pinned
                    && real.current_widget_id() == model.current.as_deref(),
                &format!("seed {seed} step {step}: zone state drifted from the model"),
            )?;
        }
        ensure(
            denials > 0,
            &format!("seed {seed}: no mount was ever denied"),
        )?;
    }
    Ok(())
}

/// The reference store: only what the documentation says survives a switch.
struct StoreModel {
    /// Current session.
    invariant: Option<SessionInvariant>,
    /// Focused zone.
    focus: Option<String>,
    /// Active workspace.
    workspace: Option<String>,
    /// Governance toggle.
    governance: bool,
    /// Start stamp.
    started: Option<u64>,
    /// Display label.
    label: Option<String>,
}

/// Bug caught: a session switch that leaves the previous identity's focus,
/// workspace or governance toggle in place, so tenant B opens on tenant A's
/// workspace and with A's elevated view.
#[test]
fn sim_session_switch_never_inherits_the_previous_identity_view() -> Result<(), String> {
    for seed in 0..120_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(230_000));
        let mut real = SessionStore::new();
        let mut model = StoreModel {
            invariant: None,
            focus: None,
            workspace: None,
            governance: false,
            started: None,
            label: None,
        };
        let mut switches = 0_usize;
        for step in 0..150_usize {
            match random.below(6) {
                0 | 1 => {
                    let tenant = format!("tenant-{}", random.below(3));
                    let session = session_in(&tenant, SESSION_ID)?;
                    let label = if random.below(2) == 0 {
                        None
                    } else {
                        Some(format!("label-{}", random.below(5)))
                    };
                    let now = random.next_u64();
                    if model.invariant.as_ref() != Some(&session) {
                        switches = switches.saturating_add(1);
                        model.focus = None;
                        model.workspace = None;
                        model.governance = false;
                    }
                    model.invariant = Some(session.clone());
                    model.started = Some(now);
                    model.label.clone_from(&label);
                    real.start_session(session, label, now);
                }
                2 => {
                    let zone = format!("zone-{}", random.below(3));
                    real.set_focus(Some(zone.clone()));
                    model.focus = Some(zone);
                }
                3 => {
                    let space = format!("space-{}", random.below(3));
                    real.set_active_workspace(Some(space.clone()));
                    model.workspace = Some(space);
                }
                4 => {
                    let visible = random.below(2) == 0;
                    real.set_governance_visible(visible);
                    model.governance = visible;
                }
                _ => {
                    real.set_focus(None);
                    real.set_active_workspace(None);
                    model.focus = None;
                    model.workspace = None;
                }
            }
            ensure(
                real.invariant() == model.invariant.as_ref()
                    && real.focused_zone() == model.focus.as_deref()
                    && real.active_workspace() == model.workspace.as_deref()
                    && real.governance_visible() == model.governance
                    && real.session_started_ms() == model.started
                    && real.display_name() == model.label.as_deref(),
                &format!("seed {seed} step {step}: the store drifted from the model"),
            )?;
        }
        ensure(
            switches > 0,
            &format!("seed {seed}: the session never changed"),
        )?;
    }
    Ok(())
}

/// Bug caught: two stores (two tenants in one process) that share state, or a
/// clone that keeps writing through to the original.
#[test]
fn sim_session_stores_and_clones_are_independent() -> Result<(), String> {
    for seed in 0..60_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(240_000));
        let mut first = SessionStore::new();
        let mut second = SessionStore::new();
        first.start_session(session_in("tenant-a", SESSION_ID)?, None, 1);
        second.start_session(session_in("tenant-b", SESSION_ID)?, None, 2);
        first.set_focus(Some(String::from("a-focus")));
        first.set_governance_visible(true);
        let snapshot = first.clone();
        for _ in 0..50 {
            match random.below(3) {
                0 => first.set_focus(Some(random.text(4))),
                1 => first.set_active_workspace(Some(random.text(4))),
                _ => first.set_display_name(Some(random.text(4))),
            }
        }
        ensure(
            second.focused_zone().is_none()
                && second.active_workspace().is_none()
                && second.display_name().is_none()
                && !second.governance_visible()
                && second.session_started_ms() == Some(2),
            &format!("seed {seed}: another store's writes reached this one"),
        )?;
        ensure(
            snapshot.focused_zone() == Some("a-focus")
                && snapshot.active_workspace().is_none()
                && snapshot.display_name().is_none()
                && snapshot.governance_visible()
                && snapshot.session_started_ms() == Some(1),
            &format!("seed {seed}: a clone followed its original"),
        )?;
        ensure(
            first.invariant() != second.invariant(),
            "two tenants must hold two invariants",
        )?;
    }
    Ok(())
}

/// Bug caught: a store that invents state before a session begins (a
/// default focus, a visible governance toggle, a start stamp), which would
/// show governance affordances to a user who has no session yet.
#[test]
fn sim_session_new_store_starts_empty_and_hidden() -> Result<(), String> {
    for _ in 0..3 {
        let empty = SessionStore::default();
        ensure(
            empty.invariant().is_none()
                && empty.focused_zone().is_none()
                && empty.active_workspace().is_none()
                && empty.session_started_ms().is_none()
                && empty.display_name().is_none()
                && !empty.governance_visible(),
            "a new store must hold nothing and hide governance",
        )?;
    }
    let mut store = SessionStore::new();
    store.start_session(session_in("tenant-z", SESSION_ID)?, None, u64::MAX);
    ensure(
        store.session_started_ms() == Some(u64::MAX) && !store.governance_visible(),
        "the start stamp is the caller's clock and governance stays hidden",
    )
}
