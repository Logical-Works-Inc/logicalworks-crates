//! Seeded broker abuse campaigns (issues drishti#8 to drishti#10): a monkey over the whole
//! API, neighbours on one session, a noisy neighbour, and a saboteur that
//! audits the audit stream. One seed drives order, identities, and the clock;
//! every oracle is a model or an invariant, never the broker's own answer.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{Arc, Mutex};

use drishti::canvas_protocol::Capability;
use drishti::native_host::broker::{
    AuditAction, BrokerError, Inspector, Permission, Reach, RegisteredHandle, SubscriptionId,
    TapEvent, TapListener,
};
use drishti::native_host::protocol::{JsonValue, SignalPattern};

use crate::broker_scope::{ids_of, replays};
use crate::broker_shared::{
    Harness, SESSION_SHARED, collector, exposure, register, register_listen, session,
    subscribe_all, widget,
};
use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// The tenants every campaign shares one session id across.
const TENANTS: [&str; 3] = ["tenant-a", "tenant-b", "tenant-c"];
/// A small id pool, so the monkey collides with itself.
const POOL: [&str; 5] = ["w.0", "w.1", "w.2", "w.3", "w.4"];

/// A live widget in the model.
struct Live {
    /// Index into [`TENANTS`].
    tenant: usize,
    /// Widget id.
    id: &'static str,
    /// The broker's handle.
    handle: RegisteredHandle,
    /// Subscriptions the widget owns.
    subs: Vec<SubscriptionId>,
}

/// A listener that logs nothing.
fn quiet_tap() -> TapListener {
    Arc::new(|_: TapEvent| {})
}

/// What a monkey run leaves for the saboteur.
struct Trail {
    /// The broker and its capture.
    harness: Harness,
    /// Signal id to the tenant that emitted it, from the model.
    routed: BTreeMap<String, usize>,
    /// Replay hash.
    hash: u64,
}

/// The tenant a model slot names.
fn tenant_name(index: usize) -> Result<&'static str, String> {
    TENANTS
        .get(index)
        .copied()
        .ok_or_else(|| String::from("tenant out of range"))
}

/// Walk the whole broker API in nonsense order against a model of the live
/// widgets, their subscriptions, and the duplicate rule.
fn run_monkey(seed: u64) -> Result<Trail, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut live: Vec<Live> = Vec::new();
    let mut routed = BTreeMap::new();
    for _ in 0..400 {
        let pick = random.below_len(live.len());
        match random.below(7) {
            0 => {
                let tenant = random.below_len(TENANTS.len());
                let id = *POOL.get(random.below_len(POOL.len())).ok_or("pool")?;
                let taken = live
                    .iter()
                    .any(|held| held.tenant == tenant && held.id == id);
                let outcome = harness.broker().register(
                    widget(id, vec![Capability::SignalEmit], false),
                    session(tenant_name(tenant)?, SESSION_SHARED)?,
                );
                match (taken, outcome) {
                    (false, Ok(handle)) => live.push(Live {
                        tenant,
                        id,
                        handle,
                        subs: Vec::new(),
                    }),
                    (true, Err(BrokerError::DuplicateSourceId(again))) if again == id => {}
                    (state, other) => {
                        ensure(false, || format!("register: taken {state} met {other:?}"))?
                    }
                }
            }
            1 => {
                if let Some(held) = live.get_mut(pick) {
                    let pattern = SignalPattern::new(String::from("**"), None);
                    let made = harness
                        .broker()
                        .subscribe(
                            &held.handle,
                            pattern,
                            collector(&Arc::new(Mutex::new(Vec::new()))),
                        )
                        .map_err(|error| format!("subscribe: {error}"))?;
                    held.subs.push(made);
                }
            }
            2 => unsubscribe_step(&harness, &mut live, pick, &mut random)?,
            3 => {
                if pick < live.len() {
                    let gone = live.swap_remove(pick);
                    harness.broker().unregister(&gone.handle);
                    harness.broker().unregister(&gone.handle);
                    let after = harness.broker().emit(&gone.handle, "ping", JsonValue::Null);
                    ensure(after.err() == Some(BrokerError::InvalidHandle), || {
                        String::from("a retired handle must not emit")
                    })?;
                }
            }
            4 => {
                if let Some(held) = live.get(pick) {
                    match harness.broker().emit(&held.handle, "ping", JsonValue::Null) {
                        Ok(envelope) => {
                            routed.insert(envelope.signal_id().to_owned(), held.tenant);
                        }
                        Err(BrokerError::RateLimited) => {}
                        Err(other) => ensure(false, || format!("emit: {other:?}"))?,
                    }
                }
            }
            5 => read_step(&harness, &live, pick)?,
            _ => tap_step(&harness, &live, pick)?,
        }
        trace.mix_len(live.len());
    }
    Ok(Trail {
        hash: trace.finish(),
        harness,
        routed,
    })
}

