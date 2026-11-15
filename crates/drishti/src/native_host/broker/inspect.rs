//! Scoped inspectors and owner-checked tap revocation (issues drishti#8 and drishti#10).
//!
//! Every inspector resolves its caller from the broker's own registration
//! (found by the handle's nonce, never trusted from the handle's fields),
//! asks the grant store before a wide reach, reads through the one
//! [`Visibility`] filter, and audits the attempt with the reach it asked for.

use std::sync::Arc;

use lgwks_std::id::Uuid;

use super::super::ensure;
use super::audit::Visibility;
use super::limits::{tenant_count, within};
use super::{
    AuditAction, Broker, BrokerError, BrokerState, Caller, Decision, Inspector, Limit, Permission,
    Reach, RegisteredHandle, StoredTap, Subject, Subscription, SubscriptionId, TapEvent, TapId,
    TapListener,
};

/// The caller behind `handle`: the broker's own registration when the nonce
/// is live, otherwise what the handle claims (a forged or retired handle).
pub(super) fn caller_of(state: &BrokerState, handle: &RegisteredHandle) -> Caller {
    match state.widgets.get(&handle.nonce()) {
        Some(held) => Caller::of(&held.session),
        None => Caller::from_widget(handle),
    }
}

impl Broker {
    /// Resolve the caller from its registration and, for a wide reach, ask
    /// the grant store (outside the lock) for the cross-tenant permission.
    fn authorize(
        &self,
        caller: &RegisteredHandle,
        reach: Reach,
    ) -> (Caller, Result<Visibility, BrokerError>) {
        let resolved = self
            .lock()
            .widgets
            .get(&caller.nonce())
            .map(|held| held.session.clone());
        let Some(session) = resolved else {
            return (Caller::from_widget(caller), Err(BrokerError::InvalidHandle));
        };
        let who = Caller::of(&session);
        let access = match reach {
            Reach::Own => Ok(()),
            Reach::AllTenants => self.require(&session, Permission::CrossTenantInspect),
        };
        (who.clone(), access.map(|()| Visibility::new(who, reach)))
    }

    /// Audit one refusal as [`Decision::Denied`] (own reach).
    pub(super) fn audit_denial(&self, caller: Caller, action: AuditAction, error: &BrokerError) {
        self.publish(caller, action, Reach::Own, Decision::Denied(error.clone()));
    }

    /// Audit a refusal the broker did not take itself (a host module that
    /// refused before reaching the broker), as `handle`'s caller.
    pub(crate) fn deny_handle(
        &self,
        handle: &RegisteredHandle,
        action: AuditAction,
        error: &BrokerError,
    ) {
        let who = caller_of(&self.lock(), handle);
        self.audit_denial(who, action, error);
    }

