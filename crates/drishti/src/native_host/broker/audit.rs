//! Audit vocabulary and the one scope filter of the broker.
//!
//! Issues drishti#8, drishti#9, and drishti#10 meet here. Every broker decision becomes one typed
//! [`AuditEvent`] (caller, action, requested reach, decision, and the missing
//! permission on a denial). Every inspector reads through [`Visibility`], the
//! single filter that binds a read to the caller's [`Scope`] (session plus
//! tenant) unless the injected grant store allows [`Reach::AllTenants`].
//! Delivery uses the same [`same_scope`] rule, so reads and fan-out never
//! drift apart.

use std::fmt;

use super::super::capability::CapabilityError;
use super::{BrokerError, IntentHandle, RegisteredHandle};
use crate::canvas_protocol::{Capability, SessionInvariant};

/// Session plus tenant: the scope an authenticated caller is bound to.
/// # Examples
/// ```
/// use drishti::native_host::broker::Scope;
/// assert_ne!(Scope::new("tenant-a", "s1"), Scope::new("tenant-b", "s1"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    /// Tenant the caller belongs to.
    tenant_id: String,
    /// Session the caller acts in.
    session_id: String,
}

impl Scope {
    /// Bind a scope to a tenant and a session. Readers of tenant-keyed stores
    /// (the tape reader) take one, so no read can name a session alone.
    /// # Examples
    /// ```
    /// use drishti::native_host::broker::Scope;
    /// let scope = Scope::new("tenant-a", "s1");
    /// assert_eq!((scope.tenant_id(), scope.session_id()), ("tenant-a", "s1"));
    /// ```
    #[must_use]
    pub fn new(tenant_id: &str, session_id: &str) -> Self {
        Self {
            tenant_id: tenant_id.to_owned(),
            session_id: session_id.to_owned(),
        }
    }

    /// Tenant the caller belongs to.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::Scope;
    /// assert_eq!(Scope::new("tenant-a", "s1").tenant_id(), "tenant-a");
    /// ```
    #[must_use]
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// Session the caller acts in.
    /// # Examples
    /// ```
    /// # use drishti::native_host::broker::Scope;
    /// assert_eq!(Scope::new("tenant-a", "s1").session_id(), "s1");
    /// ```
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// True when a record in `session_id` and `tenant_id` lies in this scope
    /// (both must match; see [`same_scope`]).
    fn holds(&self, session_id: &str, tenant_id: &str) -> bool {
        same_scope(&self.session_id, &self.tenant_id, session_id, tenant_id)
    }
}

/// The one scope rule: both session and tenant must match. Delivery and every
/// inspector call this, so a session id shared by two tenants never joins them.
pub(super) fn same_scope(session_a: &str, tenant_a: &str, session_b: &str, tenant_b: &str) -> bool {
    session_a == session_b && tenant_a == tenant_b
}

/// Who acted: the scope plus the user the session acts for. Only the broker
/// builds one, so read it off a captured [`AuditEvent`].
/// # Examples
/// ```
/// use drishti::cli::drive::{SESSION_ID, USER_SUB, captured, frozen_broker, session};
/// let (broker, events, _open) = frozen_broker();
/// let who = session(&mut String::new(), "tenant-a")?;
/// assert!(broker.register_command_bar(who).is_err());
/// let event = captured(&events).remove(0);
/// let caller = event.caller();
/// assert_eq!((caller.scope().session_id(), caller.user_sub()), (SESSION_ID, USER_SUB));
/// # Ok::<(), drishti::cli::Failure>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    /// Scope the caller is bound to.
    scope: Scope,
    /// User subject the session acts for.
    user_sub: String,
}

impl Caller {
    /// The caller behind an authenticated session.
    pub(super) fn of(session: &SessionInvariant) -> Self {
        Self::claimed(
            session.tenant_id(),
            session.session_id(),
            session.user_sub(),
        )
    }

    /// A caller known only by what it claims (a forged or retired handle).
    fn claimed(tenant_id: &str, session_id: &str, user_sub: &str) -> Self {
        Self {
            scope: Scope::new(tenant_id, session_id),
            user_sub: user_sub.to_owned(),
        }
    }

    /// The caller a widget handle claims.
    pub(super) fn from_widget(handle: &RegisteredHandle) -> Self {
        Self::claimed(handle.tenant_id(), handle.session_id(), handle.user_sub())
    }