/// A seeded actor tries to cancel the newest subscription of widget `pick`;
/// only the owner may, and a stranger is refused with an invalid handle.
fn unsubscribe_step(
    harness: &Harness,
    live: &mut [Live],
    pick: usize,
    random: &mut SimRng,
) -> Result<(), String> {
    let actor = live
        .get(random.below_len(live.len()))
        .map(|held| held.handle.clone());
    let owned = live.get(pick).and_then(|held| held.subs.last().copied());
    if let (Some(actor), Some(sub)) = (actor, owned) {
        let result = harness.broker().unsubscribe(&actor, &sub);
        let by_owner = live.get(pick).is_some_and(|held| held.handle == actor);
        match (by_owner, result) {
            (true, Ok(())) => {
                if let Some(held) = live.get_mut(pick) {
                    held.subs.pop();
                }
            }
            (false, Err(BrokerError::InvalidHandle)) => {}
            (state, other) => ensure(false, || {
                format!("unsubscribe: owner {state} met {other:?}")
            })?,
        }
    }
    Ok(())
}

/// Widget `pick` reads its own subscriptions, which must equal the model, and
/// is refused the wide read.
fn read_step(harness: &Harness, live: &[Live], pick: usize) -> Result<(), String> {
    if let Some(held) = live.get(pick) {
        let wanted: BTreeSet<SubscriptionId> = live
            .iter()
            .filter(|other| other.tenant == held.tenant)
            .flat_map(|other| other.subs.iter().copied())
            .collect();
        ensure(
            ids_of(harness, &held.handle, Reach::Own) == Ok(wanted),
            || String::from("own read must equal the model"),
        )?;
        ensure(
            ids_of(harness, &held.handle, Reach::AllTenants) == Err(BrokerError::CrossTenantDenied),
            || String::from("a wide read needs the grant"),
        )?;
    }
    Ok(())
}

/// Widget `pick` opens a tap and cancels it twice; only the first succeeds.
fn tap_step(harness: &Harness, live: &[Live], pick: usize) -> Result<(), String> {
    if let Some(held) = live.get(pick) {
        let tap = harness
            .broker()
            .tap(&held.handle, Reach::Own, quiet_tap())
            .map_err(|error| format!("tap: {error}"))?;
        let first = harness.broker().untap(&held.handle, &tap);
        let second = harness.broker().untap(&held.handle, &tap);
        ensure(
            first == Ok(()) && second == Err(BrokerError::InvalidHandle),
            || String::from("a tap is cancelled once"),
        )?;
    }
    Ok(())
}

/// The monkey's hash, for replay.
fn monkey_hash(seed: u64) -> Result<u64, String> {
    run_monkey(seed).map(|trail| trail.hash)
}

/// Monkey. Bug caught: any state the documented path never reaches: a
/// duplicate admitted, a stranger revoking, a retired handle emitting, an
/// own read that drifts from the model.
#[test]
fn sim_broker_monkey_whole_api_agrees_with_the_model() -> Result<(), String> {
    replays(monkey_hash, [41, 211, 971], 42)
}

