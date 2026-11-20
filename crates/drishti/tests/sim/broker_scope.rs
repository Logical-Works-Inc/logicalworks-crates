//! Seeded broker scope, audit, tenant-key, and revocation campaigns
//! (issues drishti#8 to drishti#10).
//!
//! Each campaign is driven by one seed and judged by an independent model,
//! never by the broker's own output. Every run is replayed: the same seed
//! gives the same trace hash and a distinct seed diverges.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use drishti::canvas_protocol::Capability;
use drishti::native_host::broker::{
    AuditAction, BrokerError, Decision, Emitter, Permission, Reach, RegisteredHandle, Subject,
    SubscriptionId, TapEvent, TapListener,
};
use drishti::native_host::protocol::{JsonValue, SignalEnvelope, SignalPattern};
use lgwks_std::id::Uuid;

use crate::broker_shared::{
    Harness, SESSION_ONE, SESSION_SHARED, collector, lock_recover, lock_vec, register,
    register_listen, session, widget,
};
use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// Replay a campaign for each seed and require a distinct seed to diverge.
pub(crate) fn replays(
    run: fn(u64) -> Result<u64, String>,
    seeds: [u64; 3],
    other: u64,
) -> Result<(), String> {
    for seed in seeds {
        ensure(run(seed)? == run(seed)?, || format!("seed {seed} diverged"))?;
    }
    ensure(run(seeds[0])? != run(other)?, || {
        String::from("distinct seeds must diverge")
    })
}

/// Identities the caller's inspector read returned.
pub(crate) fn ids_of(
    harness: &Harness,
    caller: &RegisteredHandle,
    reach: Reach,
) -> Result<BTreeSet<SubscriptionId>, BrokerError> {
    harness
        .broker()
        .registered_subscriptions(caller, reach)
        .map(|listed| listed.iter().map(|sub| sub.id()).collect())
}

/// Subscribe a catch-all listener that logs nothing.
fn subscribe_quiet(harness: &Harness, handle: &RegisteredHandle) -> Result<SubscriptionId, String> {
    harness
        .broker()
        .subscribe(
            handle,
            SignalPattern::new(String::from("**"), None),
            collector(&Arc::new(Mutex::new(Vec::new()))),
        )
        .map_err(|error| format!("subscribe failed: {error}"))
}

/// Three widgets per tenant in the shared session, with the same widget ids
/// in every tenant (`listen` makes each hear its own emits).
fn tenant_rows(
    harness: &Harness,
    tenants: &[&str],
    prefix: &str,
    listen: bool,
) -> Result<Vec<Vec<RegisteredHandle>>, String> {
    let mut rows = Vec::new();
    for tenant in tenants {
        let mut row = Vec::new();
        for index in 0..3 {
            let id = format!("{prefix}-{index}");
            row.push(if listen {
                register_listen(harness, &id, tenant, SESSION_SHARED)?
            } else {
                register(harness, &id, tenant, SESSION_SHARED)?
            });
        }
        rows.push(row);
    }
    Ok(rows)
}