    /// The caller a command-bar handle claims.
    pub(super) fn from_bar(handle: &IntentHandle) -> Self {
        Self::claimed(handle.tenant_id(), handle.session_id(), handle.user_sub())
    }

    /// The caller a peer handle claims: only the tenant is known.
    pub(super) fn from_tenant(tenant_id: &str) -> Self {
        Self::claimed(tenant_id, "", "")
    }

    /// Scope the caller is bound to.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{captured, frozen_broker, session};
    /// let (broker, events, _open) = frozen_broker();
    /// let who = session(&mut String::new(), "tenant-a")?;
    /// assert!(broker.register_command_bar(who).is_err());
    /// let event = captured(&events).remove(0);
    /// assert_eq!(event.caller().scope().tenant_id(), "tenant-a");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn scope(&self) -> &Scope {
        &self.scope
    }

    /// Subject id of the user behind the session; empty when the caller is
    /// known only by tenant (a peer handle).
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{USER_SUB, captured, frozen_broker, session};
    /// let (broker, events, _open) = frozen_broker();
    /// let who = session(&mut String::new(), "tenant-a")?;
    /// assert!(broker.register_command_bar(who).is_err());
    /// assert_eq!(captured(&events).remove(0).caller().user_sub(), USER_SUB);
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub fn user_sub(&self) -> &str {
        &self.user_sub
    }
}

/// What the injected grant store is asked about. Kernel capabilities stay in
/// [`Capability`] (schema-locked); cross-tenant inspection is broker-local.
/// # Examples
/// ```
/// use drishti::canvas_protocol::Capability;
/// use drishti::native_host::broker::Permission;
/// assert_eq!(Permission::Capability(Capability::IntentEmit).to_string(), "intent.emit");
/// assert_eq!(Permission::CrossTenantInspect.to_string(), "inspect.cross_tenant");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Permission {
    /// A kernel capability such as `intent.emit`.
    Capability(Capability),
    /// Read subscriptions and taps beyond the caller's own tenant.
    CrossTenantInspect,
}

impl fmt::Display for Permission {
    /// The capability wire string, or `inspect.cross_tenant`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Capability(ref capability) => write!(formatter, "{capability}"),
            Self::CrossTenantInspect => formatter.write_str("inspect.cross_tenant"),
        }
    }
}

impl Permission {
    /// The typed refusal for this permission not being granted.
    pub(super) fn refusal(self) -> BrokerError {
        match self {
            Self::Capability(capability) => {
                BrokerError::CapabilityDenied(CapabilityError::NotGranted(capability))
            }
            Self::CrossTenantInspect => BrokerError::CrossTenantDenied,
        }
    }
}

/// How far an inspector read reaches.
/// # Examples
/// ```
/// use drishti::native_host::broker::Reach;
/// assert_ne!(Reach::Own, Reach::AllTenants);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reach {
    /// The caller's own session and tenant only.
    Own,
    /// Every tenant; needs [`Permission::CrossTenantInspect`].
    AllTenants,
}

/// The kinds of registry entry an audit event names.
/// # Examples
/// ```
/// use drishti::native_host::broker::Subject;
/// assert_ne!(Subject::Widget("a".into()), Subject::Widget("b".into()));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Subject {
    /// A widget registration, by widget id.
    Widget(String),
    /// An ML peer registration.
    MlPeer,
    /// The command-bar registration.
    CommandBar,
    /// A widget subscription.
    Subscription,
    /// A tap listener.
    Tap,
}

/// The emit paths a refusal can come from. The audit event names the path,
/// never the caller's kind or payload, so a hostile emitter cannot grow the
/// trail with its own bytes.
/// # Examples
/// ```
/// use drishti::native_host::broker::Emitter;
/// assert_ne!(Emitter::Widget, Emitter::Prediction);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Emitter {
    /// `emit`: a registered widget.
    Widget,
    /// `emit_intent`: the command bar.
    Intent,
    /// `emit_prediction`: a verified ML peer.
    Prediction,
}

/// Every API that enumerates broker state for a caller. A new inspector adds a
/// variant here and an entry in [`Inspector::ALL`]; the suite iterates `ALL`.
/// # Examples
/// ```
/// use drishti::native_host::broker::Inspector;
/// assert!(Inspector::ALL.contains(&Inspector::Taps));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Inspector {
    /// `registered_subscriptions`: the subscription registry.
    Subscriptions,
    /// `tap`: the live signal stream.
    Taps,
}

