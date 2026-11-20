//! Shared broker harness for the broker contract suites.
//!
//! One home for the frozen broker, its session and widget fixtures, and the
//! registration helpers both halves drive, so the halves cannot drift from
//! each other. Suites include this file with `#[path]`; it is never a test
//! target itself. Session identities come from the shared fixtures module.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard};

use drishti::canvas_protocol::{Capability, SessionInvariant};
use drishti::native_host::broker::{
    AuditAction, AuditEvent, AuditFn, Broker, BrokerError, ClockFn, Decision, GrantFn,
    GrantVerificationResult, IdFn, Inspector, Permission, Reach, RegisteredHandle, SignalListener,
    Subscription, TapEvent, TapId, TapListener, VerifyGrantFn,
};
use drishti::native_host::protocol::{JsonValue, SignalEnvelope, SignalPattern, WidgetDescriptor};
use lgwks_std::id::Uuid;

use crate::fixtures::ensure;

/// First isolation session fixture.
pub(crate) const SESSION_ONE: &str = "55555555-5555-5555-5555-555555555555";
/// Second isolation session fixture.
pub(crate) const SESSION_TWO: &str = "66666666-6666-6666-6666-666666666666";
/// Shared session fixture for tenant isolation.
pub(crate) const SESSION_SHARED: &str = "77777777-7777-7777-7777-777777777777";

/// Frozen millisecond clock shared by the broker under test.
#[derive(Debug, Default)]
pub(crate) struct FrozenTime {
    /// Current millisecond time.
    now_ms: Mutex<u64>,
}

impl FrozenTime {
    /// Read the frozen time.
    pub(crate) fn get(&self) -> u64 {
        match self.now_ms.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// Move the frozen time.
    pub(crate) fn set(&self, now_ms: u64) {
        match self.now_ms.lock() {
            Ok(mut guard) => *guard = now_ms,
            Err(poisoned) => *poisoned.into_inner() = now_ms,
        }
    }
}

/// Frozen identity counter shared by the broker under test.
#[derive(Debug, Default)]
pub(crate) struct FrozenIds {
    /// Next counter value.
    next: Mutex<u128>,
}

impl FrozenIds {
    /// A counter whose first identity follows `base`, so a seed picks the
    /// identity space a run mints from.
    pub(crate) fn starting_at(base: u128) -> Self {
        Self {
            next: Mutex::new(base),
        }
    }

    /// Mint the next deterministic UUID.
    pub(crate) fn mint(&self) -> Uuid {
        let id = match self.next.lock() {
            Ok(mut guard) => {
                *guard = guard.wrapping_add(1);
                *guard
            }
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                *guard = guard.wrapping_add(1);
                *guard
            }
        };
        Uuid::from(id.to_le_bytes())
    }
}

/// Lock a test mutex, recovering from poisoning by keeping the data.
pub(crate) fn lock_recover<T>(slot: &Arc<Mutex<T>>) -> MutexGuard<'_, T> {
    match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Lock a test envelope log, recovering from poisoning by keeping the data.
pub(crate) fn lock_vec(
    slot: &Arc<Mutex<Vec<SignalEnvelope>>>,
) -> MutexGuard<'_, Vec<SignalEnvelope>> {
    lock_recover(slot)
}

/// The injected grant store of the tests: a flippable list of
/// `(tenant, permission)` grants (`None` grants every tenant) that remembers
/// every question the broker asked it.
#[derive(Debug, Default)]
pub(crate) struct GrantBook {
    /// Current grants; `None` is every tenant.
    held: Mutex<Vec<(Option<String>, Permission)>>,
    /// Every `(tenant, permission)` the broker asked about, in order.
    asked: Mutex<Vec<(String, Permission)>>,
}

impl GrantBook {
    /// Grant `permission` to `tenant`, or to every tenant with `None`.
    pub(crate) fn grant(&self, tenant: Option<&str>, permission: Permission) {
        match self.held.lock() {
            Ok(mut guard) => guard.push((tenant.map(String::from), permission)),
            Err(poisoned) => poisoned
                .into_inner()
                .push((tenant.map(String::from), permission)),
        }
    }

    /// Revoke every grant.
    pub(crate) fn clear(&self) {
        match self.held.lock() {
            Ok(mut guard) => guard.clear(),
            Err(poisoned) => poisoned.into_inner().clear(),
        }
    }

