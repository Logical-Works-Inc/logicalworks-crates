//! Broker identity types: opaque handles, subscription snapshots, tap events.
//!
//! Split from the broker root for the size law. The registry, rate limits,
//! and delivery paths stay in the parent module and `emit`; this module owns
//! only identity and event records. Reads go through the accessors so a
//! forged handle can never alias a live registration.

use lgwks_std::id::Uuid;

use super::super::protocol::{SignalEnvelope, SignalPattern};

/// Opaque handle issued to a registered widget. The nonce prevents forgery:
/// only handles the broker issued will ever look up.
///
/// Fields are private: only the broker mints live handles (tests mint forged
/// ones through [`RegisteredHandle::new`] to prove they fail closed), and
/// every reader goes through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::broker::RegisteredHandle;
/// let nonce = lgwks_std::id::Uuid::from([1_u8; 16]);
/// let forged = RegisteredHandle::new(nonce, "s".into(), "w".into(), "t".into(), "u".into());
/// assert_eq!(forged.clone(), forged);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RegisteredHandle {
    /// Unforgeable nonce.
    nonce: Uuid,
    /// Widget id snapshot at registration.
    descriptor_id: String,
    /// Session id snapshot at registration.
    session_id: String,
    /// Tenant snapshot at registration.
    tenant_id: String,
    /// User snapshot at registration.
    user_sub: String,
}

impl RegisteredHandle {
    /// Assemble a handle. Only the broker mints live ones; tests mint forged
    /// ones to prove they fail closed.
    /// # Examples
    /// ```
    /// use drishti::native_host::broker::RegisteredHandle;
    /// let nonce = lgwks_std::id::Uuid::from([1_u8; 16]);
    /// let handle = RegisteredHandle::new(nonce, "s1".into(), "w1".into(), "t1".into(), "u1".into());
    /// assert_eq!((handle.session_id(), handle.descriptor_id()), ("s1", "w1"));
    /// ```
    #[must_use]
    pub fn new(
        nonce: Uuid,
        session_id: String,
        descriptor_id: String,
        tenant_id: String,
        user_sub: String,
    ) -> Self {
        Self {
            nonce,
            session_id,
            descriptor_id,
            tenant_id,
            user_sub,
        }
    }

    /// Unforgeable nonce minted at widget registration.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::RegisteredHandle;
    /// let nonce = lgwks_std::id::Uuid::from([5_u8; 16]);
    /// let handle = RegisteredHandle::new(nonce, "s".into(), "w".into(), "t".into(), "u".into());
    /// assert_eq!(handle.nonce(), nonce);
    /// ```
    #[must_use]
    pub const fn nonce(&self) -> Uuid {
        self.nonce
    }

    /// Widget id the handle was minted for.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::RegisteredHandle;
    /// let nonce = lgwks_std::id::Uuid::from([1_u8; 16]);
    /// let handle = RegisteredHandle::new(nonce, "s".into(), "widget.a".into(), "t".into(), "u".into());
    /// assert_eq!(handle.descriptor_id(), "widget.a");
    /// ```
    #[must_use]
    pub fn descriptor_id(&self) -> &str {
        &self.descriptor_id
    }

    /// Session the handle was minted in.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::RegisteredHandle;
    /// let nonce = lgwks_std::id::Uuid::from([1_u8; 16]);
    /// let handle = RegisteredHandle::new(nonce, "s9".into(), "w".into(), "t".into(), "u".into());
    /// assert_eq!(handle.session_id(), "s9");
    /// ```
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Tenant the handle was minted for.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::RegisteredHandle;
    /// let nonce = lgwks_std::id::Uuid::from([1_u8; 16]);
    /// let handle = RegisteredHandle::new(nonce, "s".into(), "w".into(), "tenant-a".into(), "u".into());
    /// assert_eq!(handle.tenant_id(), "tenant-a");
    /// ```
    #[must_use]
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// User the handle acts for.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::RegisteredHandle;
    /// let nonce = lgwks_std::id::Uuid::from([1_u8; 16]);
    /// let handle = RegisteredHandle::new(nonce, "s".into(), "w".into(), "t".into(), "user-1".into());
    /// assert_eq!(handle.user_sub(), "user-1");
    /// ```
    #[must_use]
    pub fn user_sub(&self) -> &str {
        &self.user_sub
    }
}

/// Opaque handle issued to a verified ML peer.
///
/// Fields are private: only the broker mints live handles, and every reader
/// goes through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::broker::MlHandle;
/// let handle = MlHandle::new(lgwks_std::id::Uuid::from([2_u8; 16]), "t".into());
/// assert_eq!(handle.tenant_id(), "t");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MlHandle {
    /// Unforgeable nonce.
    nonce: Uuid,
    /// Tenant the peer is scoped to.
    tenant_id: String,
}

