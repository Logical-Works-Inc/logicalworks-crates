//! Seeded broker campaigns for the audit fixes: the subscription view never
//! carries a handle (fix 1), every refusal reaches the audit sink (fix 3),
//! ML peers are bounded per tenant (fix 4), and a payload is bounded by its
//! serialized size (fix 7).
//!
//! Each campaign is driven by one seed and judged by a model written from the
//! rules, never by the broker's own output. Every run is replayed: the same
//! seed gives the same trace hash and a distinct seed diverges.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use drishti::canvas_protocol::Capability;
use drishti::native_host::broker::{
    AuditAction, AuditEvent, BrokerError, Decision, Emitter, IntentHandle, Limit, MlHandle, Reach,
    RegisteredHandle, SignedMlGrant, Subject, TapEvent,
};
use drishti::native_host::protocol::{JsonValue, PartialIntent, SignalPattern};
use lgwks_std::id::Uuid;

use crate::broker_scope::{ids_of, replays};
use crate::broker_shared::{
    Harness, SESSION_SHARED, collector, lock_vec, register, register_listen, session,
    subscribe_all, tap_own,
};
use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// A nonce no registration holds.
fn stray_nonce(random: &mut SimRng) -> Uuid {
    let high = u128::from(random.next_u64()).wrapping_shl(64) | u128::from(random.next_u64());
    Uuid::from(high.to_le_bytes())
}

/// The audit events published since `before` events existed.
pub(crate) fn since(harness: &Harness, before: usize) -> Vec<AuditEvent> {
    harness.audit_events().into_iter().skip(before).collect()
}

/// Require exactly one new event since `before`: `action`, denied with
/// `error`. Returns it for further checks.
pub(crate) fn one_denial(
    harness: &Harness,
    before: usize,
    action: &AuditAction,
    error: &BrokerError,
) -> Result<AuditEvent, String> {
    let fresh = since(harness, before);
    ensure(fresh.len() == 1, || {
        format!("want one audit event for {action:?}, got {}", fresh.len())
    })?;
    let event = fresh.first().ok_or("no audit event")?;
    ensure(
        event.action() == action && event.decision() == &Decision::Denied(error.clone()),
        || format!("want {action:?} denied {error:?}, got {event:?}"),
    )?;
    Ok(event.clone())
}