/// Two tenants share one session id, widget ids, and glob; seeded churn runs
/// against a model of which subscriptions each tenant owns.
fn run_two_tenants(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let tenants = ["tenant-a", "tenant-b"];
    let handles = tenant_rows(&harness, &tenants, "widget", true)?;
    let logs: Vec<Arc<Mutex<Vec<SignalEnvelope>>>> = tenants
        .iter()
        .map(|_| Arc::new(Mutex::new(Vec::new())))
        .collect();
    let mut model: Vec<Vec<(usize, SubscriptionId)>> = vec![Vec::new(), Vec::new()];
    for _ in 0..240 {
        let side = random.below_len(2);
        let slot = random.below_len(3);
        let row = handles
            .get(side)
            .ok_or_else(|| String::from("side out of range"))?;
        let log = logs
            .get(side)
            .ok_or_else(|| String::from("side out of range"))?;
        let owned = model
            .get_mut(side)
            .ok_or_else(|| String::from("side out of range"))?;
        let handle = row
            .get(slot)
            .ok_or_else(|| String::from("slot out of range"))?;
        match random.below(4) {
            0 => {
                let pattern = SignalPattern::new(String::from("**"), None);
                let id = harness
                    .broker()
                    .subscribe(handle, pattern, collector(log))
                    .map_err(|error| format!("subscribe failed: {error}"))?;
                owned.push((slot, id));
                trace.mix_u64(1);
            }
            1 => {
                let before = lock_vec(log).len();
                harness
                    .broker()
                    .emit(handle, "ping", JsonValue::Null)
                    .map_err(|error| format!("emit failed: {error}"))?;
                let gained = lock_vec(log).len().saturating_sub(before);
                ensure(gained == owned.len(), || {
                    format!("delivered {gained}, owned {}", owned.len())
                })?;
                trace.mix_len(gained);
            }
            2 => {
                if let Some((owner, id)) = owned.pop() {
                    let owner_handle = row
                        .get(owner)
                        .ok_or_else(|| String::from("owner out of range"))?;
                    harness
                        .broker()
                        .unsubscribe(owner_handle, &id)
                        .map_err(|error| format!("owner unsubscribe failed: {error}"))?;
                    trace.mix_u64(2);
                }
            }
            _ => {
                let seen = ids_of(&harness, handle, Reach::Own)
                    .map_err(|error| format!("own read failed: {error}"))?;
                let wanted: BTreeSet<SubscriptionId> = owned.iter().map(|held| held.1).collect();
                ensure(seen == wanted, || {
                    String::from("own read must equal the tenant's model")
                })?;
                ensure(
                    ids_of(&harness, handle, Reach::AllTenants)
                        == Err(BrokerError::CrossTenantDenied),
                    || String::from("ungranted wide read must refuse"),
                )?;
                trace.mix_len(seen.len());
            }
        }
    }
    for (tenant, log) in tenants.iter().zip(logs.iter()) {
        for envelope in lock_vec(log).iter() {
            ensure(envelope.session_invariant().tenant_id() == *tenant, || {
                String::from("a delivery crossed the tenant line")
            })?;
        }
        trace.mix_len(lock_vec(log).len());
    }
    Ok(trace.finish())
}

/// Neighbour. Bug caught: a delivery or inspector read that joins records on
/// the session id alone (issue drishti#8), so tenant B sees tenant A's subscriptions.
#[test]
fn sim_broker_neighbour_same_session_never_reads_across_tenants() -> Result<(), String> {
    replays(run_two_tenants, [3, 29, 811], 4)
}