impl MlHandle {
    /// Assemble a handle. Only the broker mints live ones; tests mint forged
    /// ones to prove they fail closed.
    /// # Examples
    /// ```
    /// use drishti::native_host::broker::MlHandle;
    /// let nonce = lgwks_std::id::Uuid::from([2_u8; 16]);
    /// assert_ne!(MlHandle::new(nonce, "a".into()), MlHandle::new(nonce, "b".into()));
    /// ```
    #[must_use]
    pub fn new(nonce: Uuid, tenant_id: String) -> Self {
        Self { nonce, tenant_id }
    }

    /// Unforgeable nonce minted at ML peer registration.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::MlHandle;
    /// let nonce = lgwks_std::id::Uuid::from([2_u8; 16]);
    /// assert_eq!(MlHandle::new(nonce, "t".into()).nonce(), nonce);
    /// ```
    #[must_use]
    pub const fn nonce(&self) -> Uuid {
        self.nonce
    }

    /// Tenant the peer is scoped to.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::MlHandle;
    /// let handle = MlHandle::new(lgwks_std::id::Uuid::from([2_u8; 16]), "tenant-a".into());
    /// assert_eq!(handle.tenant_id(), "tenant-a");
    /// ```
    #[must_use]
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
}

/// Opaque handle issued to the command bar.
///
/// Fields are private: only the broker mints live handles, and every reader
/// goes through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::broker::IntentHandle;
/// let handle = IntentHandle::new(lgwks_std::id::Uuid::from([3_u8; 16]), "s".into(), "t".into(), "u".into());
/// assert_eq!(handle.user_sub(), "u");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct IntentHandle {
    /// Unforgeable nonce.
    nonce: Uuid,
    /// Session the bar is bound to.
    session_id: String,
    /// Tenant the bar is bound to.
    tenant_id: String,
    /// User the bar acts for.
    user_sub: String,
}

impl IntentHandle {
    /// Assemble a handle. Only the broker mints live ones; tests mint forged
    /// ones to prove they fail closed.
    /// # Examples
    /// ```
    /// use drishti::native_host::broker::IntentHandle;
    /// let nonce = lgwks_std::id::Uuid::from([3_u8; 16]);
    /// let handle = IntentHandle::new(nonce, "s".into(), "t".into(), "u".into());
    /// assert_eq!((handle.session_id(), handle.tenant_id()), ("s", "t"));
    /// ```
    #[must_use]
    pub fn new(nonce: Uuid, session_id: String, tenant_id: String, user_sub: String) -> Self {
        Self {
            nonce,
            session_id,
            tenant_id,
            user_sub,
        }
    }

    /// Unforgeable nonce minted at command-bar registration.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::IntentHandle;
    /// let nonce = lgwks_std::id::Uuid::from([3_u8; 16]);
    /// assert_eq!(IntentHandle::new(nonce, "s".into(), "t".into(), "u".into()).nonce(), nonce);
    /// ```
    #[must_use]
    pub const fn nonce(&self) -> Uuid {
        self.nonce
    }

    /// Tenant the bar is bound to.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::IntentHandle;
    /// let nonce = lgwks_std::id::Uuid::from([3_u8; 16]);
    /// assert_eq!(IntentHandle::new(nonce, "s".into(), "tenant-a".into(), "u".into()).tenant_id(), "tenant-a");
    /// ```
    #[must_use]
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// User the bar acts for.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::IntentHandle;
    /// let nonce = lgwks_std::id::Uuid::from([3_u8; 16]);
    /// assert_eq!(IntentHandle::new(nonce, "s".into(), "t".into(), "user-1".into()).user_sub(), "user-1");
    /// ```
    #[must_use]
    pub fn user_sub(&self) -> &str {
        &self.user_sub
    }

