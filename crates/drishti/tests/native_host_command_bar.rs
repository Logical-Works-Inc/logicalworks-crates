//! Command-bar flows: parse, gate, emit, fan-out.
//!
//! Counterpart of `CanvasCommandBarTests/CommandBarTests` and the abuse
//! command-bar cases (kernel#850 row 15): view-equivalent submit flows run
//! parse-to-fan-out, overlong and unknown inputs never reach the broker, and
//! frozen buckets admit five then limit.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use drishti::canvas_protocol::Capability;
use drishti::native_host::broker::{Broker, BrokerError, IntentHandle, Permission, SignalListener};
use drishti::native_host::capability::CapabilityError;
use drishti::native_host::command_bar::{IntentParseError, IntentPipeline};
use drishti::native_host::protocol::{
    JsonValue, PartialIntent, SignalEnvelope, SignalPattern, SourceKind,
};
use lgwks_std::id::Uuid;

#[path = "support/broker.rs"]
mod broker;
#[path = "support/fixtures.rs"]
mod fixtures;

/// Register the bar; the harness grants `intent.emit` unless a test clears it.
fn bar(harness: &broker::Harness) -> Result<IntentHandle, String> {
    harness
        .broker()
        .register_command_bar(fixtures::session("tenant")?)
        .map_err(|error| format!("{error}"))
}

/// Parse one input and emit it through the bar.
fn submit(
    broker: &Broker,
    bar: &drishti::native_host::broker::IntentHandle,
    input: &str,
) -> Result<drishti::native_host::protocol::SignalEnvelope, String> {
    let partial = IntentPipeline::parse(input).map_err(|error| format!("{error}"))?;
    broker
        .emit_intent(bar, partial)
        .map_err(|error| format!("{error}"))
}

/// Find and freeform submits fan out with their shapes.
#[test]
fn submits_fan_out_with_shapes() -> Result<(), String> {
    let harness = broker::Harness::valid();
    let handle = broker::register(&harness, "widget.cmd", "tenant", fixtures::SESSION_ID)?;
    let got = Arc::new(Mutex::new(Vec::new()));
    let listener = {
        let got = Arc::clone(&got);
        Arc::new(
            move |envelope: drishti::native_host::protocol::SignalEnvelope| match got.lock() {
                Ok(mut guard) => guard.push(envelope),
                Err(poisoned) => poisoned.into_inner().push(envelope),
            },
        )
    };
    harness
        .broker()
        .subscribe(
            &handle,
            SignalPattern::new(String::from("intent.*"), None),
            listener,
        )
        .map_err(|error| format!("subscribe failed: {error}"))?;
    let bar = bar(&harness)?;
    let found = submit(harness.broker(), &bar, "/find acme")?;
    if found.kind() != "intent.find" {
        return Err(String::from("find must keep its kind"));
    }
    let freeform = submit(harness.broker(), &bar, "do the thing")?;
    if freeform.kind() != "intent.freeform" {
        return Err(String::from("text must be freeform"));
    }
    let deliveries = match got.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    if deliveries.len() != 2 {
        return Err(String::from("both submits must fan out"));
    }
    Ok(())
}

/// Overlong and unknown inputs never reach the broker.
#[test]
fn bad_inputs_never_emit() -> Result<(), String> {
    let harness = broker::Harness::valid();
    bar(&harness)?;
    let long = "x".repeat(600);
    match IntentPipeline::parse(&long) {
        Err(IntentParseError::InputTooLong) => {}
        Err(error) => return Err(format!("wrong error: {error}")),
        Ok(_) => return Err(String::from("600 chars must fail")),
    }
    match IntentPipeline::parse("/exec anything") {
        Err(IntentParseError::UnknownCommand) => {}
        Err(error) => return Err(format!("wrong error: {error}")),
        Ok(_) => return Err(String::from("exec must fail")),
    }
    Ok(())
}

/// The command-bar gate: registration without the grant is denied.
#[test]
fn command_bar_gate_requires_intent_emit() -> Result<(), String> {
    let harness = broker::Harness::valid();
    harness.grants().clear();
    let refused = harness
        .broker()
        .register_command_bar(fixtures::session("tenant")?);
    if refused
        != Err(BrokerError::CapabilityDenied(CapabilityError::NotGranted(
            Capability::IntentEmit,
        )))
    {
        return Err(String::from("missing intent.emit must be denied"));
    }
    harness
        .grants()
        .grant(None, Permission::Capability(Capability::IntentEmit));
    bar(&harness)?;
    Ok(())
}

