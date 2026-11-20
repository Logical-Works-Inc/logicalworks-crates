//! Canvas signal broker: registry, stamping, fan-out, rate limits, tap.
//!
//! Absorbed from `surfaces/native-host/Sources/CanvasBroker` (kernel#850 row
//! 15). The Swift actor becomes a synchronous mutex-guarded broker: every
//! method takes `&self`, locks once for state, and never holds the lock
//! while invoking caller closures (listeners, audit sink, identity and clock
//! functions live outside the lock entirely), so re-entrant calls cannot
//! deadlock. Subscription and tap maps iterate in id order, so delivery
//! order is deterministic for a fixed registration sequence.
//!
//! Beyond the Swift source, all tested: delivery requires both session AND
//! tenant equality (tenant-scoped subscriptions); command-bar registration
//! and cross-tenant reads require a grant from the injected grant store;
//! every inspector reads through one scope filter bound to the caller's own
//! registration; every registry has a numeric bound; revocation needs the
//! owning handle; and every decision reaches the injected audit sink as a
//! typed [`AuditEvent`] (the required audit log). No caller closure (clock,
//! identity, grant store, audit sink, listener) runs under the state lock.
//!
//! Split for the size law: identity types live in `handles`, the audit
//! vocabulary in `audit`, the registry bounds in `limits`, the inspectors in
//! `inspect`, the emit and delivery paths in `emit`, the injected callbacks
//! and the signed grant in `grant`; this root owns the registry state,
//! admission, and rate-limit machinery.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use lgwks_macros::Error;

use lgwks_std::id::Uuid;

use super::capability::{CapabilityError, is_ml_only, is_privileged};
use super::ensure;
use super::protocol::{SignalPattern, WidgetDescriptor, valid_signal_kind};
use crate::canvas_protocol::{Capability, SessionInvariant};

/// Audit vocabulary and the scope filter (split for the size law).
mod audit;
/// Emit and delivery paths (split for the size law).
mod emit;
/// Injected callbacks and the signed ML grant (split for the size law).
mod grant;
/// Opaque handles and event records (split for the size law).
mod handles;
/// Scoped inspectors and owner-checked tap revocation.
mod inspect;
/// Registry bounds.
mod limits;

pub use audit::{
    AuditAction, AuditEvent, Caller, Decision, Emitter, Inspector, Permission, Reach, Scope,
    Subject,
};
pub use grant::{
    AuditFn, ClockFn, GrantFn, GrantVerificationResult, IdFn, SignalListener, SignedMlGrant,
    TapListener, VerifyGrantFn,
};
pub use handles::{
    IntentHandle, MlHandle, RegisteredHandle, Subscription, SubscriptionId, TapEvent, TapId,
};
pub use limits::Limit;

use limits::{tenant_count, within};