    /// Session the bar is bound to.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::IntentHandle;
    /// let nonce = lgwks_std::id::Uuid::from([3_u8; 16]);
    /// assert_eq!(IntentHandle::new(nonce, "s9".into(), "t".into(), "u".into()).session_id(), "s9");
    /// ```
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

/// Opaque subscription identity returned by `subscribe`. The inner nonce is
/// crate-visible so the registry can index by it; external callers use the
/// value opaquely.
/// # Examples
/// ```
/// use std::sync::Arc;
/// use drishti::cli::drive::{frozen_broker, session, widget_owner};
/// use drishti::native_host::broker::SignalListener;
/// use drishti::native_host::protocol::SignalPattern;
/// let (broker, _events, _open) = frozen_broker();
/// let mut digest = String::new();
/// let bound = session(&mut digest, "tenant-a")?;
/// let owner = widget_owner(&mut digest, &broker, &bound)?;
/// let listener: SignalListener = Arc::new(|envelope| drop(envelope));
/// let first = broker.subscribe(&owner, SignalPattern::new("a".into(), None), listener.clone())?;
/// let second = broker.subscribe(&owner, SignalPattern::new("a".into(), None), listener)?;
/// assert_ne!(first, second);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SubscriptionId(pub(crate) Uuid);

/// Opaque tap identity returned by `tap`. The inner nonce is crate-visible
/// so the registry can index by it; external callers use it opaquely.
/// # Examples
/// ```
/// use std::sync::Arc;
/// use drishti::cli::drive::{frozen_broker, session, widget_owner};
/// use drishti::native_host::broker::{Reach, TapListener};
/// let (broker, _events, _open) = frozen_broker();
/// let mut digest = String::new();
/// let bound = session(&mut digest, "tenant-a")?;
/// let owner = widget_owner(&mut digest, &broker, &bound)?;
/// let listener: TapListener = Arc::new(|event| drop(event));
/// let first = broker.tap(&owner, Reach::Own, listener.clone())?;
/// let second = broker.tap(&owner, Reach::Own, listener)?;
/// assert_ne!(first, second);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TapId(pub(crate) Uuid);

/// Read-only view of one active subscription. It names the subscription and
/// where it delivers, and carries no handle: revocation acts on the owner's
/// nonce, so a view that held the handle would let any reader in scope revoke
/// the owner's widget. Fields are crate-visible so the registry can snapshot
/// without an extra constructor; external readers go through the accessors.
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
/// broker.subscribe(&owner, SignalPattern::new("forms.**".into(), None), listener)?;
/// let views = broker.registered_subscriptions(&owner, Reach::Own)?;
/// assert_eq!(views.len(), 1);
/// assert_eq!(views[0].pattern().kind_glob(), "forms.**");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct Subscription {
    /// Subscription identity.
    pub(crate) id: SubscriptionId,
    /// Widget id of the owner (a name, never a credential).
    pub(crate) descriptor_id: String,
    /// Session this subscription delivers in.
    pub(crate) session_id: String,
    /// Tenant this subscription delivers in.
    pub(crate) tenant_id: String,
    /// Match pattern.
    pub(crate) pattern: SignalPattern,
}

impl Subscription {
    /// Identity `subscribe` returned for this subscription.
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
    /// let id = broker.subscribe(&owner, SignalPattern::new("a".into(), None), listener)?;
    /// let views = broker.registered_subscriptions(&owner, Reach::Own)?;
    /// assert_eq!(views[0].id(), id);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn id(&self) -> SubscriptionId {
        self.id
    }

    /// Widget id of the owning widget.
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
    /// broker.subscribe(&owner, SignalPattern::new("a".into(), None), listener)?;
    /// let views = broker.registered_subscriptions(&owner, Reach::Own)?;
    /// assert_eq!(views[0].descriptor_id(), "widget.owner");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn descriptor_id(&self) -> &str {
        &self.descriptor_id
    }

    /// Pattern the subscription matches envelopes against.
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
    /// let pattern = SignalPattern::new("a.*".into(), None);
    /// broker.subscribe(&owner, pattern.clone(), listener)?;
    /// let views = broker.registered_subscriptions(&owner, Reach::Own)?;
    /// assert_eq!(views[0].pattern(), &pattern);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn pattern(&self) -> &SignalPattern {
        &self.pattern
    }

    /// Session this subscription delivers in.
    /// # Examples
    /// ```
    /// use std::sync::Arc;
    /// use drishti::cli::drive::{SESSION_ID, frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::{Reach, SignalListener};
    /// use drishti::native_host::protocol::SignalPattern;
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &bound)?;
    /// let listener: SignalListener = Arc::new(|envelope| drop(envelope));
    /// broker.subscribe(&owner, SignalPattern::new("a".into(), None), listener)?;
    /// let views = broker.registered_subscriptions(&owner, Reach::Own)?;
    /// assert_eq!(views[0].session_id(), SESSION_ID);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Tenant this subscription delivers in.
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
    /// broker.subscribe(&owner, SignalPattern::new("a".into(), None), listener)?;
    /// let views = broker.registered_subscriptions(&owner, Reach::Own)?;
    /// assert_eq!(views[0].tenant_id(), "tenant-a");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
}