/// After a monkey run the audit stream is scanned by an independent model:
/// ordinals contiguous, every route attributed to the tenant that emitted it,
/// every emitted signal routed exactly once, no caller outside the tenants.
fn run_audit_hygiene(seed: u64) -> Result<u64, String> {
    let trail = run_monkey(seed)?;
    let events = trail.harness.audit_events();
    let mut seen = BTreeSet::new();
    for (index, event) in events.iter().enumerate() {
        let ordinal = u64::try_from(index).map_err(|error| format!("{error}"))?;
        ensure(event.seq() == ordinal, || {
            String::from("ordinals must be contiguous")
        })?;
        let tenant = event.caller().scope().tenant_id();
        ensure(TENANTS.contains(&tenant), || {
            format!("a caller outside the tenants: {tenant:?}")
        })?;
        if let AuditAction::Route { ref signal_id, .. } = *event.action() {
            let owner = trail
                .routed
                .get(signal_id)
                .copied()
                .ok_or("a route the model never emitted")?;
            ensure(
                tenant_name(owner)? == tenant && seen.insert(signal_id.clone()),
                || String::from("a route attributed to the wrong tenant or twice"),
            )?;
        }
    }
    ensure(seen.len() == trail.routed.len(), || {
        String::from("every emitted signal must be routed once")
    })?;
    Ok(trail.hash ^ u64::try_from(events.len()).map_err(|error| format!("{error}"))?)
}

/// Saboteur. Bug caught: an audit event that names another tenant's caller,
/// drops or doubles a route, or leaves a gap in the ordinals.
#[test]
fn sim_broker_saboteur_no_foreign_byte_in_the_audit_stream() -> Result<(), String> {
    replays(run_audit_hygiene, [43, 223, 983], 44)
}

/// Several tenants on one session id; every inspector, at every reach, for
/// every caller, is compared with the model of who may see whom.
fn run_exposure(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let count = random.below_len(TENANTS.len()).saturating_add(1).max(2);
    let mut handles = Vec::new();
    for index in 0..count {
        let handle = register_listen(
            &harness,
            &format!("widget.t{index}"),
            tenant_name(index)?,
            SESSION_SHARED,
        )?;
        subscribe_all(&harness, &handle)?;
        handles.push(handle);
    }
    let lucky = random.below_len(count);
    let everyone: BTreeSet<(String, String)> = (0..count)
        .map(|index| {
            tenant_name(index).map(|name| (String::from(name), String::from(SESSION_SHARED)))
        })
        .collect::<Result<_, _>>()?;
    for which in Inspector::ALL {
        for (index, caller) in handles.iter().enumerate() {
            let own = BTreeSet::from([(
                String::from(tenant_name(index)?),
                String::from(SESSION_SHARED),
            )]);
            let seen = exposure(&harness, which, caller, Reach::Own, &handles)?;
            ensure(seen == Ok(own), || {
                format!("{which:?} leaked beyond the caller's tenant")
            })?;
        }
    }
    harness
        .grants()
        .grant(Some(tenant_name(lucky)?), Permission::CrossTenantInspect);
    for which in Inspector::ALL {
        for (index, caller) in handles.iter().enumerate() {
            let wide = exposure(&harness, which, caller, Reach::AllTenants, &handles)?;
            let expected = if index == lucky {
                Ok(everyone.clone())
            } else {
                Err(BrokerError::CrossTenantDenied)
            };
            ensure(wide == expected, || {
                format!("{which:?} wide read must follow the per-tenant grant")
            })?;
        }
    }
    harness.grants().clear();
    let revoked = handles.get(lucky).map(|caller| {
        exposure(
            &harness,
            Inspector::Subscriptions,
            caller,
            Reach::AllTenants,
            &handles,
        )
    });
    ensure(
        revoked == Some(Ok(Err(BrokerError::CrossTenantDenied))),
        || String::from("a revoked grant must stop the wide read"),
    )?;
    trace.mix_len(count);
    trace.mix_len(lucky);
    Ok(trace.finish())
}

