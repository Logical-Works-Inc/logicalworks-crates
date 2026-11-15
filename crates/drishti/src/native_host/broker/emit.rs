//! Broker emit and delivery paths: widget, intent, and prediction emission.
//!
//! Split from the broker root for the size law. Registration, the registry
//! state, and rate-limit machinery stay in the parent module; this module
//! owns the delivery paths that run after admission. The lock is always
//! released before any caller closure runs (see [`Broker::route`]).

use std::sync::Arc;

use lgwks_std::id::Uuid;

use super::super::ensure;
use super::super::pattern::match_pattern;
use super::super::protocol::{JsonValue, PartialIntent, SignalEnvelope, SignalPattern, SourceKind};
use super::super::time::format_millis;
use super::audit::same_scope;
use super::inspect::caller_of;
use super::limits::{payload_within, tenant_count, within};
use super::{
    AuditAction, Broker, BrokerError, BrokerState, Caller, CommandBarRegistration, Decision,
    Emitter, IntentHandle, Limit, MlHandle, MlRegistration, Reach, RegisteredHandle,
    SignalListener, StoredSubscription, Subject, SubscriptionId, TapEvent, TapListener,
    TokenBucket, WidgetRegistration, canonical_handle, require_valid_kind, widget_key,
};
use crate::canvas_protocol::SessionInvariant;

impl Broker {
    /// Deliver `envelope` to collected listeners, audit it, and fire taps:
    /// the shared tail of the widget, intent, and prediction emit paths.
    /// The lock must already be released; delivery runs caller closures.
    /// The route is audited before any tap runs, so a panicking tap surfaces
    /// after the record exists.
    fn route(
        &self,
        listeners: &[SignalListener],
        taps: &[TapListener],
        envelope: SignalEnvelope,
        throttled: u64,
        now_ms: u64,
    ) -> SignalEnvelope {
        let (matched, fanout_duration_ns) = deliver(listeners, &envelope);
        let action = AuditAction::Route {
            signal_id: envelope.signal_id().to_owned(),
            kind: envelope.kind().to_owned(),
            source_id: envelope.source_id().to_owned(),
        };
        let caller = Caller::of(envelope.session_invariant());
        self.publish(caller, action, Reach::Own, Decision::Allowed);
        fire_taps(
            taps,
            &TapEvent::new(
                redact_payload(&envelope),
                matched,
                fanout_duration_ns,
                throttled,
                now_ms,
            ),
        );
        envelope
    }

    /// Audit the refusal in `outcome`, if any, as a denied emit by the caller
    /// `resolve` finds in the registry (or the handle's own claim). The lock is
    /// taken after the refusing path released it and dropped before the sink
    /// runs. Every refusal is audited, rate limits included: the sink owns its
    /// own backpressure, and a hidden flood is worse than a loud one.
    fn audit_refusal<T>(
        &self,
        outcome: Result<T, BrokerError>,
        emitter: Emitter,
        resolve: impl FnOnce(&BrokerState) -> Caller,
    ) -> Result<T, BrokerError> {
        if let Err(ref error) = outcome {
            let who = resolve(&self.lock());
            self.audit_denial(who, AuditAction::Emit(emitter), error);
        }
        outcome
    }

    /// The checks every emit path makes before it touches the registry: the
    /// kind grammar, then the payload bound. Returns the injected time and a
    /// fresh signal id, both read outside the lock.
    fn begin_emit(&self, kind: &str, payload: &JsonValue) -> Result<(u64, Uuid), BrokerError> {
        require_valid_kind(kind)?;
        payload_within(payload)?;
        Ok(((self.now)(), (self.new_id)()))
    }