/// Second registration fails while one bar lives.
#[test]
fn second_command_bar_fails_until_unregistered() -> Result<(), String> {
    let harness = broker::Harness::valid();
    let first = bar(&harness)?;
    let refused = harness
        .broker()
        .register_command_bar(fixtures::session("tenant")?);
    if refused != Err(BrokerError::CommandBarAlreadyRegistered) {
        return Err(String::from("second bar must fail"));
    }
    harness.broker().unregister_command_bar(&first);
    bar(&harness)?;
    Ok(())
}

/// Intents stamp the user origin and fan out to intent subscribers.
#[test]
fn intents_stamp_user_origin_and_fan_out() -> Result<(), String> {
    let harness = broker::Harness::valid();
    let widget = broker::register(&harness, "widget.cmd", "tenant", fixtures::SESSION_ID)?;
    let got = Arc::new(Mutex::new(Vec::new()));
    let listener: SignalListener = {
        let got = Arc::clone(&got);
        Arc::new(move |envelope: SignalEnvelope| match got.lock() {
            Ok(mut guard) => guard.push(envelope),
            Err(poisoned) => poisoned.into_inner().push(envelope),
        })
    };
    harness
        .broker()
        .subscribe(
            &widget,
            SignalPattern::new(String::from("intent.*"), None),
            listener,
        )
        .map_err(|error| format!("subscribe failed: {error}"))?;
    let bar = bar(&harness)?;
    let envelope = harness
        .broker()
        .emit_intent(
            &bar,
            PartialIntent::new(
                String::from("intent.find"),
                JsonValue::Object(BTreeMap::from([(
                    String::from("query"),
                    JsonValue::String(String::from("acme")),
                )])),
                String::from("acme"),
            ),
        )
        .map_err(|error| format!("{error}"))?;
    if envelope.source_kind() != SourceKind::User {
        return Err(String::from("intent must stamp user origin"));
    }
    if envelope.source_id() != format!("command-bar:{}", fixtures::USER_SUB) {
        return Err(String::from("intent source must name the user"));
    }
    if broker::lock_vec(&got).len() != 1 {
        return Err(String::from("intent must fan out to intent subscribers"));
    }
    let audited = harness.audited();
    if audited.len() != 1 || audited.first().map(String::as_str) != Some(envelope.signal_id()) {
        return Err(String::from("audit must record the intent"));
    }
    Ok(())
}

/// Intent delivery works within one session, with frozen 5/second buckets.
#[test]
fn intents_deliver_in_session_with_rate_limit() -> Result<(), String> {
    let harness = broker::Harness::valid();
    let widget = broker::register(&harness, "widget.cmd", "tenant", fixtures::SESSION_ID)?;
    let got = broker::subscribe_all(&harness, &widget)?;
    let bar = bar(&harness)?;
    let intent = || {
        PartialIntent::new(
            String::from("intent.find"),
            JsonValue::Object(BTreeMap::new()),
            String::from("acme"),
        )
    };
    for _ in 0..5 {
        harness
            .broker()
            .emit_intent(&bar, intent())
            .map_err(|error| format!("{error}"))?;
    }
    if broker::lock_vec(&got).len() != 5 {
        return Err(String::from("five intents must deliver"));
    }
    let mut limited = 0_usize;
    for _ in 0..2 {
        if harness.broker().emit_intent(&bar, intent()) == Err(BrokerError::RateLimited) {
            limited = limited.saturating_add(1);
        }
    }
    if limited != 2 {
        return Err(String::from("frozen 7 emits must limit 2"));
    }
    // Forged and malformed intents fail closed.
    let forged = IntentHandle::new(
        Uuid::from(4242_u128.to_le_bytes()),
        String::from(fixtures::SESSION_ID),
        String::from("tenant"),
        String::from(fixtures::USER_SUB),
    );
    if harness.broker().emit_intent(&forged, intent()) != Err(BrokerError::InvalidHandle) {
        return Err(String::from("forged bar must be invalid"));
    }
    if harness.broker().emit_intent(
        &bar,
        PartialIntent::new(String::new(), JsonValue::Null, String::new()),
    ) != Err(BrokerError::MalformedKind)
    {
        return Err(String::from("empty kind must be malformed"));
    }
    Ok(())
}

/// Frozen buckets admit five submits then limit the sixth.
#[test]
fn frozen_bucket_limits_sixth_submit() -> Result<(), String> {
    let harness = broker::Harness::valid();
    let bar = bar(&harness)?;
    for _ in 0..5 {
        submit(harness.broker(), &bar, "/find acme")?;
    }
    match submit(harness.broker(), &bar, "/find acme") {
        Err(message) if message.contains("rate limited") => Ok(()),
        Err(message) => Err(format!("wrong error: {message}")),
        Ok(_) => Err(String::from("sixth submit must limit")),
    }
}