/// Rich telemetry event emitted after every fan-out.
///
/// Fields are private: the broker builds events and tap readers consume them
/// through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::broker::TapEvent;
/// # use drishti::native_host::protocol::{JsonValue, SignalEnvelope};
/// # fn envelope() -> Result<SignalEnvelope, Box<dyn std::error::Error>> {
/// #     let (broker, _events, _open) = drishti::cli::drive::frozen_broker();
/// #     let mut digest = String::new();
/// #     let bound = drishti::cli::drive::session(&mut digest, "tenant-a")?;
/// #     let owner = drishti::cli::drive::widget_owner(&mut digest, &broker, &bound)?;
/// #     Ok(broker.emit(&owner, "a", JsonValue::Null)?)
/// # }
/// let event = TapEvent::new(envelope()?, 2, 1_500, 0, 1_000);
/// assert_eq!((event.matched_subscriber_count(), event.timestamp_ms()), (2, 1_000));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TapEvent {
    /// Routed envelope with the payload redacted to null.
    envelope: SignalEnvelope,
    /// Subscribers matched.
    matched_subscriber_count: usize,
    /// Fan-out wall time in nanoseconds.
    fanout_duration_ns: u64,
    /// Emits throttled by the token bucket since the last success.
    rate_limited_count: u64,
    /// Event time in millisecond time.
    timestamp_ms: u64,
}

impl TapEvent {
    /// Assemble a tap event. Only the broker builds these.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::TapEvent;
    /// use drishti::native_host::protocol::JsonValue;
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &bound)?;
    /// let sent = broker.emit(&owner, "a.b", JsonValue::Null)?;
    /// let event = TapEvent::new(sent, 3, 42, 1, 1_000);
    /// assert_eq!(event.envelope().kind(), "a.b");
    /// assert_eq!((event.fanout_duration_ns(), event.rate_limited_count()), (42, 1));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new(
        envelope: SignalEnvelope,
        matched_subscriber_count: usize,
        fanout_duration_ns: u64,
        rate_limited_count: u64,
        timestamp_ms: u64,
    ) -> Self {
        Self {
            envelope,
            matched_subscriber_count,
            fanout_duration_ns,
            rate_limited_count,
            timestamp_ms,
        }
    }

    /// Routed envelope with the payload redacted to null.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::TapEvent;
    /// use drishti::native_host::protocol::JsonValue;
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &bound)?;
    /// let sent = broker.emit(&owner, "a.b", JsonValue::Bool(true))?;
    /// let event = TapEvent::new(sent.clone(), 0, 0, 0, 0);
    /// assert_eq!(event.envelope(), &sent);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn envelope(&self) -> &SignalEnvelope {
        &self.envelope
    }

    /// Number of subscribers the fan-out reached.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::TapEvent;
    /// use drishti::native_host::protocol::JsonValue;
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &bound)?;
    /// let sent = broker.emit(&owner, "a", JsonValue::Null)?;
    /// assert_eq!(TapEvent::new(sent, 7, 0, 0, 0).matched_subscriber_count(), 7);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn matched_subscriber_count(&self) -> usize {
        self.matched_subscriber_count
    }

    /// Fan-out wall time in nanoseconds.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::TapEvent;
    /// use drishti::native_host::protocol::JsonValue;
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &bound)?;
    /// let sent = broker.emit(&owner, "a", JsonValue::Null)?;
    /// assert_eq!(TapEvent::new(sent, 0, 900, 0, 0).fanout_duration_ns(), 900);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn fanout_duration_ns(&self) -> u64 {
        self.fanout_duration_ns
    }

    /// Emits throttled by the token bucket since the last success.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::TapEvent;
    /// use drishti::native_host::protocol::JsonValue;
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &bound)?;
    /// let sent = broker.emit(&owner, "a", JsonValue::Null)?;
    /// assert_eq!(TapEvent::new(sent, 0, 0, 5, 0).rate_limited_count(), 5);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn rate_limited_count(&self) -> u64 {
        self.rate_limited_count
    }

    /// Injected-clock reading, in milliseconds, taken when the route fired.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::TapEvent;
    /// use drishti::native_host::protocol::JsonValue;
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &bound)?;
    /// let sent = broker.emit(&owner, "a", JsonValue::Null)?;
    /// assert_eq!(TapEvent::new(sent, 0, 0, 0, 1_000).timestamp_ms(), 1_000);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn timestamp_ms(&self) -> u64 {
        self.timestamp_ms
    }
}
