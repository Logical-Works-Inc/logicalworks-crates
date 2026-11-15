//! Broker lifecycle of `api-drive-host`: register, subscribe, emit, revoke.
//!
//! Each step below calls one lifecycle operation of the [`Broker`] and prints
//! the digest lines of its result, then the audited outcome: the newest audit
//! event the call left, as one `audited.<step>=` JSON line. A user of the
//! binary reads the whole life of a widget subscription, an ML peer and a
//! command bar, each from its registration to its revocation, together with
//! the audited decision of every call. A forged handle after a revocation is a
//! negative control: the broker must refuse it and audit the refusal.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use drishti::canvas_protocol::{Capability, SessionInvariant};
use drishti::cli::Failure;
use drishti::cli::drive::{Events, ensure, must, note, refused, uuid};
use drishti::native_host::broker::{
    Broker, BrokerError, IntentHandle, MlHandle, RegisteredHandle, SignalListener, SignedMlGrant,
    SubscriptionId,
};
use drishti::native_host::protocol::{JsonValue, PartialIntent, SignalEnvelope, SignalPattern};

use super::audit_drive::outcome;

/// Subscribe `handle` to every kind and print the audited registration.
pub(super) fn subscribe(
    digest: &mut String,
    broker: &Broker,
    events: &Events,
    handle: &RegisteredHandle,
) -> Result<SubscriptionId, Failure> {
    let listener: SignalListener = Arc::new(|_: SignalEnvelope| {});
    let id = must(
        digest,
        "subscription",
        broker.subscribe(
            handle,
            SignalPattern::new(String::from("**"), None),
            listener,
        ),
    )?;
    outcome(digest, "subscribe", events)?;
    Ok(id)
}

/// Revoke a subscription and print the audited revocation.
pub(super) fn unsubscribe(
    digest: &mut String,
    broker: &Broker,
    events: &Events,
    handle: &RegisteredHandle,
    subscription: &SubscriptionId,
) -> Result<(), Failure> {
    must(
        digest,
        "unsubscribed",
        broker.unsubscribe(handle, subscription),
    )?;
    outcome(digest, "unsubscribe", events)
}

/// Revoke a widget and print the audited revocation.
pub(super) fn unregister(
    digest: &mut String,
    broker: &Broker,
    events: &Events,
    handle: &RegisteredHandle,
) -> Result<(), Failure> {
    broker.unregister(handle);
    outcome(digest, "unregister", events)
}

/// Register an ML peer under a signed grant, emit a prediction, revoke the
/// peer, then replay a forged peer handle. Each call prints its audited outcome.
pub(super) fn ml_peer(
    digest: &mut String,
    broker: &Broker,
    events: &Events,
    session: &SessionInvariant,
) -> Result<(), Failure> {
    let grant = SignedMlGrant::new(
        uuid(digest, "grant-id", "99999999-9999-9999-9999-999999999999")?,
        String::from("tenant-drive"),
        vec![Capability::TapeRead],
        9_999_999_999,
        1_000_000_000,
        vec![7, 8, 9],
    );
    note(
        digest,
        "grant",
        &(
            grant.grant_id(),
            grant.tenant_id(),
            grant.capabilities().len(),
            grant.issued_at_ms(),
            grant.expires_at_ms(),
            grant.signature().len(),
        ),
    )?;
    let ml: MlHandle = must(
        digest,
        "ml-peer",
        broker.register_ml_peer(grant, session.clone()),
    )?;
    outcome(digest, "register_ml_peer", events)?;
    note(digest, "ml", &(ml.nonce(), ml.tenant_id()))?;
    let prediction = must(
        digest,
        "prediction",
        broker.emit_prediction(&ml, "ml.result", JsonValue::Null),
    )?;
    outcome(digest, "emit_prediction", events)?;
    note(digest, "ml-signal", &prediction.signal_id())?;
    broker.unregister_ml_peer(&ml);
    outcome(digest, "unregister_ml_peer", events)?;
    let forged_ml = MlHandle::new(
        uuid(
            digest,
            "forged-nonce",
            "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa0",
        )?,
        String::from("tenant-drive"),
    );
    refused(
        broker.emit_prediction(&forged_ml, "ml.forged", JsonValue::Null),
        "forged ML handle must fail",
    )?;
    outcome(digest, "emit_prediction_forged", events)
}

/// Refuse a command bar without its capability, register one once the grant
/// opens, emit an intent through it, then revoke it. `open` is the grant switch
/// of the frozen broker. Each call prints its audited outcome.
pub(super) fn command_bar(
    digest: &mut String,
    broker: &Broker,
    events: &Events,
    open: &AtomicBool,
    session: &SessionInvariant,
) -> Result<(), Failure> {
    open.store(false, Ordering::SeqCst);
    let denied = broker.register_command_bar(session.clone());
    ensure(
        matches!(denied, Err(BrokerError::CapabilityDenied(_))),
        "an ungranted command bar must be refused",
    )?;
    outcome(digest, "register_command_bar_denied", events)?;
    open.store(true, Ordering::SeqCst);
    let bar: IntentHandle = must(digest, "bar", broker.register_command_bar(session.clone()))?;
    outcome(digest, "register_command_bar", events)?;
    note(
        digest,
        "bar",
        &(
            bar.nonce(),
            bar.tenant_id(),
            bar.user_sub(),
            bar.session_id(),
        ),
    )?;
    let partial = PartialIntent::new(
        String::from("intent.drive"),
        JsonValue::Null,
        String::from("drive"),
    );
    note(
        digest,
        "partial",
        &(partial.kind(), partial.payload(), partial.text()),
    )?;
    let intent = must(digest, "intent-emitted", broker.emit_intent(&bar, partial))?;
    outcome(digest, "emit_intent", events)?;
    note(digest, "intent-signal", &intent.signal_id())?;
    broker.unregister_command_bar(&bar);
    outcome(digest, "unregister_command_bar", events)
}