    /// Every question asked so far.
    pub(crate) fn asked(&self) -> Vec<(String, Permission)> {
        match self.asked.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Answer the broker, remembering the question.
    fn allows(&self, tenant: &str, permission: &Permission) -> bool {
        match self.asked.lock() {
            Ok(mut guard) => guard.push((String::from(tenant), permission.clone())),
            Err(poisoned) => poisoned
                .into_inner()
                .push((String::from(tenant), permission.clone())),
        }
        let held = match self.held.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        held.iter().any(|entry| {
            entry.1 == *permission && entry.0.as_deref().is_none_or(|name| name == tenant)
        })
    }
}

/// Test harness: broker plus its frozen dependencies, grant store, and audit
/// capture.
pub(crate) struct Harness {
    /// Broker under test.
    broker: Broker,
    /// Frozen clock.
    time: Arc<FrozenTime>,
    /// The injected grant store.
    grants: Arc<GrantBook>,
    /// Captured audit events.
    audit: Arc<Mutex<Vec<AuditEvent>>>,
}

impl Harness {
    /// Build a harness whose ML grants verify with `verdict`. The grant store
    /// starts with `intent.emit` granted to every tenant.
    pub(crate) fn with_verdict(verdict: GrantVerificationResult) -> Self {
        Self::build(verdict, FrozenIds::default())
    }

    /// Build a harness whose identity space and clock start from `seed`.
    pub(crate) fn seeded(seed: u64) -> Self {
        let base = u128::from(seed).wrapping_mul(0x9e37_79b9_7f4a_7c15_u128);
        let harness = Self::build(GrantVerificationResult::Valid, FrozenIds::starting_at(base));
        harness.time().set(seed % 10_000);
        harness
    }

    /// Wire the broker over the frozen clock, `ids`, grant store, and sink.
    fn build(verdict: GrantVerificationResult, ids: FrozenIds) -> Self {
        let time = Arc::new(FrozenTime::default());
        let ids = Arc::new(ids);
        let audit = Arc::new(Mutex::new(Vec::new()));
        let grants = Arc::new(GrantBook::default());
        grants.grant(None, Permission::Capability(Capability::IntentEmit));
        let clock: ClockFn = {
            let time = Arc::clone(&time);
            Arc::new(move || time.get())
        };
        let new_id: IdFn = {
            let ids = Arc::clone(&ids);
            Arc::new(move || ids.mint())
        };
        let verify: VerifyGrantFn = Arc::new(move |_| verdict.clone());
        let grant_fn: GrantFn = {
            let book = Arc::clone(&grants);
            Arc::new(move |session, permission| book.allows(session.tenant_id(), permission))
        };
        let audit_fn: AuditFn = {
            let audit = Arc::clone(&audit);
            Arc::new(move |event: &AuditEvent| {
                lock_recover(&audit).push(event.clone());
            })
        };
        Self {
            broker: Broker::new(clock, new_id, verify, grant_fn, audit_fn),
            time,
            grants,
            audit,
        }
    }

    /// Build a harness with valid grants.
    pub(crate) fn valid() -> Self {
        Self::with_verdict(GrantVerificationResult::Valid)
    }

    /// Broker under test.
    pub(crate) fn broker(&self) -> &Broker {
        &self.broker
    }

    /// Frozen clock.
    pub(crate) fn time(&self) -> &Arc<FrozenTime> {
        &self.time
    }

    /// The injected grant store.
    pub(crate) fn grants(&self) -> &Arc<GrantBook> {
        &self.grants
    }

    /// Every audit event, in the order the sink received them.
    pub(crate) fn audit_events(&self) -> Vec<AuditEvent> {
        lock_recover(&self.audit).clone()
    }