/// Seeded decisions judged by a model that knows how many events each one
/// produces, their order, and the injected clock at the time.
fn run_audit_stream(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut widgets = vec![register(&harness, "widget.base", "tenant", SESSION_ONE)?];
    let mut bar = None;
    // The setup registration above is the first decision of the stream.
    let mut sequence = u64::try_from(harness.audit_events().len())
        .map_err(|error| format!("event count overflow: {error}"))?;
    ensure(sequence == 1, || {
        String::from("the setup registration must audit once")
    })?;
    for step in 0..150_u64 {
        // The clock fool: the instant jumps anywhere, backwards included, and
        // repeats; the sink must still see the injected value and a contiguous
        // ordinal.
        let now = random.below(50_000).saturating_add(step % 3);
        harness.time().set(now);
        let before = harness.audit_events().len();
        let expected = 1_usize;
        let target = widgets
            .get(random.below_len(widgets.len()))
            .cloned()
            .ok_or_else(|| String::from("widget index out of range"))?;
        match random.below(8) {
            0 => widgets.push(register(
                &harness,
                &format!("widget.s{step}"),
                "tenant",
                SESSION_ONE,
            )?),
            1 => {
                let again = harness.broker().register(
                    widget(target.descriptor_id(), vec![Capability::SignalEmit], false),
                    session("tenant", SESSION_ONE)?,
                );
                ensure(
                    matches!(again, Err(BrokerError::DuplicateSourceId(_))),
                    || String::from("duplicate id must be refused"),
                )?;
            }
            2 => {
                subscribe_quiet(&harness, &target)?;
            }
            3 => {
                // Admitted or refused (a rate limit), an emit audits once.
                let emitted = harness.broker().emit(&target, "tick", JsonValue::Null);
                trace.mix_bool(emitted.is_ok());
            }
            4 => {
                ids_of(&harness, &target, Reach::Own)
                    .map_err(|error| format!("own read failed: {error}"))?;
            }
            5 => {
                let wide_granted = random.below(2) == 0;
                if wide_granted {
                    harness
                        .grants()
                        .grant(Some("tenant"), Permission::CrossTenantInspect);
                } else {
                    harness.grants().clear();
                }
                let wide = ids_of(&harness, &target, Reach::AllTenants);
                ensure(wide.is_ok() == wide_granted, || {
                    String::from("wide read must follow the store")
                })?;
                trace.mix_bool(wide_granted);
            }
            6 => match bar.take() {
                Some(held) => harness.broker().unregister_command_bar(&held),
                None => {
                    harness
                        .grants()
                        .grant(None, Permission::Capability(Capability::IntentEmit));
                    bar = Some(
                        harness
                            .broker()
                            .register_command_bar(session("tenant", SESSION_ONE)?)
                            .map_err(|error| format!("granted bar failed: {error}"))?,
                    );
                }
            },
            _ => {
                harness.grants().clear();
                ensure(
                    harness
                        .broker()
                        .register_command_bar(session("tenant", SESSION_ONE)?)
                        .is_err(),
                    || String::from("ungranted bar must be refused"),
                )?;
            }
        }
        ensure(
            harness.audit_events().len().saturating_sub(before) == expected,
            || format!("step {step} must audit {expected} event(s)"),
        )?;
        check_new_events(&harness, before, now, &mut sequence, &mut trace)?;
    }
    Ok(trace.finish())
}

/// Judge the events a step added after the first `before`: contiguous
/// ordinals, the injected clock, and a denial that names what it lacked.
fn check_new_events(
    harness: &Harness,
    before: usize,
    now: u64,
    sequence: &mut u64,
    trace: &mut TraceHash,
) -> Result<(), String> {
    for event in harness.audit_events().iter().skip(before) {
        ensure(event.seq() == *sequence && event.at_ms() == now, || {
            String::from("seq must be contiguous and clocked")
        })?;
        *sequence = sequence.saturating_add(1);
        let lacked = match *event.decision() {
            Decision::Denied(BrokerError::CrossTenantDenied) => {
                (event.reach() == Reach::AllTenants).then_some(Permission::CrossTenantInspect)
            }
            Decision::Denied(BrokerError::CapabilityDenied(_)) => (event.action()
                == &AuditAction::Register(Subject::CommandBar)
                && event.caller().user_sub() == crate::fixtures::USER_SUB)
                .then_some(Permission::Capability(Capability::IntentEmit)),
            _ => None,
        };
        ensure(lacked == event.decision().missing(), || {
            String::from("a denial must name the permission it lacked")
        })?;
        trace.mix_str(&format!("{:?}{:?}", event.action(), event.decision()));
    }
    Ok(())
}

/// Saboteur. Bug caught: a decision that skips the audit sink, audits twice,
/// reorders, or stamps a clock other than the injected one (issue drishti#10).
#[test]
fn sim_broker_saboteur_every_decision_audited_once_in_order() -> Result<(), String> {
    replays(run_audit_stream, [5, 61, 977], 6)
}