/// A neighbour in the same session and tenant reads the owner's subscription
/// view and tries every handle the view could give it.
fn run_view_revocation(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let owner = register_listen(&harness, "widget.owner", "tenant-a", SESSION_SHARED)?;
    let sibling = register(&harness, "widget.sibling", "tenant-a", SESSION_SHARED)?;
    let heard = subscribe_all(&harness, &owner)?;
    let tapped = tap_own(&harness, &owner, Arc::new(|_: TapEvent| {}))?;
    let views = harness
        .broker()
        .registered_subscriptions(&sibling, Reach::Own)
        .map_err(|error| format!("{error}"))?;
    let view = views
        .first()
        .ok_or_else(|| String::from("the sibling must see the owner's subscription"))?;
    ensure(
        views.len() == 1
            && view.descriptor_id() == "widget.owner"
            && view.tenant_id() == "tenant-a"
            && view.session_id() == SESSION_SHARED,
        || String::from("the view must name the owner's widget and scope"),
    )?;
    let mut pings = 0_usize;
    for _ in 0..80 {
        let before = harness.audit_events().len();
        // Whatever the view gave: its names, with a nonce the attacker picks.
        let forged = RegisteredHandle::new(
            stray_nonce(&mut random),
            String::from(view.session_id()),
            String::from(view.descriptor_id()),
            String::from(view.tenant_id()),
            String::from(crate::fixtures::USER_SUB),
        );
        let revoke_sub = AuditAction::Revoke(Subject::Subscription);
        let revoke_tap = AuditAction::Revoke(Subject::Tap);
        let attempt = random.below(5);
        let (action, outcome) = match attempt {
            0 => (
                revoke_sub,
                harness.broker().unsubscribe(&sibling, &view.id()),
            ),
            1 => (
                revoke_sub,
                harness.broker().unsubscribe(&forged, &view.id()),
            ),
            2 => (revoke_tap, harness.broker().untap(&sibling, &tapped)),
            3 => (revoke_tap, harness.broker().untap(&forged, &tapped)),
            _ => {
                harness.broker().unregister(&forged);
                let widget = AuditAction::Revoke(Subject::Widget(String::from("widget.owner")));
                (widget, Err(BrokerError::InvalidHandle))
            }
        };
        ensure(outcome == Err(BrokerError::InvalidHandle), || {
            format!("attempt {attempt} must be refused, got {outcome:?}")
        })?;
        one_denial(&harness, before, &action, &BrokerError::InvalidHandle)?;
        trace.mix_u64(attempt);
        if random.below(4) == 0 {
            harness
                .broker()
                .emit(&owner, "ping", JsonValue::Null)
                .map_err(|error| format!("owner emit failed: {error}"))?;
            pings = pings.saturating_add(1);
        }
    }
    ensure(lock_vec(&heard).len() == pings, || {
        String::from("the owner's subscription must keep delivering")
    })?;
    let still = ids_of(&harness, &sibling, Reach::Own).map_err(|error| format!("{error}"))?;
    ensure(still.len() == 1 && still.contains(&view.id()), || {
        String::from("the owner's subscription must survive every attempt")
    })?;
    harness
        .broker()
        .untap(&owner, &tapped)
        .map_err(|error| format!("the owner must keep its tap: {error}"))?;
    trace.mix_len(pings);
    Ok(trace.finish())
}

/// Neighbour. Bug caught: a subscription view that hands out the owner's
/// handle (nonce included), so any reader in scope revokes another widget.
#[test]
fn sim_broker_neighbour_view_never_lends_the_owners_handle() -> Result<(), String> {
    replays(run_view_revocation, [7, 43, 619], 8)
}

/// A grant with an ML-only capability for `tenant`, with a fresh identity.
fn ml_grant(random: &mut SimRng, tenant: &str) -> SignedMlGrant {
    let id = u128::from(random.next_u64()).wrapping_shl(64) | u128::from(random.next_u64());
    SignedMlGrant::new(
        Uuid::from(id.to_le_bytes()),
        String::from(tenant),
        vec![Capability::TapeRead],
        9_999_999_999,
        1,
        vec![1],
    )
}

/// Register an ML peer in `tenant`'s shared session.
fn ml_peer(
    harness: &Harness,
    random: &mut SimRng,
    tenant: &str,
) -> Result<Result<MlHandle, BrokerError>, String> {
    let scope = session(tenant, SESSION_SHARED)?;
    Ok(harness
        .broker()
        .register_ml_peer(ml_grant(random, tenant), scope))
}

/// An intent whose payload is `payload`.
fn intent(payload: JsonValue) -> PartialIntent {
    PartialIntent::new(String::from("intent.x"), payload, String::from("x"))
}

/// One refusal path: what the refused call returned, the tenant the audit
/// event must name, and the action it must carry.
struct Refusal {
    /// The refusal the call returned, if it was refused.
    got: Option<BrokerError>,
    /// Tenant the audit event must name as the caller.
    tenant: &'static str,
    /// Action the audit event must carry.
    action: AuditAction,
}

/// Registered identities the refusal paths act as.
struct Rig {
    /// A registered widget of `tenant-a`.
    widget: RegisteredHandle,
    /// The registered command bar of `tenant-a`.
    bar: IntentHandle,
}