    /// Signal ids of the routed envelopes, in routing order.
    pub(crate) fn audited(&self) -> Vec<String> {
        self.audit_events()
            .iter()
            .filter_map(|event| match *event.action() {
                AuditAction::Route { ref signal_id, .. } => Some(signal_id.clone()),
                _ => None,
            })
            .collect()
    }
}

/// Tap `caller`'s own scope or fail the test.
pub(crate) fn tap_own(
    harness: &Harness,
    caller: &RegisteredHandle,
    listener: TapListener,
) -> Result<TapId, String> {
    harness
        .broker()
        .tap(caller, Reach::Own, listener)
        .map_err(|error| format!("tap failed: {error}"))
}

/// Cancel a tap as its owner or fail the test.
pub(crate) fn untap_ok(
    harness: &Harness,
    caller: &RegisteredHandle,
    id: &TapId,
) -> Result<(), String> {
    harness
        .broker()
        .untap(caller, id)
        .map_err(|error| format!("untap failed: {error}"))
}

/// The subscriptions `caller` may see in its own scope, or fail the test.
pub(crate) fn own_subscriptions(
    harness: &Harness,
    caller: &RegisteredHandle,
) -> Result<Vec<Subscription>, String> {
    harness
        .broker()
        .registered_subscriptions(caller, Reach::Own)
        .map_err(|error| format!("inspect failed: {error}"))
}

/// The `(tenant, session)` pairs one inspector exposes to `caller` at `reach`
/// (the outer error is a harness fault, the inner one the broker's refusal).
/// A tap is judged by what it hears when every emitter emits once.
pub(crate) fn exposure(
    harness: &Harness,
    which: Inspector,
    caller: &RegisteredHandle,
    reach: Reach,
    emitters: &[RegisteredHandle],
) -> Result<Result<BTreeSet<(String, String)>, BrokerError>, String> {
    match which {
        Inspector::Subscriptions => Ok(harness
            .broker()
            .registered_subscriptions(caller, reach)
            .map(|listed| {
                listed
                    .iter()
                    .map(|sub| {
                        (
                            String::from(sub.tenant_id()),
                            String::from(sub.session_id()),
                        )
                    })
                    .collect()
            })),
        Inspector::Taps => {
            let heard = Arc::new(Mutex::new(BTreeSet::new()));
            let sink = Arc::clone(&heard);
            let listener: TapListener = Arc::new(move |event: TapEvent| {
                let seen = event.envelope().session_invariant();
                lock_recover(&sink).insert((
                    String::from(seen.tenant_id()),
                    String::from(seen.session_id()),
                ));
            });
            let id = match harness.broker().tap(caller, reach, listener) {
                Ok(id) => id,
                Err(refusal) => return Ok(Err(refusal)),
            };
            for emitter in emitters {
                harness
                    .broker()
                    .emit(emitter, "probe", JsonValue::Null)
                    .map_err(|error| format!("probe emit failed: {error}"))?;
            }
            harness
                .broker()
                .untap(caller, &id)
                .map_err(|error| format!("probe untap failed: {error}"))?;
            let seen = lock_recover(&heard).clone();
            Ok(Ok(seen))
        }
        other => Err(format!("no driver for inspector {other:?}")),
    }
}

/// Session fixture binding `tenant` to `session_id` under the fixed work
/// object, policy bundle and user; a rejected shape comes back as the fixture
/// fault text rather than a panic.
pub(crate) fn session(tenant: &str, session_id: &str) -> Result<SessionInvariant, String> {
    crate::fixtures::session_in(tenant, session_id)
}

/// Descriptor of kind `demo.kind` holding exactly `capabilities`, where
/// `self_listen` decides whether its own emissions reach its own
/// subscriptions.
pub(crate) fn widget(
    widget_id: &str,
    capabilities: Vec<Capability>,
    self_listen: bool,
) -> WidgetDescriptor {
    let mut descriptor = crate::fixtures::widget_descriptor(widget_id, "demo.kind", capabilities);
    descriptor.set_self_listen(self_listen);
    descriptor
}

/// Register a widget with an explicit self-listen flag or fail the test.
pub(crate) fn register_with(
    harness: &Harness,
    widget_id: &str,
    tenant: &str,
    session_id: &str,
    self_listen: bool,
) -> Result<RegisteredHandle, String> {
    harness
        .broker
        .register(
            widget(
                widget_id,
                vec![Capability::SignalEmit, Capability::SignalSubscribe],
                self_listen,
            ),
            session(tenant, session_id)?,
        )
        .map_err(|error| format!("register failed: {error}"))
}

/// Register a default widget or fail the test.
pub(crate) fn register(
    harness: &Harness,
    widget_id: &str,
    tenant: &str,
    session_id: &str,
) -> Result<RegisteredHandle, String> {
    register_with(harness, widget_id, tenant, session_id, false)
}

/// Register a self-listening widget: its own emissions deliver back to it,
/// so tap and fan-out counts observe the full match set.
pub(crate) fn register_listen(
    harness: &Harness,
    widget_id: &str,
    tenant: &str,
    session_id: &str,
) -> Result<RegisteredHandle, String> {
    register_with(harness, widget_id, tenant, session_id, true)
}

/// Collector listener appending every delivery.
pub(crate) fn collector(slot: &Arc<Mutex<Vec<SignalEnvelope>>>) -> SignalListener {
    let slot = Arc::clone(slot);
    Arc::new(move |envelope: SignalEnvelope| {
        lock_vec(&slot).push(envelope);
    })
}

/// Subscribe a catch-all and return its deliveries.
pub(crate) fn subscribe_all(
    harness: &Harness,
    handle: &RegisteredHandle,
) -> Result<Arc<Mutex<Vec<SignalEnvelope>>>, String> {
    let deliveries = Arc::new(Mutex::new(Vec::new()));
    harness
        .broker
        .subscribe(
            handle,
            SignalPattern::new(String::from("**"), None),
            collector(&deliveries),
        )
        .map_err(|error| format!("subscribe failed: {error}"))?;
    Ok(deliveries)
}

/// Harness validity: the shared broker harness registers, subscribes, emits,
/// taps, and audits across the isolation sessions. This test lives with the
/// harness so every suite that includes it also proves it.
#[test]
fn shared_harness_is_valid() -> Result<(), String> {
    let seeded = Harness::seeded(7);
    let probe = register(&seeded, "widget.seeded", "tenant", SESSION_ONE)?;
    let other = Harness::seeded(8);
    let rival = register(&other, "widget.seeded", "tenant", SESSION_ONE)?;
    ensure(
        seeded.time().get() == 7 && probe.nonce() != rival.nonce(),
        || String::from("a seed must fix the clock and the identity space"),
    )?;
    let harness = Harness::valid();
    let first = register_listen(&harness, "widget.shared-one", "tenant", SESSION_ONE)?;
    let second = register_listen(&harness, "widget.shared-two", "tenant", SESSION_TWO)?;
    let shared = register_listen(&harness, "widget.shared-each", "tenant", SESSION_SHARED)?;
    let log_second = subscribe_all(&harness, &second)?;
    let log_shared = subscribe_all(&harness, &shared)?;
    let slot = Arc::new(Mutex::new(Vec::new()));
    harness
        .broker()
        .subscribe(
            &first,
            SignalPattern::new(String::from("**"), None),
            collector(&slot),
        )
        .map_err(|error| format!("subscribe failed: {error}"))?;
    harness
        .broker()
        .emit(&second, "ping", JsonValue::Null)
        .map_err(|error| format!("emit failed: {error}"))?;
    ensure(
        lock_vec(&log_second).len() == 1 && lock_vec(&slot).is_empty(),
        || String::from("delivery must respect session lines"),
    )?;
    harness
        .broker()
        .emit(&first, "pong", JsonValue::Null)
        .map_err(|error| format!("emit failed: {error}"))?;
    ensure(
        lock_vec(&slot).len() == 1 && lock_vec(&log_second).len() == 1,
        || String::from("same-session delivery must fan out"),
    )?;
    harness.time().set(5_000);
    harness
        .broker()
        .emit(&shared, "tick", JsonValue::Null)
        .map_err(|error| format!("emit failed: {error}"))?;
    ensure(lock_vec(&log_shared).len() == 1, || {
        String::from("shared session must deliver")
    })?;
    ensure(harness.audited().len() == 3, || {
        String::from("audit must record every emit")
    })?;
    let deaf = register(&harness, "widget.shared-deaf", "tenant", SESSION_ONE)?;
    let log_deaf = subscribe_all(&harness, &deaf)?;
    harness
        .broker()
        .emit(&deaf, "ping", JsonValue::Null)
        .map_err(|error| format!("emit failed: {error}"))?;
    ensure(lock_vec(&log_deaf).is_empty(), || {
        String::from("no self-delivery without self_listen")
    })?;
    ensure(
        lock_vec(&slot).len() == 2 && harness.audited().len() == 4,
        || String::from("deaf widget must still fan out"),
    )?;
    let own = BTreeSet::from([(String::from("tenant"), String::from(SESSION_ONE))]);
    for which in Inspector::ALL {
        let seen = exposure(
            &harness,
            which,
            &first,
            Reach::Own,
            std::slice::from_ref(&first),
        )?;
        ensure(seen == Ok(own.clone()), || {
            format!("{which:?} must expose exactly the caller's scope")
        })?;
    }
    ensure(own_subscriptions(&harness, &second)?.len() == 1, || {
        String::from("the caller must see its own subscription only")
    })?;
    let mute: TapListener = Arc::new(|_: TapEvent| {});
    let id = tap_own(&harness, &first, mute)?;
    untap_ok(&harness, &first, &id)?;
    harness.grants().clear();
    let wide = harness
        .broker()
        .register_command_bar(session("tenant", SESSION_ONE)?);
    let asked = harness.grants().asked();
    let events = harness.audit_events();
    let denied = matches!(
        events.last().map(AuditEvent::decision),
        Some(&Decision::Denied(_))
    );
    ensure(wide.is_err() && !asked.is_empty() && denied, || {
        String::from("an ungranted bar must be refused, asked, and audited")
    })
}