/// Broker failures.
/// # Examples
/// ```
/// use drishti::native_host::broker::BrokerError;
/// assert_eq!(BrokerError::InvalidHandle.to_string(), "invalid handle");
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum BrokerError {
    /// Registration holds an ML-only or privileged capability.
    #[error("capability elevation: {0}")]
    CapabilityElevation(Capability),
    /// A widget with this source id is already registered.
    #[error("duplicate source id: {0}")]
    DuplicateSourceId(String),
    /// Unknown, forged, or retired handle.
    #[error("invalid handle")]
    InvalidHandle,
    /// Malformed signal kind.
    #[error("malformed kind")]
    MalformedKind,
    /// Token bucket exhausted.
    #[error("rate limited")]
    RateLimited,
    /// ML operation without a peer registration.
    #[error("ML grant required")]
    MlGrantRequired,
    /// ML grant failed verification.
    #[error("ML grant invalid: {0:?}")]
    MlGrantInvalid(GrantVerificationResult),
    /// A command bar is already registered.
    #[error("command bar already registered")]
    CommandBarAlreadyRegistered,
    /// The command-bar session is missing tenant or user.
    #[error("invalid session")]
    InvalidSession,
    /// The grant store did not grant a required capability.
    #[error(transparent)]
    CapabilityDenied(#[from] CapabilityError),
    /// A read across tenants without the cross-tenant grant.
    #[error("cross-tenant inspection not granted")]
    CrossTenantDenied,
    /// A registry is full.
    #[error("limit exceeded: {0}")]
    LimitExceeded(Limit),
}

/// Token-bucket rate limiter: 100 tokens/second refill with a 200 burst on
/// the widget path, 5/second with a 5 burst on the intent path. A backwards
/// clock moves the refill marker without granting tokens.
struct TokenBucket {
    /// Tokens added per second.
    refill_per_second: f64,
    /// Maximum tokens.
    capacity: f64,
    /// Current tokens.
    tokens: f64,
    /// Millisecond time of the last refill.
    last_ms: u64,
}

impl TokenBucket {
    /// Full bucket at `now_ms`.
    fn new(now_ms: u64, refill_per_second: f64, capacity: f64) -> Self {
        Self {
            refill_per_second,
            capacity,
            tokens: capacity,
            last_ms: now_ms,
        }
    }

    /// Refill from the elapsed time, then consume one token when available.
    fn consume(&mut self, now_ms: u64) -> bool {
        if now_ms < self.last_ms {
            self.last_ms = now_ms;
        } else {
            let elapsed_ms = now_ms.wrapping_sub(self.last_ms);
            self.tokens = self
                .capacity
                .min(self.tokens + seconds_from_millis(elapsed_ms) * self.refill_per_second);
            self.last_ms = now_ms;
        }
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Millisecond differences to fractional seconds, exact below 2^53 ms
/// (285,000 years of uptime), without an `as` cast: the low and high halves
/// come out of the little-endian bytes, so the narrowing is infallible.
fn seconds_from_millis(millis: u64) -> f64 {
    let bytes = millis.to_le_bytes();
    let low = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let high = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    f64::from(high).mul_add(4_294_967_296.0, f64::from(low)) / 1_000.0
}

/// Widget registration record.
#[derive(Clone)]
struct WidgetRegistration {
    /// Registered descriptor.
    descriptor: WidgetDescriptor,
    /// Session the widget registered in.
    session: SessionInvariant,
}

/// ML peer registration record.
#[derive(Clone)]
struct MlRegistration {
    /// Verified grant.
    grant: SignedMlGrant,
}

/// Command-bar registration record.
#[derive(Clone)]
struct CommandBarRegistration {
    /// Issued handle.
    handle: IntentHandle,
    /// Session the bar registered in.
    session: SessionInvariant,
}

/// Stored subscription record.
struct StoredSubscription {
    /// Owning widget handle.
    handle: RegisteredHandle,
    /// Match pattern.
    pattern: SignalPattern,
    /// Delivery closure.
    listener: SignalListener,
}

/// Stored tap record.
struct StoredTap {
    /// Nonce of the widget that owns the tap; its unregistration cancels it.
    owner: Uuid,
    /// Tenant of the owner (the per-tenant bound counts it).
    tenant: String,
    /// Scope-filtered listener.
    listener: TapListener,
}

/// Tenant and widget id: the key of everything a widget id names, so two
/// tenants may reuse an id without sharing a reservation or a bucket.
type WidgetKey = (String, String);

/// The key of `registration`.
fn widget_key(registration: &WidgetRegistration) -> WidgetKey {
    (
        registration.session.tenant_id().to_owned(),
        registration.descriptor.widget_id().to_owned(),
    )
}

/// The handle the broker itself would mint for `registration`: subscription
/// and delivery scope come from this, never from the fields a caller's handle
/// claims.
fn canonical_handle(nonce: Uuid, registration: &WidgetRegistration) -> RegisteredHandle {
    RegisteredHandle::new(
        nonce,
        registration.session.session_id().to_owned(),
        registration.descriptor.widget_id().to_owned(),
        registration.session.tenant_id().to_owned(),
        registration.session.user_sub().to_owned(),
    )
}

/// Broker state behind the handle lock: data only, no caller closures.
struct BrokerState {
    /// Nonce to widget registration.
    widgets: BTreeMap<Uuid, WidgetRegistration>,
    /// Tenant and widget id to nonce (duplicate detection).
    source_ids: BTreeMap<WidgetKey, Uuid>,
    /// Nonce to ML registration.
    ml_peers: BTreeMap<Uuid, MlRegistration>,
    /// Singleton command-bar registration.
    command_bar: Option<CommandBarRegistration>,
    /// Subscription id to record, in id order.
    subscriptions: BTreeMap<Uuid, StoredSubscription>,
    /// Tap id to record, in id order.
    taps: BTreeMap<Uuid, StoredTap>,
    /// Widget rate buckets by tenant and widget id.
    rate_limiters: BTreeMap<WidgetKey, TokenBucket>,
    /// Throttled counts since the last success, by tenant and widget id.
    rate_limited_counts: BTreeMap<WidgetKey, u64>,
    /// ML peer sessions by handle nonce.
    ml_sessions: BTreeMap<Uuid, SessionInvariant>,
    /// Command-bar rate buckets by handle nonce.
    intent_limiters: BTreeMap<Uuid, TokenBucket>,
    /// Throttled intent counts by handle nonce.
    intent_limited_counts: BTreeMap<Uuid, u64>,
}

/// Canvas signal bus: envelope stamping, capability enforcement,
/// pattern-match fan-out, and telemetry tap. Synchronous and headless.
/// # Examples
/// ```
/// use drishti::cli::drive::{captured, frozen_broker};
/// let (_broker, events, _open) = frozen_broker();
/// assert!(captured(&events).is_empty());
/// ```
pub struct Broker {
    /// Injected millisecond clock.
    now: ClockFn,
    /// Injected envelope identity.
    new_id: IdFn,
    /// Injected ML grant verifier.
    verify_grant: VerifyGrantFn,
    /// Injected grant store.
    grants: GrantFn,
    /// Injected audit sink.
    audit: AuditFn,
    /// Next audit ordinal.
    audit_seq: AtomicU64,
    /// Guarded data state.
    state: Mutex<BrokerState>,
}

impl std::fmt::Debug for Broker {
    /// The injected closures and guarded state do not print.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Broker").finish_non_exhaustive()
    }
}

impl Broker {
    /// Build a broker over injected time, identity, ML grant verification,
    /// the permission grant store, and the audit sink (required: there is no
    /// constructor without one). Time and identity are frozen by tests and
    /// live in production.
    /// # Examples
    /// ```
    /// use std::sync::Arc;
    /// use drishti::native_host::broker::{AuditEvent, AuditFn, Broker, ClockFn, GrantFn, GrantVerificationResult, IdFn, VerifyGrantFn};
    /// let clock: ClockFn = Arc::new(|| 1_000);
    /// let new_id: IdFn = Arc::new(|| lgwks_std::id::Uuid::from([7_u8; 16]));
    /// let verify: VerifyGrantFn = Arc::new(|_| GrantVerificationResult::Valid);
    /// let grants: GrantFn = Arc::new(|_, _| false);
    /// let audit: AuditFn = Arc::new(|_: &AuditEvent| {});
    /// let broker = Broker::new(clock, new_id, verify, grants, audit);
    /// // The grant store denies everything, so the command bar cannot register.
    /// let session = drishti::cli::drive::session(&mut String::new(), "t")?;
    /// assert!(broker.register_command_bar(session).is_err());
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn new(
        now: ClockFn,
        new_id: IdFn,
        verify_grant: VerifyGrantFn,
        grants: GrantFn,
        audit: AuditFn,
    ) -> Self {
        Self {
            now,
            new_id,
            verify_grant,
            grants,
            audit,
            audit_seq: AtomicU64::new(0),
            state: Mutex::new(BrokerState {
                widgets: BTreeMap::new(),
                source_ids: BTreeMap::new(),
                ml_peers: BTreeMap::new(),
                command_bar: None,
                subscriptions: BTreeMap::new(),
                taps: BTreeMap::new(),
                rate_limiters: BTreeMap::new(),
                rate_limited_counts: BTreeMap::new(),
                ml_sessions: BTreeMap::new(),
                intent_limiters: BTreeMap::new(),
                intent_limited_counts: BTreeMap::new(),
            }),
        }
    }

    /// Lock broker state, recovering from poisoning by keeping the state.
    fn lock(&self) -> MutexGuard<'_, BrokerState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Publish one decision to the audit sink. The clock is read first, then
    /// the ordinal is taken, then the sink runs, all outside the state lock:
    /// a re-entrant closure finishes its own event before this one takes its
    /// ordinal, so one thread's events reach the sink in ordinal order.
    fn publish(&self, caller: Caller, action: AuditAction, reach: Reach, decision: Decision) {
        let at_ms = (self.now)();
        let seq = self.audit_seq.fetch_add(1, Ordering::SeqCst);
        (self.audit)(&AuditEvent::new(
            seq, at_ms, caller, action, reach, decision,
        ));
    }

    /// Audit the outcome of a public decision and hand it back.
    fn decide<T>(
        &self,
        caller: Caller,
        action: AuditAction,
        reach: Reach,
        outcome: Result<T, BrokerError>,
    ) -> Result<T, BrokerError> {
        self.publish(caller, action, reach, Decision::of(&outcome));
        outcome
    }

    /// Ask the grant store (outside the lock) for `permission`.
    fn require(
        &self,
        session: &SessionInvariant,
        permission: Permission,
    ) -> Result<(), BrokerError> {
        ensure((self.grants)(session, &permission), || permission.refusal())
    }

    /// Register a widget. Rejects ML-only and privileged capabilities,
    /// duplicate widget ids within the tenant, and a full registry.
    ///
    /// # Errors
    ///
    /// [`BrokerError::CapabilityElevation`], [`BrokerError::DuplicateSourceId`]
    /// or [`BrokerError::LimitExceeded`]; every outcome is audited.
    /// # Examples
    /// ```
    /// use drishti::canvas_protocol::Capability;
    /// use drishti::cli::drive::{frozen_broker, session};
    /// use drishti::native_host::broker::BrokerError;
    /// use drishti::native_host::protocol::{WidgetDescriptor, WidgetSurface};
    /// let (broker, _events, _open) = frozen_broker();
    /// let session = session(&mut String::new(), "tenant-a")?;
    /// let widget = |caps| WidgetDescriptor::new("k".into(), "widget.a".into(), "1".into(), WidgetSurface::Native, vec![], vec![], caps);
    /// broker.register(widget(vec![Capability::SignalEmit]), session.clone())?;
    /// assert_eq!(
    ///     broker.register(widget(vec![Capability::SignalEmit]), session.clone()).err(),
    ///     Some(BrokerError::DuplicateSourceId("widget.a".into()))
    /// );
    /// // ML-only capabilities never register as a widget.
    /// let ml = WidgetDescriptor::new("k".into(), "widget.b".into(), "1".into(), WidgetSurface::Native, vec![], vec![], vec![Capability::TapeRead]);
    /// assert_eq!(broker.register(ml, session).err(), Some(BrokerError::CapabilityElevation(Capability::TapeRead)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn register(
        &self,
        descriptor: WidgetDescriptor,
        session: SessionInvariant,
    ) -> Result<RegisteredHandle, BrokerError> {
        let caller = Caller::of(&session);
        let subject = Subject::Widget(descriptor.widget_id().to_owned());
        let outcome = self.admit_widget(descriptor, session);
        self.decide(caller, AuditAction::Register(subject), Reach::Own, outcome)
    }

    /// Admission for [`Broker::register`].
    fn admit_widget(
        &self,
        descriptor: WidgetDescriptor,
        session: SessionInvariant,
    ) -> Result<RegisteredHandle, BrokerError> {
        let elevated = descriptor
            .capabilities()
            .iter()
            .find(|capability| is_ml_only(capability) || is_privileged(capability));
        if let Some(capability) = elevated {
            let refusal = Err(BrokerError::CapabilityElevation(capability.clone()));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "admit_widget: returning an error to the caller");
            return refusal;
        }
        let nonce = (self.new_id)();
        let registration = WidgetRegistration {
            descriptor,
            session,
        };
        let key = widget_key(&registration);
        let mut state = self.lock();
        ensure(!state.source_ids.contains_key(&key), || {
            BrokerError::DuplicateSourceId(key.1.clone())
        })?;
        within(state.widgets.len(), Limit::Widgets)?;
        let owners = state.widgets.values().map(|held| held.session.tenant_id());
        within(tenant_count(owners, &key.0), Limit::WidgetsPerTenant)?;
        let handle = canonical_handle(nonce, &registration);
        state.source_ids.insert(key, nonce);
        state.widgets.insert(nonce, registration);
        Ok(handle)
    }

    /// Unregister a widget: drop its subscriptions, taps, registration,
    /// source-id reservation, and rate-limit state. Only a live registration
    /// is acted on, and it is cleaned up by its own key, never by the fields
    /// of the handle presented; a retired or forged handle is audited as a
    /// denial and changes nothing.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{captured, frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::{BrokerError, Decision};
    /// let (broker, events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let session = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &session)?;
    /// broker.unregister(&owner);
    /// broker.unregister(&owner); // the retired handle is a denial
    /// let last = captured(&events).pop().map(|event| event.decision().clone());
    /// assert_eq!(last, Some(Decision::Denied(BrokerError::InvalidHandle)));
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    pub fn unregister(&self, handle: &RegisteredHandle) {
        let nonce = handle.nonce();
        let removed = self.lock().widgets.remove(&nonce);
        let (caller, outcome) = match removed {
            None => (Caller::from_widget(handle), Err(BrokerError::InvalidHandle)),
            Some(registration) => {
                let key = widget_key(&registration);
                let mut state = self.lock();
                state
                    .subscriptions
                    .retain(|_, sub| sub.handle.nonce() != nonce);
                state.taps.retain(|_, tap| tap.owner != nonce);
                state.source_ids.remove(&key);
                state.rate_limiters.remove(&key);
                state.rate_limited_counts.remove(&key);
                (Caller::of(&registration.session), Ok(()))
            }
        };
        let subject = Subject::Widget(handle.descriptor_id().to_owned());
        self.publish(
            caller,
            AuditAction::Revoke(subject),
            Reach::Own,
            Decision::of(&outcome),
        );
    }

    /// Register an ML peer: the grant must verify, must carry at least one
    /// ML-only capability, and must fit the registry.
    ///
    /// # Errors
    ///
    /// [`BrokerError::MlGrantInvalid`] or [`BrokerError::LimitExceeded`] (the
    /// registry or the session's tenant is full); every outcome is audited.
    /// # Examples
    /// ```
    /// use drishti::canvas_protocol::Capability;
    /// use drishti::cli::drive::{frozen_broker, session};
    /// use drishti::native_host::broker::{BrokerError, GrantVerificationResult, SignedMlGrant};
    /// let (broker, _events, _open) = frozen_broker();
    /// let session = session(&mut String::new(), "tenant-a")?;
    /// let id = lgwks_std::id::Uuid::from([1_u8; 16]);
    /// let grant = |caps| SignedMlGrant::new(id, "tenant-a".into(), caps, 9_999, 1_000, vec![]);
    /// let peer = broker.register_ml_peer(grant(vec![Capability::TapeRead]), session.clone())?;
    /// assert_eq!(peer.tenant_id(), "tenant-a");
    /// // A grant with no ML-only capability is structurally invalid.
    /// let plain = broker.register_ml_peer(grant(vec![Capability::SignalEmit]), session);
    /// assert_eq!(plain.err(), Some(BrokerError::MlGrantInvalid(GrantVerificationResult::InvalidGrant)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn register_ml_peer(
        &self,
        grant: SignedMlGrant,
        session: SessionInvariant,
    ) -> Result<MlHandle, BrokerError> {
        let caller = Caller::of(&session);
        let outcome = self.admit_ml_peer(grant, session);
        let action = AuditAction::Register(Subject::MlPeer);
        self.decide(caller, action, Reach::Own, outcome)
    }

    /// Admission for [`Broker::register_ml_peer`].
    fn admit_ml_peer(
        &self,
        grant: SignedMlGrant,
        session: SessionInvariant,
    ) -> Result<MlHandle, BrokerError> {
        let verdict = (self.verify_grant)(&grant);
        ensure(verdict == GrantVerificationResult::Valid, || {
            BrokerError::MlGrantInvalid(verdict.clone())
        })?;
        ensure(grant.capabilities().iter().any(is_ml_only), || {
            BrokerError::MlGrantInvalid(GrantVerificationResult::InvalidGrant)
        })?;
        let nonce = (self.new_id)();
        let mut state = self.lock();
        within(state.ml_peers.len(), Limit::MlPeers)?;
        let holders = state.ml_sessions.values().map(|held| held.tenant_id());
        within(
            tenant_count(holders, session.tenant_id()),
            Limit::MlPeersPerTenant,
        )?;
        let handle = MlHandle::new(nonce, grant.tenant_id().to_owned());
        state.ml_peers.insert(nonce, MlRegistration { grant });
        state.ml_sessions.insert(nonce, session);
        Ok(handle)
    }

    /// Unregister an ML peer. An unknown handle is audited as a denial.
    /// # Examples
    /// ```
    /// use drishti::canvas_protocol::Capability;
    /// use drishti::cli::drive::{captured, frozen_broker, session};
    /// use drishti::native_host::broker::{BrokerError, Decision, SignedMlGrant};
    /// let (broker, events, _open) = frozen_broker();
    /// let session = session(&mut String::new(), "tenant-a")?;
    /// let id = lgwks_std::id::Uuid::from([1_u8; 16]);
    /// let grant = SignedMlGrant::new(id, "tenant-a".into(), vec![Capability::TapeRead], 9_999, 1_000, vec![]);
    /// let peer = broker.register_ml_peer(grant, session)?;
    /// broker.unregister_ml_peer(&peer);
    /// broker.unregister_ml_peer(&peer);
    /// let last = captured(&events).pop().map(|event| event.decision().clone());
    /// assert_eq!(last, Some(Decision::Denied(BrokerError::InvalidHandle)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn unregister_ml_peer(&self, handle: &MlHandle) {
        let outcome = {
            let mut state = self.lock();
            state.ml_sessions.remove(&handle.nonce());
            match state.ml_peers.remove(&handle.nonce()) {
                Some(_) => Ok(()),
                None => Err(BrokerError::InvalidHandle),
            }
        };
        let caller = Caller::from_tenant(handle.tenant_id());
        let action = AuditAction::Revoke(Subject::MlPeer);
        self.publish(caller, action, Reach::Own, Decision::of(&outcome));
    }

    /// Register the singleton command bar. The injected grant store must
    /// hold `intent.emit` for the session (checked before the slot, so a
    /// caller without the grant learns nothing about the slot), and the
    /// session must name a tenant and a user.
    ///
    /// # Errors
    ///
    /// [`BrokerError::CapabilityDenied`],
    /// [`BrokerError::CommandBarAlreadyRegistered`] or
    /// [`BrokerError::InvalidSession`]; every outcome is audited.
    /// # Examples
    /// ```
    /// use std::sync::atomic::Ordering;
    /// use drishti::cli::drive::{frozen_broker, session};
    /// use drishti::native_host::broker::BrokerError;
    /// let (broker, _events, open) = frozen_broker();
    /// let session = session(&mut String::new(), "tenant-a")?;
    /// // The grant store starts closed, then opens.
    /// assert!(matches!(broker.register_command_bar(session.clone()), Err(BrokerError::CapabilityDenied(_))));
    /// open.store(true, Ordering::SeqCst);
    /// broker.register_command_bar(session.clone())?;
    /// assert_eq!(broker.register_command_bar(session).err(), Some(BrokerError::CommandBarAlreadyRegistered));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn register_command_bar(
        &self,
        session: SessionInvariant,
    ) -> Result<IntentHandle, BrokerError> {
        let caller = Caller::of(&session);
        let outcome = self.admit_command_bar(session);
        let action = AuditAction::Register(Subject::CommandBar);
        self.decide(caller, action, Reach::Own, outcome)
    }

    /// Admission for [`Broker::register_command_bar`].
    fn admit_command_bar(&self, session: SessionInvariant) -> Result<IntentHandle, BrokerError> {
        self.require(&session, Permission::Capability(Capability::IntentEmit))?;
        let nonce = (self.new_id)();
        let mut state = self.lock();
        ensure(state.command_bar.is_none(), || {
            BrokerError::CommandBarAlreadyRegistered
        })?;
        ensure(
            !session.tenant_id().is_empty() && !session.user_sub().is_empty(),
            || BrokerError::InvalidSession,
        )?;
        let handle = IntentHandle::new(
            nonce,
            session.session_id().to_owned(),
            session.tenant_id().to_owned(),
            session.user_sub().to_owned(),
        );
        state.command_bar = Some(CommandBarRegistration {
            handle: handle.clone(),
            session,
        });
        Ok(handle)
    }

    /// Unregister the command bar, dropping its rate-limit state. Only the
    /// live bar's own handle acts; any other is audited as a denial.
    /// # Examples
    /// ```
    /// use std::sync::atomic::Ordering;
    /// use drishti::cli::drive::{captured, frozen_broker, session};
    /// use drishti::native_host::broker::{BrokerError, Decision};
    /// let (broker, events, open) = frozen_broker();
    /// open.store(true, Ordering::SeqCst);
    /// let bar = broker.register_command_bar(session(&mut String::new(), "tenant-a")?)?;
    /// broker.unregister_command_bar(&bar);
    /// broker.unregister_command_bar(&bar);
    /// let last = captured(&events).pop().map(|event| event.decision().clone());
    /// assert_eq!(last, Some(Decision::Denied(BrokerError::InvalidHandle)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn unregister_command_bar(&self, handle: &IntentHandle) {
        let nonce = handle.nonce();
        let mut state = self.lock();
        let held = state.command_bar.take_if(|bar| bar.handle.nonce() == nonce);
        state.intent_limiters.remove(&nonce);
        state.intent_limited_counts.remove(&nonce);
        drop(state);
        let (caller, outcome) = match held {
            Some(bar) => (Caller::of(&bar.session), Ok(())),
            None => (Caller::from_bar(handle), Err(BrokerError::InvalidHandle)),
        };
        let action = AuditAction::Revoke(Subject::CommandBar);
        self.publish(caller, action, Reach::Own, Decision::of(&outcome));
    }
}