/// Take refusal path `path` (0 to 7) with a nonce nobody holds.
fn refuse(harness: &Harness, rig: &Rig, path: u64, stray: Uuid) -> Result<Refusal, String> {
    let broker = harness.broker();
    let giant = || JsonValue::String("a".repeat(Limit::PayloadBytes.cap()));
    let user = String::from(crate::fixtures::USER_SUB);
    let shared = String::from(SESSION_SHARED);
    let widget_emit = AuditAction::Emit(Emitter::Widget);
    let refusal = |got: Option<BrokerError>, tenant: &'static str, action: AuditAction| Refusal {
        got,
        tenant,
        action,
    };
    Ok(match path {
        0 => {
            let lost = RegisteredHandle::new(
                stray,
                shared,
                String::from("w"),
                String::from("tenant-x"),
                user,
            );
            refusal(
                broker.emit(&lost, "tick", JsonValue::Null).err(),
                "tenant-x",
                widget_emit,
            )
        }
        1 => {
            let got = broker.emit(&rig.widget, "a..b", JsonValue::Null).err();
            refusal(got, "tenant-a", widget_emit)
        }
        2 => refusal(
            broker.emit(&rig.widget, "tick", giant()).err(),
            "tenant-a",
            widget_emit,
        ),
        3 => {
            let lost = MlHandle::new(stray, String::from("tenant-x"));
            let got = broker.emit_prediction(&lost, "ml.x", JsonValue::Null).err();
            refusal(got, "tenant-x", AuditAction::Emit(Emitter::Prediction))
        }
        4 => {
            let lost = IntentHandle::new(stray, shared, String::from("tenant-x"), user);
            let got = broker.emit_intent(&lost, intent(JsonValue::Null)).err();
            refusal(got, "tenant-x", AuditAction::Emit(Emitter::Intent))
        }
        5 => {
            let got = broker.emit_intent(&rig.bar, intent(giant())).err();
            refusal(got, "tenant-a", AuditAction::Emit(Emitter::Intent))
        }
        6 => {
            let log = Arc::new(Mutex::new(Vec::new()));
            let pattern = SignalPattern::new(String::from("**"), None);
            let id = broker
                .subscribe(&rig.widget, pattern, collector(&log))
                .map_err(|error| format!("{error}"))?;
            broker
                .unsubscribe(&rig.widget, &id)
                .map_err(|error| format!("{error}"))?;
            let got = broker.unsubscribe(&rig.widget, &id).err();
            refusal(got, "tenant-a", AuditAction::Revoke(Subject::Subscription))
        }
        _ => {
            let id = tap_own(harness, &rig.widget, Arc::new(|_: TapEvent| {}))?;
            broker
                .untap(&rig.widget, &id)
                .map_err(|error| format!("{error}"))?;
            let got = broker.untap(&rig.widget, &id).err();
            refusal(got, "tenant-a", AuditAction::Revoke(Subject::Tap))
        }
    })
}

/// The refusal path `path` must return.
fn refusal_error(path: u64) -> BrokerError {
    match path {
        1 => BrokerError::MalformedKind,
        2 | 5 => BrokerError::LimitExceeded(Limit::PayloadBytes),
        _ => BrokerError::InvalidHandle,
    }
}

/// Run every refusal path of the emit family, the unknown-id revocations, and
/// the rate limit; each must audit exactly one named denial.
fn run_refusals(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let widget = register(&harness, "widget.emit", "tenant-a", SESSION_SHARED)?;
    let bar = harness
        .broker()
        .register_command_bar(session("tenant-a", SESSION_SHARED)?)
        .map_err(|error| format!("{error}"))?;
    let rig = Rig { widget, bar };
    for _ in 0..120 {
        let path = random.below(8);
        let denied = refuse(&harness, &rig, path, stray_nonce(&mut random))?;
        let error = refusal_error(path);
        ensure(denied.got.as_ref() == Some(&error), || {
            format!(
                "path {path} must refuse with {error:?}, got {:?}",
                denied.got
            )
        })?;
        // The refusal is the last event; paths 6 and 7 audit setup before it.
        let last = harness.audit_events().len().saturating_sub(1);
        let event = one_denial(&harness, last, &denied.action, &error)?;
        ensure(event.caller().scope().tenant_id() == denied.tenant, || {
            format!("path {path} must audit the caller's tenant")
        })?;
        trace.mix_u64(path);
    }
    rate_limit_denial(&harness)?;
    Ok(trace.finish())
}