impl Inspector {
    /// Every inspector, for suites that must cover each one.
    /// # Examples
    /// ```
    /// use drishti::native_host::broker::Inspector;
    /// assert_eq!(Inspector::ALL, [Inspector::Subscriptions, Inspector::Taps]);
    /// ```
    pub const ALL: [Self; 2] = [Self::Subscriptions, Self::Taps];
}

/// What a decision was about.
/// # Examples
/// ```
/// use drishti::native_host::broker::{AuditAction, Subject};
/// assert_ne!(AuditAction::Register(Subject::MlPeer), AuditAction::Revoke(Subject::MlPeer));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuditAction {
    /// An envelope was stamped, delivered, and routed.
    Route {
        /// Identity of the routed envelope.
        signal_id: String,
        /// Signal kind of the routed envelope.
        kind: String,
        /// Source that emitted it.
        source_id: String,
    },
    /// A registration or subscription was requested.
    Register(Subject),
    /// A registration, subscription, or tap was revoked.
    Revoke(Subject),
    /// An inspector read was requested.
    Inspect(Inspector),
    /// An emit was refused (an admitted emit is audited as `Route`).
    Emit(Emitter),
}

/// The outcome of a decision.
/// # Examples
/// ```
/// use drishti::native_host::broker::{BrokerError, Decision};
/// assert_ne!(Decision::Allowed, Decision::Denied(BrokerError::RateLimited));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Decision {
    /// The action was admitted.
    Allowed,
    /// The action was refused, with the typed reason.
    Denied(BrokerError),
}

impl Decision {
    /// The decision a broker result records.
    pub(super) fn of<T>(result: &Result<T, BrokerError>) -> Self {
        match result.as_ref().err() {
            None => Self::Allowed,
            Some(error) => Self::Denied(error.clone()),
        }
    }

    /// The permission a denial lacked, when the denial was a missing one.
    /// # Examples
    /// ```
    /// use drishti::native_host::broker::{BrokerError, Decision, Permission};
    /// let cross = Decision::Denied(BrokerError::CrossTenantDenied);
    /// assert_eq!(cross.missing(), Some(Permission::CrossTenantInspect));
    /// // A denial for any other reason, and an admission, miss nothing.
    /// assert_eq!(Decision::Denied(BrokerError::RateLimited).missing(), None);
    /// assert_eq!(Decision::Allowed.missing(), None);
    /// ```
    #[must_use]
    pub fn missing(&self) -> Option<Permission> {
        match *self {
            Self::Denied(BrokerError::CapabilityDenied(CapabilityError::NotGranted(
                ref capability,
            ))) => Some(Permission::Capability(capability.clone())),
            Self::Denied(BrokerError::CrossTenantDenied) => Some(Permission::CrossTenantInspect),
            Self::Denied(_) | Self::Allowed => None,
        }
    }
}

/// One audited decision. `seq` orders events from the broker; the sink may see
/// concurrent callers interleave, never one caller out of order.
/// # Examples
/// ```
/// use drishti::cli::drive::{captured, frozen_broker, session};
/// use drishti::native_host::broker::{AuditAction, Decision, Subject};
/// let (broker, events, _open) = frozen_broker();
/// let who = session(&mut String::new(), "tenant-a")?;
/// assert!(broker.register_command_bar(who).is_err());
/// let event = captured(&events).remove(0);
/// assert_eq!(event.action(), &AuditAction::Register(Subject::CommandBar));
/// assert!(matches!(event.decision(), Decision::Denied(_)));
/// # Ok::<(), drishti::cli::Failure>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AuditEvent {
    /// Broker-wide ordinal, assigned when the event is published.
    seq: u64,
    /// Injected-clock millisecond time of the decision.
    at_ms: u64,
    /// Who acted.
    caller: Caller,
    /// What was decided.
    action: AuditAction,
    /// The reach the caller requested.
    reach: Reach,
    /// The outcome.
    decision: Decision,
}

impl AuditEvent {
    /// Assemble an event. Only the broker builds these.
    pub(super) const fn new(
        seq: u64,
        at_ms: u64,
        caller: Caller,
        action: AuditAction,
        reach: Reach,
        decision: Decision,
    ) -> Self {
        Self {
            seq,
            at_ms,
            caller,
            action,
            reach,
            decision,
        }
    }

