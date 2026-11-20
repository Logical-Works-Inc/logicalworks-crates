//! Seeded broker bound, grant-store, and re-entrancy campaigns
//! (issues drishti#9 and drishti#10, plus the registry bounds).
//!
//! Same discipline as `broker_scope`: one seed drives every choice, an
//! independent model judges the outcome, and each run replays to one hash.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use drishti::canvas_protocol::Capability;
use drishti::native_host::broker::{
    AuditFn, Broker, BrokerError, ClockFn, Decision, GrantFn, GrantVerificationResult, IdFn, Limit,
    Permission, Reach, RegisteredHandle, SignedMlGrant, SubscriptionId, TapEvent, TapListener,
    VerifyGrantFn,
};
use drishti::native_host::protocol::{JsonValue, SignalPattern};
use lgwks_std::id::Uuid;

use crate::broker_scope::replays;
use crate::broker_shared::{
    Harness, SESSION_ONE, SESSION_SHARED, collector, lock_recover, register, session, widget,
};
use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// A catch-all pattern.
fn everything() -> SignalPattern {
    SignalPattern::new(String::from("**"), None)
}

/// A listener that logs nothing.
fn quiet() -> drishti::native_host::broker::SignalListener {
    collector(&Arc::new(Mutex::new(Vec::new())))
}

/// A tap that logs nothing.
fn noop_tap() -> TapListener {
    Arc::new(|_: TapEvent| {})
}

/// The tenant name of row `index`.
fn tenant_name(index: usize) -> String {
    format!("tenant-{index}")
}

/// Register `count` widgets for one tenant; ids repeat across tenants.
fn fill_widgets(
    harness: &Harness,
    tenant: &str,
    count: usize,
    into: &mut Vec<RegisteredHandle>,
) -> Result<(), String> {
    for index in 0..count {
        let id = format!("widget.{index}");
        into.push(register(harness, &id, tenant, SESSION_SHARED)?);
    }
    Ok(())
}

/// Register one more widget and return the refusal it must meet.
fn refused_widget(harness: &Harness, tenant: &str) -> Result<BrokerError, String> {
    harness
        .broker()
        .register(
            widget("widget.over", vec![Capability::SignalEmit], false),
            session(tenant, SESSION_SHARED)?,
        )
        .err()
        .ok_or_else(|| String::from("a full registry must refuse"))
}

/// A valid ML grant fixture for peer `index`.
fn ml_grant(index: usize) -> Result<SignedMlGrant, String> {
    let nonce = u128::try_from(index).map_err(|error| format!("{error}"))?;
    Ok(SignedMlGrant::new(
        Uuid::from(nonce.saturating_add(1).to_le_bytes()),
        String::from("tenant-0"),
        vec![Capability::TapeRead],
        9_999_999,
        0,
        vec![1_u8; 64],
    ))
}

/// The first widget of tenant row `index`.
fn owner_of(rows: &[Vec<RegisteredHandle>], index: usize) -> Result<RegisteredHandle, String> {
    rows.get(index)
        .and_then(|row| row.first())
        .cloned()
        .ok_or_else(|| String::from("owner missing"))
}

/// Subscribe the owner `count` times and keep the identities.
fn saturate_subscriptions(
    harness: &Harness,
    owner: &RegisteredHandle,
    held: &mut Vec<(RegisteredHandle, SubscriptionId)>,
    count: usize,
) -> Result<(), String> {
    for _ in 0..count {
        let id = harness
            .broker()
            .subscribe(owner, everything(), quiet())
            .map_err(|error| format!("{error}"))?;
        held.push((owner.clone(), id));
    }
    Ok(())
}

/// The widget registry after the widget phase: tenants in seeded order, each
/// tenant's handles, and the one widget registered after the bound was hit.
struct Fleet {
    /// Tenant rows in the seeded order the phase filled them.
    order: Vec<usize>,
    /// The handles of each tenant row.
    rows: Vec<Vec<RegisteredHandle>>,
    /// The tenant that filled alone first.
    first: usize,
    /// The widget admitted after a revoke freed a slot.
    latecomer: RegisteredHandle,
}