/// Neighbour. Bug caught: an inspector that skips the shared scope filter, a
/// grant that is not per tenant, or a revocation the store's flip never
/// reaches. Iterating `Inspector::ALL` makes a new inspector without a driver
/// fail here.
#[test]
fn sim_broker_neighbour_every_inspector_stays_in_scope() -> Result<(), String> {
    replays(run_exposure, [47, 229, 991], 48)
}

/// One tenant floods with the id and session another tenant also uses; the
/// other's bucket is its own, and every refused flood emit is audited as one
/// denial, so the stream grows by exactly one event per emit.
fn run_noisy_neighbour(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let loud = register(&harness, "widget.same", "tenant-a", SESSION_SHARED)?;
    let quiet = register(&harness, "widget.same", "tenant-b", SESSION_SHARED)?;
    let flood = random.below_len(500).saturating_add(1_000);
    let mut admitted = 0_usize;
    for _ in 0..flood {
        match harness.broker().emit(&loud, "ping", JsonValue::Null) {
            Ok(_) => admitted = admitted.saturating_add(1),
            Err(BrokerError::RateLimited) => {}
            Err(other) => ensure(false, || format!("flood: {other:?}"))?,
        }
    }
    ensure(admitted == 200, || {
        format!("a frozen clock admits exactly the burst, got {admitted}")
    })?;
    for _ in 0..200 {
        harness
            .broker()
            .emit(&quiet, "ping", JsonValue::Null)
            .map_err(|error| format!("the neighbour was starved: {error}"))?;
    }
    ensure(
        harness.broker().emit(&quiet, "ping", JsonValue::Null) == Err(BrokerError::RateLimited),
        || String::from("the neighbour's own bucket must also bind"),
    )?;
    // Two registrations, every admitted route, and one denial per refusal.
    let refused = flood.saturating_sub(200).saturating_add(1);
    let expected = refused.saturating_add(402);
    ensure(
        harness.audit_events().len() == expected && harness.audited().len() == 400,
        || String::from("every emit must audit once, admitted or refused"),
    )?;
    trace.mix_len(flood);
    Ok(trace.finish())
}

/// Neighbour. Bug caught: rate buckets or throttle counters keyed without the
/// tenant, so a flood spends or inherits another tenant's tokens; a refused
/// emit that leaves no audit event (issue drishti#9).
#[test]
fn sim_broker_neighbour_noisy_tenant_cannot_starve_or_flood_the_stream() -> Result<(), String> {
    replays(run_noisy_neighbour, [53, 239, 997], 54)
}

/// A panicking tap surfaces from every emit like a panicking listener, after
/// the route was audited; cancelling it restores the broker.
fn run_panicking_tap(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let widget_handle = register(&harness, "widget.p", "tenant-a", SESSION_SHARED)?;
    let tap = harness
        .broker()
        .tap(
            &widget_handle,
            Reach::Own,
            Arc::new(|_: TapEvent| resume_unwind(Box::new("tap fault"))),
        )
        .map_err(|error| format!("tap failed: {error}"))?;
    let rounds = random.below_len(5).saturating_add(1);
    for round in 1..=rounds {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            harness
                .broker()
                .emit(&widget_handle, "ping", JsonValue::Null)
        }));
        ensure(outcome.is_err() && harness.audited().len() == round, || {
            String::from("the panic must surface after the route is audited")
        })?;
    }
    harness
        .broker()
        .untap(&widget_handle, &tap)
        .map_err(|error| format!("untap failed: {error}"))?;
    harness
        .broker()
        .emit(&widget_handle, "ping", JsonValue::Null)
        .map_err(|error| format!("the broker must stay usable: {error}"))?;
    ensure(harness.audited().len() == rounds.saturating_add(1), || {
        String::from("the recovery emit must be audited too")
    })?;
    trace.mix_len(rounds);
    Ok(trace.finish())
}

/// Saboteur. Bug caught: a tap panic swallowed (or caught) before the route is
/// audited, or one that leaves the broker locked or unusable.
#[test]
fn sim_broker_saboteur_panicking_tap_surfaces_after_the_route_is_audited() -> Result<(), String> {
    replays(run_panicking_tap, [59, 241, 1_009], 60)
}