/// A saboteur probes the inspectors with random, retired, and tenant-forged
/// handles; nothing leaks and every probe is audited once.
fn run_forged_probes(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let tenants = ["tenant-a", "tenant-b"];
    let mut real: Vec<RegisteredHandle> = Vec::new();
    let mut owned: Vec<BTreeSet<SubscriptionId>> = Vec::new();
    for tenant in tenants {
        let handle = register(&harness, "widget.real", tenant, SESSION_SHARED)?;
        let mut ids = BTreeSet::new();
        for _ in 0..3 {
            ids.insert(subscribe_quiet(&harness, &handle)?);
        }
        real.push(handle);
        owned.push(ids);
    }
    let retired = register(&harness, "widget.retired", "tenant-a", SESSION_SHARED)?;
    harness.broker().unregister(&retired);
    for step in 0..200_u64 {
        let side = random.below_len(2);
        let handle = real
            .get(side)
            .ok_or_else(|| String::from("side out of range"))?;
        let truth = owned
            .get(side)
            .ok_or_else(|| String::from("side out of range"))?;
        let claimed = tenants
            .get(1_usize.saturating_sub(side))
            .copied()
            .ok_or("tenant out of range")?;
        let forged = |nonce: Uuid| {
            RegisteredHandle::new(
                nonce,
                String::from(SESSION_SHARED),
                String::from("widget.real"),
                String::from(claimed),
                String::from(crate::fixtures::USER_SUB),
            )
        };
        let reach = if random.below(2) == 0 {
            Reach::Own
        } else {
            Reach::AllTenants
        };
        let before = harness.audit_events().len();
        let kind = random.below(3);
        let probe = match kind {
            0 => {
                let stray = u128::from(step).saturating_add(0xf00d);
                ids_of(&harness, &forged(Uuid::from(stray.to_le_bytes())), reach)
            }
            1 => ids_of(&harness, &retired, reach),
            _ => ids_of(&harness, &forged(handle.nonce()), reach),
        };
        let sound = match (kind, reach) {
            (2, Reach::Own) => probe.as_ref() == Ok(truth),
            (2, _) => probe == Err(BrokerError::CrossTenantDenied),
            _ => probe == Err(BrokerError::InvalidHandle),
        };
        ensure(sound, || {
            format!("probe {step} leaked or misclassified: {probe:?}")
        })?;
        trace.mix_u64(kind);
        ensure(
            harness.audit_events().len().saturating_sub(before) == 1,
            || String::from("every probe must audit exactly once"),
        )?;
    }
    Ok(trace.finish())
}

/// Wrong-order. Bug caught: an inspector that trusts the tenant a handle
/// claims, or serves a retired handle (issue drishti#8).
#[test]
fn sim_broker_wrong_order_forged_and_retired_handles_never_leak() -> Result<(), String> {
    replays(run_forged_probes, [13, 71, 503], 14)
}

/// Two tenants reuse widget ids; their rate buckets and id reservations are
/// separate. A model counts admitted emits per (tenant, widget).
fn run_tenant_keys(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let handles = tenant_rows(&harness, &["tenant-a", "tenant-b"], "widget.same", false)?;
    let mut admitted = [[0_usize; 3]; 2];
    for _ in 0..900 {
        let side = random.below_len(2);
        let slot = random.below_len(3);
        let handle = handles
            .get(side)
            .and_then(|row| row.get(slot))
            .ok_or_else(|| String::from("handle out of range"))?;
        let counter = admitted
            .get_mut(side)
            .and_then(|row| row.get_mut(slot))
            .ok_or_else(|| String::from("counter out of range"))?;
        let outcome = harness.broker().emit(handle, "tick", JsonValue::Null);
        match (outcome, *counter < 200) {
            (Ok(_), true) => *counter = counter.saturating_add(1),
            (Err(BrokerError::RateLimited), false) => {}
            (other, _) => ensure(false, || format!("bucket model broke: {other:?}"))?,
        }
        trace.mix_len(*counter);
    }
    // Revoking one tenant's widget leaves the other's id reserved.
    let gone = handles
        .first()
        .and_then(|row| row.first())
        .ok_or_else(|| String::from("no widget to revoke"))?;
    harness.broker().unregister(gone);
    let again = harness.broker().register(
        widget("widget.same-0", vec![Capability::SignalEmit], false),
        session("tenant-b", SESSION_SHARED)?,
    );
    ensure(
        again
            == Err(BrokerError::DuplicateSourceId(String::from(
                "widget.same-0",
            ))),
        || String::from("the other tenant's id must stay reserved"),
    )?;
    register(&harness, "widget.same-0", "tenant-a", SESSION_SHARED)?;
    Ok(trace.finish())
}