    /// Emit a widget signal: capability-checked registration, kind-checked
    /// payload, token-bucket admission, authoritative stamping, session- and
    /// tenant-scoped fan-out, audit, and tap.
    ///
    /// # Errors
    ///
    /// [`BrokerError::MalformedKind`], [`BrokerError::LimitExceeded`] for a
    /// payload past [`Limit::PayloadBytes`], [`BrokerError::InvalidHandle`] or
    /// [`BrokerError::RateLimited`]; every refusal is audited.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::BrokerError;
    /// use drishti::native_host::protocol::JsonValue;
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &bound)?;
    /// let sent = broker.emit(&owner, "forms.submitted", JsonValue::Null)?;
    /// assert_eq!(sent.kind(), "forms.submitted");
    /// let bad = broker.emit(&owner, "bad..kind", JsonValue::Null);
    /// assert_eq!(bad.err(), Some(BrokerError::MalformedKind));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn emit(
        &self,
        handle: &RegisteredHandle,
        kind: &str,
        payload: JsonValue,
    ) -> Result<SignalEnvelope, BrokerError> {
        let outcome = self.emit_widget(handle, kind, payload);
        self.audit_refusal(outcome, Emitter::Widget, |state| caller_of(state, handle))
    }

    /// Admission and delivery for [`Broker::emit`].
    fn emit_widget(
        &self,
        handle: &RegisteredHandle,
        kind: &str,
        payload: JsonValue,
    ) -> Result<SignalEnvelope, BrokerError> {
        let (now_ms, signal_id) = self.begin_emit(kind, &payload)?;
        let mut state = self.lock();
        let registration = state
            .widgets
            .get(&handle.nonce())
            .cloned()
            .ok_or(BrokerError::InvalidHandle)?;
        let key = widget_key(&registration);
        let bucket = state.rate_limiters.entry(key.clone());
        let bucket = bucket.or_insert_with(|| TokenBucket::new(now_ms, 100.0, 200.0));
        ensure(bucket.consume(now_ms), || {
            let count = state.rate_limited_counts.entry(key.clone()).or_insert(0);
            *count = count.saturating_add(1);
            BrokerError::RateLimited
        })?;
        let envelope = stamp_widget(kind, payload, &registration, now_ms, &signal_id);
        let throttled = {
            let slot = state.rate_limited_counts.entry(key).or_insert(0);
            let taken = *slot;
            *slot = 0;
            taken
        };
        let skip = (!registration.descriptor.self_listen()).then_some(handle.nonce());
        let listeners = matching_listeners(&state, &envelope, skip);
        let taps = tap_listeners(&state);
        drop(state);
        Ok(self.route(&listeners, &taps, envelope, throttled, now_ms))
    }

    /// Subscribe a registered widget's listener. Delivery scope is the
    /// broker's own registration, not the fields of the handle presented.
    ///
    /// # Errors
    ///
    /// [`BrokerError::InvalidHandle`] for an unknown handle and
    /// [`BrokerError::LimitExceeded`] for a full registry; every attempt is
    /// audited.
    /// # Examples
    /// ```
    /// use std::sync::Arc;
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::{Reach, SignalListener};
    /// use drishti::native_host::protocol::SignalPattern;
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &bound)?;
    /// let listener: SignalListener = Arc::new(|envelope| drop(envelope));
    /// let id = broker.subscribe(&owner, SignalPattern::new("forms.**".into(), None), listener)?;
    /// let held = broker.registered_subscriptions(&owner, Reach::Own)?;
    /// assert_eq!(held.iter().map(|view| view.id()).collect::<Vec<_>>(), [id]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn subscribe(
        &self,
        handle: &RegisteredHandle,
        pattern: SignalPattern,
        listener: SignalListener,
    ) -> Result<SubscriptionId, BrokerError> {
        let id = (self.new_id)();
        let mut state = self.lock();
        let who = caller_of(&state, handle);
        let outcome = admit_subscription(&mut state, handle, id, pattern, listener);
        drop(state);
        self.decide(
            who,
            AuditAction::Register(Subject::Subscription),
            Reach::Own,
            outcome,
        )
    }

    /// Remove a subscription. Only the owning widget's handle acts; a
    /// stranger and an unknown id are both audited as a denial.
    ///
    /// # Errors
    ///
    /// [`BrokerError::InvalidHandle`] for an unknown id or a non-owner.
    /// # Examples
    /// ```
    /// use std::sync::Arc;
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::{BrokerError, SignalListener};
    /// use drishti::native_host::protocol::SignalPattern;
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &bound)?;
    /// let listener: SignalListener = Arc::new(|envelope| drop(envelope));
    /// let id = broker.subscribe(&owner, SignalPattern::new("a".into(), None), listener)?;
    /// broker.unsubscribe(&owner, &id)?;
    /// // The slot is gone, so a second revocation is a denial.
    /// assert_eq!(broker.unsubscribe(&owner, &id), Err(BrokerError::InvalidHandle));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn unsubscribe(
        &self,
        handle: &RegisteredHandle,
        id: &SubscriptionId,
    ) -> Result<(), BrokerError> {
        let mut state = self.lock();
        let who = caller_of(&state, handle);
        let owned = state
            .subscriptions
            .get(&id.0)
            .is_some_and(|held| held.handle.nonce() == handle.nonce());
        let outcome = if owned {
            state.subscriptions.remove(&id.0);
            Ok(())
        } else {
            Err(BrokerError::InvalidHandle)
        };
        drop(state);
        self.decide(
            who,
            AuditAction::Revoke(Subject::Subscription),
            Reach::Own,
            outcome,
        )
    }

    /// Emit a command-bar intent: handle-bound session check, kind check,
    /// 5/second token bucket, user stamping, fan-out, audit, tap.
    ///
    /// # Errors
    ///
    /// The refusals of [`Broker::emit`]; every refusal is audited.
    /// # Examples
    /// ```
    /// use std::sync::atomic::Ordering;
    /// use drishti::cli::drive::{frozen_broker, session};
    /// use drishti::native_host::protocol::{JsonValue, PartialIntent};
    /// let (broker, _events, open) = frozen_broker();
    /// open.store(true, Ordering::SeqCst);
    /// let bar = broker.register_command_bar(session(&mut String::new(), "tenant-a")?)?;
    /// let intent = PartialIntent::new("intent.find".into(), JsonValue::Null, "/find".into());
    /// let sent = broker.emit_intent(&bar, intent)?;
    /// assert_eq!(sent.kind(), "intent.find");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn emit_intent(
        &self,
        handle: &IntentHandle,
        partial: PartialIntent,
    ) -> Result<SignalEnvelope, BrokerError> {
        let outcome = self.emit_bar(handle, partial);
        self.audit_refusal(outcome, Emitter::Intent, |state| {
            match state.command_bar.as_ref() {
                Some(bar) if bar.handle.nonce() == handle.nonce() => Caller::of(&bar.session),
                Some(_) | None => Caller::from_bar(handle),
            }
        })
    }

    /// Admission and delivery for [`Broker::emit_intent`].
    fn emit_bar(
        &self,
        handle: &IntentHandle,
        partial: PartialIntent,
    ) -> Result<SignalEnvelope, BrokerError> {
        let (now_ms, signal_id) = self.begin_emit(partial.kind(), partial.payload())?;
        let mut state = self.lock();
        let registration = state
            .command_bar
            .as_ref()
            .filter(|registration| registration.handle.nonce() == handle.nonce())
            .cloned()
            .ok_or(BrokerError::InvalidHandle)?;
        let claims_match = handle.tenant_id() == registration.session.tenant_id()
            && handle.user_sub() == registration.session.user_sub();
        ensure(claims_match, || BrokerError::InvalidHandle)?;
        let bucket = state.intent_limiters.entry(handle.nonce());
        let bucket = bucket.or_insert_with(|| TokenBucket::new(now_ms, 5.0, 5.0));
        ensure(bucket.consume(now_ms), || {
            let count = state
                .intent_limited_counts
                .entry(handle.nonce())
                .or_insert(0);
            *count = count.saturating_add(1);
            BrokerError::RateLimited
        })?;
        let envelope = stamp_intent(&partial, &registration, now_ms, &signal_id);
        let throttled = {
            let slot = state
                .intent_limited_counts
                .entry(handle.nonce())
                .or_insert(0);
            let taken = *slot;
            *slot = 0;
            taken
        };
        let listeners = matching_listeners(&state, &envelope, None);
        let taps = tap_listeners(&state);
        drop(state);
        Ok(self.route(&listeners, &taps, envelope, throttled, now_ms))
    }

    /// Emit an ML prediction: peer plus session lookup, kind check, ML
    /// stamping, fan-out, audit, tap.
    ///
    /// # Errors
    ///
    /// The refusals of [`Broker::emit`] except the rate limit; every refusal
    /// is audited.
    /// # Examples
    /// ```
    /// use drishti::canvas_protocol::Capability;
    /// use drishti::cli::drive::{frozen_broker, session};
    /// use drishti::native_host::broker::{BrokerError, MlHandle, SignedMlGrant};
    /// use drishti::native_host::protocol::JsonValue;
    /// let (broker, _events, _open) = frozen_broker();
    /// let bound = session(&mut String::new(), "tenant-a")?;
    /// let id = lgwks_std::id::Uuid::from([1_u8; 16]);
    /// let grant = SignedMlGrant::new(id, "tenant-a".into(), vec![Capability::TapeRead], 9_999, 1_000, vec![]);
    /// let peer = broker.register_ml_peer(grant, bound)?;
    /// let sent = broker.emit_prediction(&peer, "ml.suggest", JsonValue::Null)?;
    /// assert_eq!(sent.kind(), "ml.suggest");
    /// // A handle the broker never issued is refused.
    /// let forged = MlHandle::new(lgwks_std::id::Uuid::from([9_u8; 16]), "tenant-a".into());
    /// assert_eq!(broker.emit_prediction(&forged, "ml.suggest", JsonValue::Null).err(), Some(BrokerError::InvalidHandle));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn emit_prediction(
        &self,
        handle: &MlHandle,
        kind: &str,
        payload: JsonValue,
    ) -> Result<SignalEnvelope, BrokerError> {
        let outcome = self.emit_peer(handle, kind, payload);
        self.audit_refusal(outcome, Emitter::Prediction, |state| {
            match state.ml_sessions.get(&handle.nonce()) {
                Some(session) => Caller::of(session),
                None => Caller::from_tenant(handle.tenant_id()),
            }
        })
    }

    /// Admission and delivery for [`Broker::emit_prediction`].
    fn emit_peer(
        &self,
        handle: &MlHandle,
        kind: &str,
        payload: JsonValue,
    ) -> Result<SignalEnvelope, BrokerError> {
        let (now_ms, signal_id) = self.begin_emit(kind, &payload)?;
        let state = self.lock();
        let registration = state
            .ml_peers
            .get(&handle.nonce())
            .cloned()
            .ok_or(BrokerError::InvalidHandle)?;
        let session = state
            .ml_sessions
            .get(&handle.nonce())
            .cloned()
            .ok_or(BrokerError::InvalidHandle)?;
        let envelope = stamp_ml(kind, payload, &session, &registration, &signal_id, now_ms);
        let listeners = matching_listeners(&state, &envelope, None);
        let taps = tap_listeners(&state);
        drop(state);
        Ok(self.route(&listeners, &taps, envelope, 0, now_ms))
    }
}