/// Fill the widget registry past its per-tenant and global bounds in seeded
/// tenant order; a revoke frees exactly one slot, for anyone.
fn widget_phase(
    harness: &Harness,
    random: &mut SimRng,
    trace: &mut TraceHash,
) -> Result<Fleet, String> {
    let per_tenant = Limit::WidgetsPerTenant.cap();
    let tenants = Limit::Widgets
        .cap()
        .checked_div(per_tenant)
        .ok_or_else(|| String::from("the per-tenant widget bound must be non-zero"))?;
    let mut order: Vec<usize> = (0..tenants).collect();
    for index in 0..order.len() {
        let swap = random.below_len(order.len());
        order.swap(index, swap);
    }
    let mut rows: Vec<Vec<RegisteredHandle>> = vec![Vec::new(); tenants];
    let first = *order
        .first()
        .ok_or_else(|| String::from("tenant order is empty"))?;
    // One tenant alone fills its own bound and meets the per-tenant refusal.
    let row = rows.get_mut(first).ok_or("row out of range")?;
    fill_widgets(harness, &tenant_name(first), per_tenant, row)?;
    ensure(
        refused_widget(harness, &tenant_name(first))?
            == BrokerError::LimitExceeded(Limit::WidgetsPerTenant),
        || String::from("tenant bound must refuse as per-tenant"),
    )?;
    // Everyone else is unaffected until the global bound binds.
    for &index in order.iter().skip(1) {
        let row = rows.get_mut(index).ok_or("row out of range")?;
        fill_widgets(harness, &tenant_name(index), per_tenant, row)?;
    }
    let newcomer = tenant_name(tenants);
    ensure(
        refused_widget(harness, &newcomer)? == BrokerError::LimitExceeded(Limit::Widgets),
        || String::from("global bound must refuse the newcomer"),
    )?;
    ensure(
        refused_widget(harness, &tenant_name(first))? == BrokerError::LimitExceeded(Limit::Widgets),
        || String::from("global bound is reported before the tenant's"),
    )?;
    // Recovery: one revoke frees exactly one slot, for anyone.
    let victim = order
        .get(random.below_len(order.len()))
        .copied()
        .ok_or("victim out of range")?;
    let gone = rows
        .get_mut(victim)
        .and_then(Vec::pop)
        .ok_or_else(|| String::from("victim row is empty"))?;
    harness.broker().unregister(&gone);
    let latecomer = register(harness, "widget.new", &newcomer, SESSION_SHARED)?;
    ensure(
        refused_widget(harness, &newcomer)? == BrokerError::LimitExceeded(Limit::Widgets),
        || String::from("the freed slot is spent; the bound binds again"),
    )?;
    trace.mix_len(victim);
    Ok(Fleet {
        order,
        rows,
        first,
        latecomer,
    })
}

/// Subscriptions: per-tenant bound first, then the global bound, then a freed
/// slot admits again. Returns the owner whose tenant filled first.
fn subscription_phase(
    harness: &Harness,
    fleet: &Fleet,
    trace: &mut TraceHash,
) -> Result<RegisteredHandle, String> {
    let sub_cap = Limit::SubscriptionsPerTenant.cap();
    let mut held: Vec<(RegisteredHandle, SubscriptionId)> = Vec::new();
    let owner = owner_of(&fleet.rows, fleet.first)?;
    saturate_subscriptions(harness, &owner, &mut held, sub_cap)?;
    ensure(
        harness.broker().subscribe(&owner, everything(), quiet())
            == Err(BrokerError::LimitExceeded(Limit::SubscriptionsPerTenant)),
        || String::from("subscription tenant bound must refuse"),
    )?;
    for &index in fleet.order.iter().skip(1) {
        let other = owner_of(&fleet.rows, index)?;
        saturate_subscriptions(harness, &other, &mut held, sub_cap)?;
    }
    ensure(
        harness
            .broker()
            .subscribe(&fleet.latecomer, everything(), quiet())
            == Err(BrokerError::LimitExceeded(Limit::Subscriptions)),
        || String::from("global subscription bound must refuse"),
    )?;
    let (freed_owner, freed) = held
        .pop()
        .ok_or_else(|| String::from("no subscription held"))?;
    harness
        .broker()
        .unsubscribe(&freed_owner, &freed)
        .map_err(|error| format!("{error}"))?;
    harness
        .broker()
        .subscribe(&fleet.latecomer, everything(), quiet())
        .map_err(|error| format!("a freed subscription slot must admit: {error}"))?;
    trace.mix_len(held.len());
    Ok(owner)
}