    /// Snapshot of the subscriptions `caller` may see: its own session and
    /// tenant, or every tenant with the cross-tenant grant. Delivery closures
    /// and the owner's handle stay inside; the view carries id, widget id,
    /// session, tenant, and pattern.
    ///
    /// # Errors
    ///
    /// [`BrokerError::InvalidHandle`] for an unknown caller and
    /// [`BrokerError::CrossTenantDenied`] for a wide reach without the grant;
    /// every attempt is audited.
    /// # Examples
    /// ```
    /// use std::sync::atomic::Ordering;
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::{BrokerError, Reach};
    /// let (broker, _events, open) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &bound)?;
    /// assert!(broker.registered_subscriptions(&owner, Reach::Own)?.is_empty());
    /// // A wide reach needs the cross-tenant grant from the grant store.
    /// let wide = broker.registered_subscriptions(&owner, Reach::AllTenants);
    /// assert_eq!(wide.err(), Some(BrokerError::CrossTenantDenied));
    /// open.store(true, Ordering::SeqCst);
    /// assert!(broker.registered_subscriptions(&owner, Reach::AllTenants).is_ok());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn registered_subscriptions(
        &self,
        caller: &RegisteredHandle,
        reach: Reach,
    ) -> Result<Vec<Subscription>, BrokerError> {
        let (who, access) = self.authorize(caller, reach);
        let outcome = access.map(|view| {
            self.lock()
                .subscriptions
                .iter()
                .filter(|entry| {
                    view.admits(entry.1.handle.session_id(), entry.1.handle.tenant_id())
                })
                .map(|(id, sub)| Subscription {
                    id: SubscriptionId(*id),
                    descriptor_id: sub.handle.descriptor_id().to_owned(),
                    session_id: sub.handle.session_id().to_owned(),
                    tenant_id: sub.handle.tenant_id().to_owned(),
                    pattern: sub.pattern.clone(),
                })
                .collect()
        });
        let action = AuditAction::Inspect(Inspector::Subscriptions);
        self.decide(who, action, reach, outcome)
    }

    /// Observe the tap stream within the caller's scope (or every tenant
    /// with the cross-tenant grant). Tap payloads are always redacted to
    /// null, and the tap is cancelled when its owner unregisters.
    ///
    /// # Errors
    ///
    /// [`BrokerError::InvalidHandle`], [`BrokerError::CrossTenantDenied`] or
    /// [`BrokerError::LimitExceeded`]; every attempt is audited.
    /// # Examples
    /// ```
    /// use std::sync::Arc;
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::{Reach, TapListener};
    /// use drishti::native_host::protocol::JsonValue;
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &bound)?;
    /// let listener: TapListener = Arc::new(|event| drop(event));
    /// broker.tap(&owner, Reach::Own, listener)?;
    /// // Emitting with a live tap still succeeds; the tap sees a redacted copy.
    /// assert!(broker.emit(&owner, "forms.submitted", JsonValue::Null).is_ok());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn tap(
        &self,
        caller: &RegisteredHandle,
        reach: Reach,
        listener: TapListener,
    ) -> Result<TapId, BrokerError> {
        let (who, access) = self.authorize(caller, reach);
        let outcome = access.and_then(|view| self.admit_tap(caller.nonce(), view, listener));
        self.decide(who, AuditAction::Inspect(Inspector::Taps), reach, outcome)
    }

    /// Admission for [`Broker::tap`]: bounds, then a listener that only hears
    /// what the visibility admits.
    fn admit_tap(
        &self,
        owner: Uuid,
        view: Visibility,
        listener: TapListener,
    ) -> Result<TapId, BrokerError> {
        let id = (self.new_id)();
        let tenant = view.caller().scope().tenant_id().to_owned();
        let scoped: TapListener = Arc::new(move |event: TapEvent| {
            let seen = event.envelope().session_invariant();
            if view.admits(seen.session_id(), seen.tenant_id()) {
                listener(event);
            }
        });
        let mut state = self.lock();
        ensure(state.widgets.contains_key(&owner), || {
            BrokerError::InvalidHandle
        })?;
        within(state.taps.len(), Limit::Taps)?;
        let owners = state.taps.values().map(|held| held.tenant.as_str());
        within(tenant_count(owners, &tenant), Limit::TapsPerTenant)?;
        state.taps.insert(
            id,
            StoredTap {
                owner,
                tenant,
                listener: scoped,
            },
        );
        Ok(TapId(id))
    }

    /// Remove a tap listener. Only the owning widget's handle acts; a
    /// stranger and an unknown id are both audited as a denial.
    ///
    /// # Errors
    ///
    /// [`BrokerError::InvalidHandle`] for an unknown id or a non-owner.
    /// # Examples
    /// ```
    /// use std::sync::Arc;
    /// use drishti::cli::drive::{frozen_broker, session, widget_owner};
    /// use drishti::native_host::broker::{BrokerError, Reach, TapListener};
    /// let (broker, _events, _open) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a")?;
    /// let owner = widget_owner(&mut digest, &broker, &bound)?;
    /// let listener: TapListener = Arc::new(|event| drop(event));
    /// let id = broker.tap(&owner, Reach::Own, listener)?;
    /// broker.untap(&owner, &id)?;
    /// assert_eq!(broker.untap(&owner, &id), Err(BrokerError::InvalidHandle));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn untap(&self, handle: &RegisteredHandle, id: &TapId) -> Result<(), BrokerError> {
        let mut state = self.lock();
        let who = caller_of(&state, handle);
        let owned = state
            .taps
            .get(&id.0)
            .is_some_and(|held| held.owner == handle.nonce());
        let outcome = if owned {
            state.taps.remove(&id.0);
            Ok(())
        } else {
            Err(BrokerError::InvalidHandle)
        };
        drop(state);
        self.decide(who, AuditAction::Revoke(Subject::Tap), Reach::Own, outcome)
    }
}