/// Collect fan-out listeners for `envelope`: session- and tenant-scoped
/// subscriptions whose pattern matches. `skip_nonce` carries the emitting
/// widget when it does not listen to itself; the intent and prediction paths
/// pass none.
fn matching_listeners(
    state: &BrokerState,
    envelope: &SignalEnvelope,
    skip_nonce: Option<Uuid>,
) -> Vec<SignalListener> {
    let mut listeners = Vec::new();
    for sub in state.subscriptions.values() {
        if skip_nonce == Some(sub.handle.nonce()) {
            continue;
        }
        let seen = envelope.session_invariant();
        let in_scope = same_scope(
            sub.handle.session_id(),
            sub.handle.tenant_id(),
            seen.session_id(),
            seen.tenant_id(),
        );
        if in_scope && match_pattern(envelope, &sub.pattern) {
            listeners.push(Arc::clone(&sub.listener));
        }
    }
    listeners
}

/// Admit a subscription under the bounds. The stored handle is rebuilt from
/// the broker's own registration, so a handle that lies about its tenant or
/// session cannot move the subscription's delivery scope.
fn admit_subscription(
    state: &mut BrokerState,
    handle: &RegisteredHandle,
    id: Uuid,
    pattern: SignalPattern,
    listener: SignalListener,
) -> Result<SubscriptionId, BrokerError> {
    let registration = state
        .widgets
        .get(&handle.nonce())
        .ok_or(BrokerError::InvalidHandle)?;
    let owner = canonical_handle(handle.nonce(), registration);
    within(state.subscriptions.len(), Limit::Subscriptions)?;
    let holders = state
        .subscriptions
        .values()
        .map(|held| held.handle.tenant_id());
    within(
        tenant_count(holders, owner.tenant_id()),
        Limit::SubscriptionsPerTenant,
    )?;
    state.subscriptions.insert(
        id,
        StoredSubscription {
            handle: owner,
            pattern,
            listener,
        },
    );
    Ok(SubscriptionId(id))
}