/// Taps: per-tenant 8, global 64.
fn tap_phase(harness: &Harness, fleet: &Fleet, owner: &RegisteredHandle) -> Result<(), String> {
    for _ in 0..Limit::TapsPerTenant.cap() {
        harness
            .broker()
            .tap(owner, Reach::Own, noop_tap())
            .map_err(|error| format!("{error}"))?;
    }
    ensure(
        harness.broker().tap(owner, Reach::Own, noop_tap())
            == Err(BrokerError::LimitExceeded(Limit::TapsPerTenant)),
        || String::from("tap tenant bound must refuse"),
    )?;
    for &index in fleet.order.iter().skip(1) {
        let other = owner_of(&fleet.rows, index)?;
        for _ in 0..Limit::TapsPerTenant.cap() {
            harness
                .broker()
                .tap(&other, Reach::Own, noop_tap())
                .map_err(|error| format!("{error}"))?;
        }
    }
    ensure(
        harness
            .broker()
            .tap(&fleet.latecomer, Reach::Own, noop_tap())
            == Err(BrokerError::LimitExceeded(Limit::Taps)),
        || String::from("global tap bound must refuse"),
    )
}

/// ML peers: the tenant bound refuses first at 8, the global bound at 64, and
/// a revoke frees a slot.
fn ml_phase(harness: &Harness) -> Result<(), String> {
    let per_tenant = Limit::MlPeersPerTenant.cap();
    let ml_session = |index: usize| match index.checked_div(per_tenant) {
        Some(owner) => session(&tenant_name(owner), SESSION_ONE),
        None => Err(String::from("no ML tenant bound")),
    };
    let mut peers = Vec::new();
    for index in 0..Limit::MlPeers.cap() {
        if index == per_tenant {
            ensure(
                harness
                    .broker()
                    .register_ml_peer(ml_grant(8_888)?, ml_session(0)?)
                    == Err(BrokerError::LimitExceeded(Limit::MlPeersPerTenant)),
                || String::from("ML tenant bound must refuse"),
            )?;
        }
        let peer = harness
            .broker()
            .register_ml_peer(ml_grant(index)?, ml_session(index)?)
            .map_err(|error| format!("{error}"))?;
        peers.push(peer);
    }
    ensure(
        harness
            .broker()
            .register_ml_peer(ml_grant(9_999)?, ml_session(0)?)
            == Err(BrokerError::LimitExceeded(Limit::MlPeers)),
        || String::from("ML bound must refuse"),
    )?;
    if let Some(peer) = peers.pop() {
        harness.broker().unregister_ml_peer(&peer);
    }
    let last = Limit::MlPeers.cap().saturating_sub(1);
    harness
        .broker()
        .register_ml_peer(ml_grant(10_000)?, ml_session(last)?)
        .map_err(|error| format!("a freed ML slot must admit: {error}"))?;
    Ok(())
}

/// Fill the widget, subscription, tap, and ML registries past their bounds in
/// seeded tenant order; each refusal is exact, others are unaffected, and a
/// revoke frees the slot.
fn run_limits(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let fleet = widget_phase(&harness, &mut random, &mut trace)?;
    let owner = subscription_phase(&harness, &fleet, &mut trace)?;
    tap_phase(&harness, &fleet, &owner)?;
    ml_phase(&harness)?;
    // Every refusal was audited exactly once as a typed limit denial.
    let denials = harness
        .audit_events()
        .iter()
        .filter(|event| {
            matches!(
                event.decision(),
                Decision::Denied(BrokerError::LimitExceeded(_))
            )
        })
        .count();
    ensure(denials == 10, || {
        format!("ten refusals must audit, got {denials}")
    })?;
    trace.mix_len(denials);
    Ok(trace.finish())
}

/// Giant-and-empty. Bug caught: a registry with no bound, an off-by-one at
/// the bound, a tenant bound reported before the global one, or a revoke that
/// does not free its slot; each of the ten refusals is also audited once.
#[test]
fn sim_broker_giant_and_empty_every_bound_refuses_at_limit_plus_one() -> Result<(), String> {
    replays(run_limits, [7, 53, 1_009], 8)
}

