//! Seeded broker storm: subscriptions churn while seeded widgets emit,
//! with rate-limit exhaustion and forged handles injected as faults.
//!
//! Same seed replays the same trace hash; the trace covers deliveries,
//! outcomes, audit counts, and subscription census.

use std::sync::{Arc, Mutex};

use drishti::canvas_protocol::Capability;
use drishti::native_host::broker::{BrokerError, GrantVerificationResult, Reach, SignalListener};
use drishti::native_host::protocol::{JsonValue, SignalPattern};
use lgwks_std::id::Uuid;

use crate::broker_shared::{
    Harness, SESSION_ONE, SESSION_SHARED, SESSION_TWO, lock_recover, lock_vec, register,
    register_listen, session, subscribe_all, widget,
};
use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// Forged widget handle that the broker must refuse.
fn forged_handle() -> drishti::native_host::broker::RegisteredHandle {
    drishti::native_host::broker::RegisteredHandle::new(
        Uuid::from(0xbeef_u128.to_le_bytes()),
        String::from("session-1"),
        String::from("widget.ghost"),
        String::from("tenant-a"),
        String::from("user"),
    )
}

/// Pick one storm target: a live handle plus a kind from the table.
fn pick_target<'handles, 'kinds>(
    random: &mut SimRng,
    handles: &'handles [drishti::native_host::broker::RegisteredHandle],
    kinds: &'kinds [&str],
    index_error: &str,
) -> Result<
    (
        &'handles drishti::native_host::broker::RegisteredHandle,
        &'kinds str,
    ),
    String,
> {
    let position = random.below_len(handles.len());
    let handle = handles
        .get(position)
        .ok_or_else(|| String::from(index_error))?;
    let kind = random
        .pick_str(kinds)
        .ok_or_else(|| String::from("kinds must not be empty"))?;
    Ok((handle, kind))
}