/// Deliver one envelope to every collected listener. Returns the matched
/// count and the monotonic fan-out nanoseconds. A panicking subscriber is a
/// caller bug and aborts the emit loudly: unlike taps, delivery makes no
/// containment promise.
fn deliver(listeners: &[SignalListener], envelope: &SignalEnvelope) -> (usize, u64) {
    let started = std::time::Instant::now();
    for listener in listeners {
        listener(envelope.clone());
    }
    let nanos = started.elapsed().as_nanos().min(u128::from(u64::MAX));
    // The value was just clamped to `u64::MAX`, so the low eight bytes are
    // the whole value, with no conversion to fail.
    let bytes = nanos.to_le_bytes();
    let clamped = u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    (listeners.len(), clamped)
}

/// Stamp a widget envelope: caller supplies kind and payload only. Identity
/// comes from the registration, never the caller.
fn stamp_widget(
    kind: &str,
    payload: JsonValue,
    registration: &WidgetRegistration,
    now_ms: u64,
    signal_id: &Uuid,
) -> SignalEnvelope {
    SignalEnvelope::new(
        signal_id.to_string(),
        String::from(kind),
        SourceKind::Widget,
        registration.descriptor.widget_id().to_owned(),
        registration.session.clone(),
        payload,
        format_millis(now_ms),
    )
}