    /// Broker-wide ordinal, assigned when the event is published.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{captured, frozen_broker, session};
    /// let (broker, events, _open) = frozen_broker();
    /// let who = session(&mut String::new(), "tenant-a")?;
    /// assert!(broker.register_command_bar(who.clone()).is_err());
    /// assert!(broker.register_command_bar(who).is_err());
    /// let seqs: Vec<u64> = captured(&events).iter().map(|event| event.seq()).collect();
    /// assert_eq!(seqs, [0, 1]);
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn seq(&self) -> u64 {
        self.seq
    }

    /// Injected-clock millisecond time of the decision.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{captured, frozen_broker, session};
    /// let (broker, events, _open) = frozen_broker();
    /// let who = session(&mut String::new(), "tenant-a")?;
    /// assert!(broker.register_command_bar(who).is_err());
    /// // The frozen broker's clock reads 1,000 ms.
    /// assert_eq!(captured(&events).remove(0).at_ms(), 1_000);
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn at_ms(&self) -> u64 {
        self.at_ms
    }

    /// Who acted.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{captured, frozen_broker, session};
    /// let (broker, events, _open) = frozen_broker();
    /// let who = session(&mut String::new(), "tenant-b")?;
    /// assert!(broker.register_command_bar(who).is_err());
    /// assert_eq!(captured(&events).remove(0).caller().scope().tenant_id(), "tenant-b");
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn caller(&self) -> &Caller {
        &self.caller
    }

    /// What was decided.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{captured, frozen_broker, session};
    /// use drishti::native_host::broker::{AuditAction, Subject};
    /// let (broker, events, _open) = frozen_broker();
    /// let who = session(&mut String::new(), "tenant-a")?;
    /// assert!(broker.register_command_bar(who).is_err());
    /// let event = captured(&events).remove(0);
    /// assert_eq!(event.action(), &AuditAction::Register(Subject::CommandBar));
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn action(&self) -> &AuditAction {
        &self.action
    }

    /// The reach the caller requested.
    /// # Examples
    /// ```
    /// use drishti::cli::drive::{captured, frozen_broker, session};
    /// use drishti::native_host::broker::Reach;
    /// let (broker, events, _open) = frozen_broker();
    /// let who = session(&mut String::new(), "tenant-a")?;
    /// assert!(broker.register_command_bar(who).is_err());
    /// assert_eq!(captured(&events).remove(0).reach(), Reach::Own);
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn reach(&self) -> Reach {
        self.reach
    }

    /// The outcome.
    /// # Examples
    /// ```
    /// use drishti::canvas_protocol::Capability;
    /// use drishti::cli::drive::{captured, frozen_broker, session};
    /// use drishti::native_host::broker::Permission;
    /// let (broker, events, _open) = frozen_broker();
    /// let who = session(&mut String::new(), "tenant-a")?;
    /// assert!(broker.register_command_bar(who).is_err());
    /// // The closed grant store refused `intent.emit`.
    /// let missing = captured(&events).remove(0).decision().missing();
    /// assert_eq!(missing, Some(Permission::Capability(Capability::IntentEmit)));
    /// # Ok::<(), drishti::cli::Failure>(())
    /// ```
    #[must_use]
    pub const fn decision(&self) -> &Decision {
        &self.decision
    }
}

/// The one filter every inspector reads through: a caller, the reach it asked
/// for, and the rule that decides which records it may see.
#[derive(Clone)]
pub(super) struct Visibility {
    /// Resolved from the broker's own registration, never from the request.
    caller: Caller,
    /// Reach already checked against the grant store.
    reach: Reach,
}

impl Visibility {
    /// Bind a checked reach to a resolved caller.
    pub(super) const fn new(caller: Caller, reach: Reach) -> Self {
        Self { caller, reach }
    }

    /// The resolved caller.
    pub(super) const fn caller(&self) -> &Caller {
        &self.caller
    }

    /// True when a record in `session_id` and `tenant_id` is visible.
    pub(super) fn admits(&self, session_id: &str, tenant_id: &str) -> bool {
        matches!(self.reach, Reach::AllTenants) || self.caller.scope.holds(session_id, tenant_id)
    }
}