/// Neighbour. Bug caught: widget ids, id reservations, or rate buckets keyed
/// without the tenant, so one tenant spends or blocks another's.
#[test]
fn sim_broker_neighbour_widget_ids_and_buckets_stay_per_tenant() -> Result<(), String> {
    replays(run_tenant_keys, [19, 97, 733], 20)
}

/// Spellings that differ only in bytes a person cannot see.
const SPELLINGS: [&str; 6] = [
    "widget.w",
    "widget.w\u{0}",
    "widget.\u{202e}w",
    "widget.w\u{200b}",
    "widget\u{0}.w",
    "WIDGET.W",
];

/// Kinds the wire grammar must refuse before anything is stamped.
const BAD_KINDS: [&str; 5] = ["a\u{0}b", "a\nb", "", "a..b", ".a"];

/// Every spelling is its own identity inside a tenant and none leaks to the
/// neighbour; refused kinds leave no audit trace. A model of the identity set
/// judges the registry and the audit stream.
fn run_weird_bytes(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut order: Vec<usize> = (0..SPELLINGS.len()).collect();
    for index in 0..order.len() {
        let swap = random.below_len(order.len());
        order.swap(index, swap);
    }
    let mut firsts = Vec::new();
    for tenant in ["tenant-a", "tenant-b"] {
        let mut row = Vec::new();
        for &index in &order {
            let id = SPELLINGS.get(index).ok_or("spelling out of range")?;
            row.push(register(&harness, id, tenant, SESSION_SHARED)?);
            let again = harness.broker().register(
                widget(id, vec![Capability::SignalEmit], false),
                session(tenant, SESSION_SHARED)?,
            );
            ensure(
                again == Err(BrokerError::DuplicateSourceId(String::from(*id))),
                || format!("spelling {index} must be a stable identity"),
            )?;
        }
        for handle in &row {
            subscribe_quiet(&harness, handle)?;
        }
        firsts.push(row);
    }
    for row in &firsts {
        let caller = row.first().ok_or_else(|| String::from("empty row"))?;
        let seen = ids_of(&harness, caller, Reach::Own).map_err(|error| format!("{error}"))?;
        ensure(seen.len() == SPELLINGS.len(), || {
            String::from("each tenant must see exactly its own spellings")
        })?;
        trace.mix_len(seen.len());
        let before = harness.audit_events().len();
        for kind in BAD_KINDS {
            ensure(
                harness.broker().emit(caller, kind, JsonValue::Null)
                    == Err(BrokerError::MalformedKind),
                || format!("kind {kind:?} must be malformed"),
            )?;
        }
        // Each refused kind is one denial and leaves no hostile bytes in it.
        let refused: Vec<_> = harness.audit_events().into_iter().skip(before).collect();
        let named = refused.iter().all(|event| {
            event.action() == &AuditAction::Emit(Emitter::Widget)
                && event.decision() == &Decision::Denied(BrokerError::MalformedKind)
        });
        ensure(refused.len() == BAD_KINDS.len() && named, || {
            String::from("a refused kind must audit one named denial")
        })?;
    }
    let mut registered = BTreeSet::new();
    for event in harness.audit_events() {
        if let AuditAction::Register(Subject::Widget(ref id)) = *event.action()
            && event.decision() == &Decision::Allowed
        {
            registered.insert((event.caller().scope().tenant_id().to_owned(), id.clone()));
        }
    }
    ensure(
        registered.len() == SPELLINGS.len().saturating_mul(2),
        || String::from("the audit stream must name every identity once"),
    )?;
    trace.mix_len(registered.len());
    Ok(trace.finish())
}