/// A frozen clock spends the 200 burst, then the 201st emit is a denial.
fn rate_limit_denial(harness: &Harness) -> Result<(), String> {
    let burst = register(harness, "widget.burst", "tenant-b", SESSION_SHARED)?;
    for _ in 0..200 {
        harness
            .broker()
            .emit(&burst, "tick", JsonValue::Null)
            .map_err(|error| format!("the burst must be admitted: {error}"))?;
    }
    let before = harness.audit_events().len();
    ensure(
        harness.broker().emit(&burst, "tick", JsonValue::Null) == Err(BrokerError::RateLimited),
        || String::from("the 201st frozen emit must be rate limited"),
    )?;
    let action = AuditAction::Emit(Emitter::Widget);
    let event = one_denial(harness, before, &action, &BrokerError::RateLimited)?;
    ensure(event.caller().scope().tenant_id() == "tenant-b", || {
        String::from("a rate limit must audit the registered caller")
    })
}

/// Saboteur. Bug caught: an emit-family refusal (forged handle, bad kind,
/// oversize payload, rate limit) or an unknown-id revocation that publishes
/// no audit event (issue drishti#9: every decision is audited).
#[test]
fn sim_broker_saboteur_every_emit_refusal_is_audited() -> Result<(), String> {
    replays(run_refusals, [11, 59, 733], 12)
}

/// A payload of shape `kind` and `size`, with the serialized bytes the model
/// counts for it by hand.
fn shaped(kind: u64, size: usize) -> (JsonValue, usize) {
    match kind {
        0 => (JsonValue::String("a".repeat(size)), size.saturating_add(2)),
        1 => (
            JsonValue::String("\"".repeat(size)),
            size.saturating_mul(2).saturating_add(2),
        ),
        2 => (
            JsonValue::Array(vec![JsonValue::Null; size]),
            size.saturating_mul(5).saturating_add(1),
        ),
        _ => {
            let member = JsonValue::String("a".repeat(size));
            let object = BTreeMap::from([(String::from("k"), member)]);
            (JsonValue::Object(object), size.saturating_add(8))
        }
    }
}

/// Seeded payload shapes cluster around the byte bound; a model that counts
/// serialized bytes by hand decides which sit inside it.
fn run_payload_bounds(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let widget = register(&harness, "widget.giant", "tenant-a", SESSION_SHARED)?;
    let cap = Limit::PayloadBytes.cap();
    for _ in 0..60 {
        let kind = random.below(4);
        let centre = match kind {
            0 => cap.saturating_sub(2),
            1 => cap.saturating_sub(2).wrapping_div(2),
            2 => cap.saturating_sub(1).wrapping_div(5),
            _ => cap.saturating_sub(8),
        };
        let size = centre.saturating_add(random.below_len(7)).saturating_sub(3);
        let (payload, bytes) = shaped(kind, size);
        let before = harness.audit_events().len();
        let outcome = harness.broker().emit(&widget, "tick", payload);
        let inside = bytes <= cap;
        let over = Err(BrokerError::LimitExceeded(Limit::PayloadBytes));
        ensure(
            outcome.map(|_| ()) == if inside { Ok(()) } else { over },
            || format!("shape {kind} of {bytes} bytes: inside the bound is {inside}"),
        )?;
        let fresh = since(&harness, before);
        let routed = matches!(
            fresh.first().map(AuditEvent::action),
            Some(&AuditAction::Route { .. })
        );
        ensure(fresh.len() == 1 && routed == inside, || {
            String::from("an admitted payload routes once, a refused one denies once")
        })?;
        trace.mix_len(bytes);
    }
    boundary_on_other_paths(&harness, &mut random)?;
    Ok(trace.finish())
}