/// Seeded grant flips between attempts: each attempt follows the store as it
/// stands, including on the retry path and when the slot is already held.
fn run_grant_flips(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut granted = true;
    let mut held = None;
    for _ in 0..120 {
        if random.below(3) == 0 {
            granted = !granted;
            if granted {
                harness
                    .grants()
                    .grant(None, Permission::Capability(Capability::IntentEmit));
            } else {
                harness.grants().clear();
            }
        }
        let attempt = harness
            .broker()
            .register_command_bar(session("tenant", SESSION_ONE)?);
        match (granted, held.is_some(), attempt) {
            (false, _, Err(BrokerError::CapabilityDenied(_))) => trace.mix_u64(1),
            (true, false, Ok(handle)) => {
                held = Some(handle);
                trace.mix_u64(2);
            }
            (true, true, Err(BrokerError::CommandBarAlreadyRegistered)) => {
                if random.below(2) == 0
                    && let Some(handle) = held.take()
                {
                    harness.broker().unregister_command_bar(&handle);
                }
                trace.mix_u64(3);
            }
            (state, slot, outcome) => ensure(false, || {
                format!("granted {state} held {slot} met {outcome:?}")
            })?,
        }
    }
    Ok(trace.finish())
}

/// Liar-upstream. Bug caught: a grant cached across calls, a capability
/// check that runs after the slot check, or a denial that frees or holds the
/// command-bar slot (issue drishti#9).
#[test]
fn sim_broker_liar_upstream_grant_store_flips_decide_every_attempt() -> Result<(), String> {
    replays(run_grant_flips, [17, 83, 619], 18)
}

/// A hook that calls back into the broker once per closure: a nested call
/// while busy does nothing, so the chain of events the re-entry causes ends.
fn reenter_once(
    busy: &Arc<AtomicBool>,
    slot: &Arc<OnceLock<Weak<Broker>>>,
    probe: &RegisteredHandle,
) -> impl Fn() + Send + Sync + 'static + use<> {
    let (busy, slot, probe) = (Arc::clone(busy), Arc::clone(slot), probe.clone());
    move || {
        if busy.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(broker) = slot.get().and_then(Weak::upgrade) {
            broker.unregister(&probe);
        }
        busy.store(false, Ordering::SeqCst);
    }
}

/// Re-entrant clock, id, grant, and sink closures call back into the broker
/// during seeded operations; nothing deadlocks and the stream stays ordered.
fn run_reentry(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let slot: Arc<OnceLock<Weak<Broker>>> = Arc::new(OnceLock::new());
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let counter = Arc::new(Mutex::new(0_u128));
    let probe = RegisteredHandle::new(
        Uuid::from(0xbad_u128.to_le_bytes()),
        String::from(SESSION_ONE),
        String::from("widget.ghost"),
        String::from("tenant"),
        String::from(crate::fixtures::USER_SUB),
    );
    let flag = || Arc::new(AtomicBool::new(false));
    let hook = || reenter_once(&flag(), &slot, &probe);
    let (clock_hook, id_hook, grant_hook, sink_hook) = (hook(), hook(), hook(), hook());
    let clock: ClockFn = Arc::new(move || {
        clock_hook();
        1_000
    });
    let new_id: IdFn = {
        let counter = Arc::clone(&counter);
        Arc::new(move || {
            id_hook();
            let mut guard = lock_recover(&counter);
            *guard = guard.wrapping_add(1);
            Uuid::from(guard.to_le_bytes())
        })
    };
    let verify: VerifyGrantFn = Arc::new(|_| GrantVerificationResult::Valid);
    let grants: GrantFn = Arc::new(move |_, _| {
        grant_hook();
        true
    });
    let audit: AuditFn = {
        let events = Arc::clone(&events);
        Arc::new(move |event| {
            lock_recover(&events).push(format!("{}:{:?}", event.seq(), event.action()));
            sink_hook();
        })
    };
    let broker = Arc::new(Broker::new(clock, new_id, verify, grants, audit));
    slot.set(Arc::downgrade(&broker))
        .map_err(|rejected| format!("slot already set, refused {rejected:?}"))?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut widgets: Vec<RegisteredHandle> = Vec::new();
    for step in 0..80_u64 {
        let pick = widgets.get(random.below_len(widgets.len())).cloned();
        match (random.below(5), pick) {
            (1, Some(handle)) => {
                broker
                    .subscribe(&handle, everything(), quiet())
                    .map_err(|error| format!("{error}"))?;
            }
            (2, Some(handle)) => {
                broker
                    .emit(&handle, "tick", JsonValue::Null)
                    .map_err(|error| format!("{error}"))?;
            }
            (3, Some(handle)) => {
                broker
                    .registered_subscriptions(&handle, Reach::AllTenants)
                    .map_err(|error| format!("{error}"))?;
            }
            (4, _) => match broker.register_command_bar(session("tenant", SESSION_ONE)?) {
                Ok(bar) => {
                    broker.unregister_command_bar(&bar);
                    trace.mix_u64(8);
                }
                Err(error) => {
                    lgwks_std::trace::debug!(%error, "command bar refused during re-entry");
                    trace.mix_u64(7);
                }
            },
            _ => widgets.push(
                broker
                    .register(
                        widget(
                            &format!("widget.r{step}"),
                            vec![Capability::SignalEmit],
                            false,
                        ),
                        session("tenant", SESSION_ONE)?,
                    )
                    .map_err(|error| format!("{error}"))?,
            ),
        }
    }
    let log = lock_recover(&events).clone();
    for (index, line) in log.iter().enumerate() {
        ensure(line.starts_with(&format!("{index}:")), || {
            String::from("one thread's events must arrive in seq order")
        })?;
        trace.mix_str(line);
    }
    Ok(trace.finish())
}