/// Seeded tenant pick and session pick for one registration.
fn pick_scope(
    random: &mut SimRng,
    tenants: &[&'static str],
    sessions: &[&'static str],
) -> Result<(&'static str, &'static str), String> {
    ensure(!tenants.is_empty(), || {
        String::from("tenants must not be empty")
    })?;
    ensure(!sessions.is_empty(), || {
        String::from("sessions must not be empty")
    })?;
    Ok((
        tenants[random.below_len(tenants.len())],
        sessions[random.below_len(sessions.len())],
    ))
}

/// Run one storm from `seed`: register widgets across tenants and sessions,
/// churn subscriptions, emit with faults, and hash the trace.
fn run_storm(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::valid();
    let broker = harness.broker();
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);

    let tenants = ["tenant-a", "tenant-b", "tenant-c"];
    let sessions = [crate::fixtures::SESSION_ID, SESSION_ONE];
    let globs = ["**", "ping", "pong.*", "tick", "forms.**", "evt.?.x"];
    let mut handles = Vec::new();
    for index in 0..6 {
        let (tenant, session_id) = pick_scope(&mut random, &tenants, &sessions)?;
        let descriptor = widget(
            &format!("widget.storm-{index}"),
            vec![Capability::SignalEmit, Capability::SignalSubscribe],
            random.below(2) == 1,
        );
        let handle = broker
            .register(descriptor, session(tenant, session_id)?)
            .map_err(|error| format!("register failed: {error}"))?;
        trace.mix_str(handle.descriptor_id());
        handles.push(handle);
    }

    let log = Arc::new(Mutex::new(Vec::new()));
    let mut subscription_ids = Vec::new();
    for _ in 0..12 {
        let position = random.below_len(handles.len());
        let handle = handles
            .get(position)
            .ok_or_else(|| String::from("handle index out of range"))?;
        let glob = random
            .pick_str(&globs)
            .ok_or_else(|| String::from("globs must not be empty"))?;
        let listener: SignalListener = {
            let log = Arc::clone(&log);
            Arc::new(
                move |envelope: drishti::native_host::protocol::SignalEnvelope| {
                    lock_recover(&log).push(format!(
                        "{}:{}",
                        envelope.source_id(),
                        envelope.kind()
                    ));
                },
            )
        };
        if let Ok(id) = broker.subscribe(
            handle,
            SignalPattern::new(String::from(glob), None),
            listener,
        ) {
            subscription_ids.push((handle, id));
        }
    }
    trace.mix_len(subscription_ids.len());

    let kinds = ["ping", "pong.a", "", "tick", "evt.q.x", "bad..kind"];
    for step in 0..400 {
        // Faults: jump the clock, forge a handle, unsubscribe mid-storm.
        if step == 150 {
            harness.time().set(5_000);
        }
        if step == 200 {
            let forged = forged_handle();
            trace.mix_bool(
                broker.emit(&forged, "ping", JsonValue::Null) == Err(BrokerError::InvalidHandle),
            );
        }
        if step == 250 {
            for (owner, id) in subscription_ids.drain(..).take(4) {
                trace.mix_bool(broker.unsubscribe(owner, &id).is_ok());
            }
        }
        let (handle, kind) = pick_target(&mut random, &handles, &kinds, "emit index out of range")?;
        match broker.emit(handle, kind, JsonValue::Number(1.0)) {
            Ok(envelope) => {
                trace.mix_str(envelope.kind());
                trace.mix_u64(1);
            }
            Err(BrokerError::RateLimited) => trace.mix_u64(2),
            Err(BrokerError::MalformedKind) => trace.mix_u64(3),
            Err(error) => ensure(false, || format!("unexpected storm outcome: {error}"))?,
        }
    }
    let deliveries = lock_recover(&log).clone();
    trace.mix_len(deliveries.len());
    for delivery in deliveries.iter().take(64) {
        trace.mix_str(delivery);
    }
    let audits = harness.audited().len();
    trace.mix_u64(u64::try_from(audits).map_err(|error| format!("audit overflow: {error}"))?);
    for handle in &handles {
        let seen = broker
            .registered_subscriptions(handle, Reach::Own)
            .map_err(|error| format!("inspect failed: {error}"))?;
        ensure(
            seen.iter().all(|subscription| {
                subscription.tenant_id() == handle.tenant_id()
                    && subscription.session_id() == handle.session_id()
            }),
            || String::from("an inspector listed a foreign subscription"),
        )?;
        trace.mix_len(seen.len());
    }
    Ok(trace.finish())
}

/// Same seed replays the same hash with identical delivery prefixes.
#[test]
fn sim_broker_storm_replays_identically() -> Result<(), String> {
    for seed in [1_u64, 7, 99, 4_042] {
        let first = run_storm(seed)?;
        let second = run_storm(seed)?;
        if first != second {
            return Err(format!("seed {seed} diverged: {first} vs {second}"));
        }
    }
    // Distinct seeds diverge (the storm is seed-sensitive).
    if run_storm(1)? == run_storm(2)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}

/// Run one tenant-isolation campaign from `seed` on the shared harness:
/// widgets register across tenants and sessions, every handle subscribes
/// catch-all, forged handles and bad grants are refused, and deliveries never
/// cross tenant or session lines.
fn run_isolation(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::valid();
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);

    let tenants = ["tenant-a", "tenant-b"];
    let sessions = [
        crate::fixtures::SESSION_ID,
        SESSION_ONE,
        SESSION_TWO,
        SESSION_SHARED,
    ];
    let mut handles = Vec::new();
    for index in 0..6 {
        let (tenant, session_id) = pick_scope(&mut random, &tenants, &sessions)?;
        let handle = if random.below(2) == 0 {
            register(&harness, &format!("widget.iso-{index}"), tenant, session_id)?
        } else {
            register_listen(&harness, &format!("widget.iso-{index}"), tenant, session_id)?
        };
        trace.mix_str(handle.descriptor_id());
        handles.push(handle);
    }

    // Every handle subscribes catch-all.
    let mut logs = Vec::new();
    for handle in &handles {
        logs.push(subscribe_all(&harness, handle)?);
    }
    trace.mix_len(logs.len());

    // Faults: a forged handle emits nothing, and a bad grant registers no peer.
    let forged = forged_handle();
    trace.mix_bool(
        harness.broker().emit(&forged, "ping", JsonValue::Null) == Err(BrokerError::InvalidHandle),
    );
    let denied = Harness::with_verdict(GrantVerificationResult::InvalidSignature);
    let grant = drishti::native_host::broker::SignedMlGrant::new(
        Uuid::from(9_u128.to_le_bytes()),
        String::from("tenant-a"),
        vec![Capability::TapeRead],
        9_999_999,
        0,
        Vec::new(),
    );
    trace.mix_bool(
        denied
            .broker()
            .register_ml_peer(grant, session("tenant-a", crate::fixtures::SESSION_ID)?)
            == Err(BrokerError::MlGrantInvalid(
                GrantVerificationResult::InvalidSignature,
            )),
    );

    // Seeded emits across the handles; every kind here is valid.
    let kinds = ["ping", "tick", "pong.a"];
    for _ in 0..200 {
        let (handle, kind) =
            pick_target(&mut random, &handles, &kinds, "handle index out of range")?;
        match harness.broker().emit(handle, kind, JsonValue::Null) {
            Ok(envelope) => {
                trace.mix_str(envelope.kind());
                trace.mix_u64(1);
            }
            Err(BrokerError::RateLimited) => trace.mix_u64(2),
            Err(error) => ensure(false, || format!("unexpected isolation outcome: {error}"))?,
        }
    }

    // Isolation: every delivery carries its own subscriber session and tenant.
    for (handle, log) in handles.iter().zip(logs.iter()) {
        for envelope in lock_vec(log).iter() {
            ensure(
                envelope.session_invariant().session_id() == handle.session_id()
                    && envelope.session_invariant().tenant_id() == handle.tenant_id(),
                || String::from("delivery crossed session or tenant lines"),
            )?;
        }
        trace.mix_len(lock_vec(log).len());
    }
    // Every routed emit reaches the audit sink exactly once.
    ensure(harness.audited().len() == 200, || {
        String::from("audit must record every emit")
    })?;
    trace.mix_len(harness.audited().len());
    Ok(trace.finish())
}