/// Stamp an ML envelope with the grant tenant as the source id.
fn stamp_ml(
    kind: &str,
    payload: JsonValue,
    session: &SessionInvariant,
    registration: &MlRegistration,
    signal_id: &Uuid,
    now_ms: u64,
) -> SignalEnvelope {
    SignalEnvelope::new(
        signal_id.to_string(),
        String::from(kind),
        SourceKind::Ml,
        registration.grant.tenant_id().to_owned(),
        session.clone(),
        payload,
        format_millis(now_ms),
    )
}

/// Stamp a command-bar intent as the user, sourced at
/// `command-bar:<user>`.
fn stamp_intent(
    partial: &PartialIntent,
    registration: &CommandBarRegistration,
    now_ms: u64,
    signal_id: &Uuid,
) -> SignalEnvelope {
    SignalEnvelope::new(
        signal_id.to_string(),
        partial.kind().to_owned(),
        SourceKind::User,
        format!("command-bar:{}", registration.session.user_sub()),
        registration.session.clone(),
        partial.payload().clone(),
        format_millis(now_ms),
    )
}

/// Redact an envelope for the tap stream: identity without payload.
fn redact_payload(envelope: &SignalEnvelope) -> SignalEnvelope {
    SignalEnvelope::new(
        envelope.signal_id().to_owned(),
        envelope.kind().to_owned(),
        envelope.source_kind(),
        envelope.source_id().to_owned(),
        envelope.session_invariant().clone(),
        JsonValue::Null,
        envelope.emitted_at().to_owned(),
    )
}

/// Fire tap listeners. A panicking tap is a caller bug and surfaces from the
/// emit like a panicking subscriber: the route was audited first, and no
/// broker lock is held, so the broker stays usable.
fn fire_taps(taps: &[TapListener], event: &TapEvent) {
    for tap in taps {
        tap(event.clone());
    }
}

/// The tap listeners to fire for one route, in id order.
fn tap_listeners(state: &BrokerState) -> Vec<TapListener> {
    state
        .taps
        .values()
        .map(|held| Arc::clone(&held.listener))
        .collect()
}