/// Saboteur. Bug caught: a clock, id, grant, or sink closure called under the
/// state lock (deadlock), or events reaching the sink out of ordinal order.
#[test]
fn sim_broker_saboteur_reentrant_closures_never_deadlock() -> Result<(), String> {
    replays(run_reentry, [29, 131, 997], 30)
}

/// The same widget id and the same subscription pressed 100 times, then
/// 10,000: exactly one registration and one bound's worth of subscriptions
/// are admitted, every press is audited, and the registry never exceeds its
/// bound.
fn run_double_click(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let tenant = if random.below(2) == 0 {
        "tenant-a"
    } else {
        "tenant-b"
    };
    let owner = register(&harness, "widget.click", tenant, SESSION_SHARED)?;
    let mut presses = 0_usize;
    for round in [100_usize, 10_000] {
        for _ in 0..round {
            let again = harness.broker().register(
                widget("widget.click", vec![Capability::SignalEmit], false),
                session(tenant, SESSION_SHARED)?,
            );
            ensure(
                again == Err(BrokerError::DuplicateSourceId(String::from("widget.click"))),
                || String::from("a repeated registration must be a duplicate"),
            )?;
            presses = presses.saturating_add(1);
        }
        let mut admitted = Vec::new();
        for _ in 0..round {
            match harness.broker().subscribe(&owner, everything(), quiet()) {
                Ok(id) => admitted.push(id),
                Err(BrokerError::LimitExceeded(Limit::SubscriptionsPerTenant)) => {}
                Err(other) => ensure(false, || format!("subscribe: {other:?}"))?,
            }
            presses = presses.saturating_add(1);
        }
        let cap = Limit::SubscriptionsPerTenant.cap();
        ensure(admitted.len() == round.min(cap), || {
            format!("admitted {} of {round}", admitted.len())
        })?;
        let held = harness
            .broker()
            .registered_subscriptions(&owner, Reach::Own)
            .map_err(|error| format!("{error}"))?;
        ensure(held.len() <= cap, || {
            String::from("the registry outgrew its bound")
        })?;
        for id in &admitted {
            harness
                .broker()
                .unsubscribe(&owner, id)
                .map_err(|error| format!("first revoke failed: {error}"))?;
            ensure(
                harness.broker().unsubscribe(&owner, id) == Err(BrokerError::InvalidHandle),
                || String::from("a second revoke must be refused"),
            )?;
        }
        trace.mix_len(admitted.len());
    }
    // Registration, every duplicate, every subscribe attempt, one read per
    // round, and per admitted subscription one revoke and its refused repeat.
    let events = harness.audit_events().len();
    let revokes = Limit::SubscriptionsPerTenant.cap().saturating_add(100);
    let repeated = revokes.saturating_mul(2);
    let expected = presses
        .saturating_add(1)
        .saturating_add(2)
        .saturating_add(repeated);
    ensure(events == expected, || {
        format!("{events} events, expected {expected}")
    })?;
    trace.mix_len(events);
    Ok(trace.finish())
}

/// Double-clicker. Bug caught: a repeated press that registers twice, a
/// subscription list that grows without bound, a second revoke that succeeds,
/// or a press that escapes the audit stream.
#[test]
fn sim_broker_double_clicker_repeats_are_idempotent_bounded_and_audited() -> Result<(), String> {
    replays(run_double_click, [61, 251, 1_013], 62)
}