/// The ML and command-bar paths hold the same bound at the same byte.
fn boundary_on_other_paths(harness: &Harness, random: &mut SimRng) -> Result<(), String> {
    let cap = Limit::PayloadBytes.cap();
    let peer = ml_peer(harness, random, "tenant-a")?.map_err(|error| format!("{error}"))?;
    let bar = harness
        .broker()
        .register_command_bar(session("tenant-a", SESSION_SHARED)?)
        .map_err(|error| format!("{error}"))?;
    let (fits, _) = shaped(0, cap.saturating_sub(2));
    let (spills, _) = shaped(0, cap.saturating_sub(1));
    let refusal = Err(BrokerError::LimitExceeded(Limit::PayloadBytes));
    let broker = harness.broker();
    let checks = [
        broker
            .emit_prediction(&peer, "ml.x", fits.clone())
            .map(|_| ()),
        broker
            .emit_prediction(&peer, "ml.x", spills.clone())
            .map(|_| ()),
        broker.emit_intent(&bar, intent(fits)).map(|_| ()),
        broker.emit_intent(&bar, intent(spills)).map(|_| ()),
    ];
    ensure(checks == [Ok(()), refusal.clone(), Ok(()), refusal], || {
        format!("the bound must hold on every path: {checks:?}")
    })
}

/// Giant-and-empty. Bug caught: a payload with no byte bound (a 60 MiB string
/// is stamped, cloned to every listener and tap), or a bound that counts the
/// wrong bytes (escapes, separators, keys).
#[test]
fn sim_broker_giant_payload_refused_at_the_serialized_byte_bound() -> Result<(), String> {
    replays(run_payload_bounds, [2, 37, 541], 3)
}

/// Peers register and retire across nine tenants against a model of the
/// per-tenant and global counts.
fn run_ml_bounds(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let tenants = [
        "ml-0", "ml-1", "ml-2", "ml-3", "ml-4", "ml-5", "ml-6", "ml-7", "ml-8",
    ];
    let mut live: Vec<Vec<MlHandle>> = tenants.iter().map(|_| Vec::new()).collect();
    for _ in 0..500 {
        let side = random.below_len(tenants.len());
        let tenant = tenants
            .get(side)
            .ok_or_else(|| String::from("tenant out of range"))?;
        let total: usize = live.iter().map(Vec::len).sum();
        let row = live
            .get_mut(side)
            .ok_or_else(|| String::from("tenant out of range"))?;
        if random.below(10) >= 7 {
            if let Some(handle) = row.pop() {
                harness.broker().unregister_ml_peer(&handle);
            }
        } else {
            let want = if total >= Limit::MlPeers.cap() {
                Some(BrokerError::LimitExceeded(Limit::MlPeers))
            } else if row.len() >= Limit::MlPeersPerTenant.cap() {
                Some(BrokerError::LimitExceeded(Limit::MlPeersPerTenant))
            } else {
                None
            };
            let before = harness.audit_events().len();
            match (ml_peer(&harness, &mut random, tenant)?, want) {
                (Ok(handle), None) => row.push(handle),
                (Err(error), Some(refused)) if error == refused => {}
                (other, expected) => ensure(false, || {
                    format!("peer gave {other:?}, the model wants {expected:?}")
                })?,
            }
            ensure(since(&harness, before).len() == 1, || {
                String::from("a peer registration audits once")
            })?;
        }
        trace.mix_len(live.iter().map(Vec::len).sum());
    }
    Ok(trace.finish())
}

/// Neighbour. Bug caught: one tenant fills the global ML peer registry so
/// every other tenant is refused (only a global bound of 64 existed).
#[test]
fn sim_broker_neighbour_ml_peers_never_starve_other_tenants() -> Result<(), String> {
    replays(run_ml_bounds, [5, 71, 449], 6)
}