/// Same seed replays the same isolation hash with identical delivery sets.
#[test]
fn sim_broker_tenant_isolation_replays_identically() -> Result<(), String> {
    for seed in [11_u64, 43, 901] {
        if run_isolation(seed)? != run_isolation(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_isolation(11)? == run_isolation(12)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}

/// Run one rate-limit campaign from `seed` on the shared harness: a frozen
/// 250-burst limits exactly 50, the clock refill re-admits 100, and every
/// outcome is allow or limit.
fn run_limits(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let harness = Harness::valid();
    let tenant = random
        .pick_str(&["tenant-a", "tenant-b"])
        .ok_or_else(|| String::from("tenants must not be empty"))?;
    let handle = register(
        &harness,
        "widget.limits",
        tenant,
        crate::fixtures::SESSION_ID,
    )?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut limited = 0_usize;
    for _ in 0..250 {
        match harness.broker().emit(&handle, "tick", JsonValue::Null) {
            Ok(_) => trace.mix_u64(1),
            Err(BrokerError::RateLimited) => {
                limited = limited.saturating_add(1);
                trace.mix_u64(2);
            }
            Err(error) => ensure(false, || format!("unexpected limit outcome: {error}"))?,
        }
    }
    ensure(limited == 50, || {
        format!("250 frozen emits must limit 50, limited {limited}")
    })?;
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
    ensure(refilled == 100, || {
        format!("one second must refill 100, refilled {refilled}")
    })?;
    trace.mix_len(limited);
    trace.mix_len(refilled);
    Ok(trace.finish())
}

/// Same seed replays the same limit hash with identical allow/limit counts.
#[test]
fn sim_broker_rate_limit_replays_identically() -> Result<(), String> {
    for seed in [13_u64, 47, 1_201] {
        if run_limits(seed)? != run_limits(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_limits(13)? == run_limits(14)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}