/// Weird-bytes. Bug caught: ids normalized or truncated at a NUL or an RTL
/// mark so two spellings collide, or a malformed kind that is stamped and
/// audited anyway.
#[test]
fn sim_broker_weird_bytes_spellings_stay_distinct_identities() -> Result<(), String> {
    replays(run_weird_bytes, [31, 137, 613], 32)
}

/// Strangers, forged handles, and owners revoke subscriptions in seeded
/// order; only the owner ever succeeds and the registry matches the model.
fn run_revocation(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::seeded(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let owner = register(&harness, "widget.owner", "tenant-a", SESSION_SHARED)?;
    let stranger = register(&harness, "widget.owner", "tenant-b", SESSION_SHARED)?;
    let mut live: Vec<SubscriptionId> = Vec::new();
    for _ in 0..160 {
        match random.below(4) {
            0 => live.push(subscribe_quiet(&harness, &owner)?),
            1 => {
                if let Some(id) = live.last() {
                    ensure(
                        harness.broker().unsubscribe(&stranger, id)
                            == Err(BrokerError::InvalidHandle),
                        || String::from("a stranger must not unsubscribe"),
                    )?;
                }
            }
            2 => {
                if let Some(id) = live.pop() {
                    harness
                        .broker()
                        .unsubscribe(&owner, &id)
                        .map_err(|error| format!("owner unsubscribe failed: {error}"))?;
                    ensure(
                        harness.broker().unsubscribe(&owner, &id)
                            == Err(BrokerError::InvalidHandle),
                        || String::from("a second revoke must be refused"),
                    )?;
                }
            }
            _ => {
                let high = u128::from(random.next_u64()).wrapping_shl(64) | 1;
                let forged = RegisteredHandle::new(
                    Uuid::from(high.to_le_bytes()),
                    String::from(SESSION_SHARED),
                    String::from("widget.owner"),
                    String::from("tenant-a"),
                    String::from(crate::fixtures::USER_SUB),
                );
                harness.broker().unregister(&forged);
            }
        }
        trace.mix_len(live.len());
    }
    let seen = ids_of(&harness, &owner, Reach::Own).map_err(|error| format!("{error}"))?;
    ensure(
        seen == live.iter().copied().collect::<BTreeSet<_>>(),
        || String::from("only owner revocations may change the registry"),
    )?;
    ensure(
        harness
            .broker()
            .emit(&owner, "ping", JsonValue::Null)
            .is_ok(),
        || String::from("forged revokes must not retire the owner"),
    )?;
    // Taps follow the same rule: a stranger cannot cancel one, and the
    // owner's unregistration cancels it.
    let heard = Arc::new(Mutex::new(0_usize));
    let listener: TapListener = {
        let heard = Arc::clone(&heard);
        Arc::new(move |_: TapEvent| {
            let mut count = lock_recover(&heard);
            *count = count.saturating_add(1);
        })
    };
    let tap = harness
        .broker()
        .tap(&owner, Reach::Own, listener)
        .map_err(|error| format!("owner tap failed: {error}"))?;
    ensure(
        harness.broker().untap(&stranger, &tap) == Err(BrokerError::InvalidHandle),
        || String::from("a stranger must not untap"),
    )?;
    let sibling = register(&harness, "widget.sibling", "tenant-a", SESSION_SHARED)?;
    harness
        .broker()
        .emit(&sibling, "ping", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    harness.broker().unregister(&owner);
    harness
        .broker()
        .emit(&sibling, "ping", JsonValue::Null)
        .map_err(|error| format!("{error}"))?;
    ensure(
        *lock_recover(&heard) == 1 && harness.broker().untap(&owner, &tap).is_err(),
        || String::from("unregistering the owner must cancel its tap"),
    )?;
    Ok(trace.finish())
}

/// Wrong-order. Bug caught: `unsubscribe` or `unregister` that acts on a
/// forged or foreign handle, or a double revoke that succeeds twice.
#[test]
fn sim_broker_wrong_order_revocation_needs_the_owner() -> Result<(), String> {
    replays(run_revocation, [23, 101, 887], 24)
}