/// Reject malformed signal kinds before the broker stamps an envelope.
fn require_valid_kind(kind: &str) -> Result<(), BrokerError> {
    ensure(valid_signal_kind(kind), || BrokerError::MalformedKind)
}

#[cfg(test)]
mod tests {
    use super::{TokenBucket, valid_signal_kind};

    /// The widget bucket holds a 200 burst, then refills at 100/second.
    #[test]
    fn widget_bucket_bursts_then_refills() -> Result<(), String> {
        let mut bucket = TokenBucket::new(0, 100.0, 200.0);
        for _ in 0..200 {
            if !bucket.consume(0) {
                return Err(String::from("burst must allow 200"));
            }
        }
        if bucket.consume(0) {
            return Err(String::from("201st frozen emit must limit"));
        }
        if !bucket.consume(1_000) {
            return Err(String::from("one second must refill 100"));
        }
        Ok(())
    }

    /// A backwards clock moves the marker without granting tokens.
    #[test]
    fn bucket_ignores_backwards_time() -> Result<(), String> {
        let mut bucket = TokenBucket::new(10_000, 100.0, 200.0);
        for _ in 0..200 {
            bucket.consume(10_000);
        }
        if bucket.consume(0) {
            return Err(String::from("backwards clock must not refill"));
        }
        Ok(())
    }

    /// Kind validation rejects empties, dots, overlong, and controls.
    #[test]
    fn kinds_validate() -> Result<(), String> {
        for bad in ["", ".", ".leading", "trailing.", "a..b", "has\ncontrol"] {
            if valid_signal_kind(bad) {
                return Err(format!("kind must be invalid: {bad:?}"));
            }
        }
        let long = "k".repeat(129);
        if valid_signal_kind(&long) {
            return Err(String::from("129 chars must be invalid"));
        }
        for good in ["a", "intent.find", "forms.submitted.v2"] {
            if !valid_signal_kind(good) {
                return Err(format!("kind must be valid: {good:?}"));
            }
        }
        Ok(())
    }
}
