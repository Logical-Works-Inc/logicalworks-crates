//! Audit-trail journey of `api-drive-host` (issues drishti#8, drishti#9, drishti#10).
//!
//! The drive installs the frozen broker's capturing audit sink, then prints
//! each captured [`AuditEvent`] as one JSON line (seq, at_ms, caller, action,
//! reach, decision, missing permission). It also reads the scoped inspectors
//! for the caller's own scope and prints the counts, and reads the newest
//! governance rows under the store's [`MAX_RECENT_ROWS`] cap. Every list is
//! bounded: the audit lines are the events one drive produces, the tap hears
//! one emit, and the tape read stops at the cap.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use drishti::cli::Failure;
use drishti::cli::drive::{Events, captured, must, note, refused};
use drishti::native_host::broker::{
    AuditAction, AuditEvent, Broker, BrokerError, Caller, Decision, Emitter, Reach,
    RegisteredHandle, Subject, TapEvent, TapId, TapListener,
};
use drishti::native_host::governance::{GovernanceQuery, GovernanceStore, MAX_RECENT_ROWS};
use drishti::native_host::protocol::JsonValue;

/// Quote `text` as a JSON string, escaping quotes, backslashes, and control
/// characters so a hostile id cannot split or forge a line.
fn json_text(text: &str) -> Result<String, Failure> {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    for symbol in text.chars() {
        match symbol {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            other if other.is_control() => write!(out, "\\u{:04x}", u32::from(other))
                .map_err(|error| Failure::from(format!("json text unwritable: {error}")))?,
            other => out.push(other),
        }
    }
    out.push('"');
    Ok(out)
}

/// Stable label for a registry entry kind.
fn subject_label(subject: &Subject) -> String {
    match *subject {
        Subject::Widget(ref widget_id) => format!("widget:{widget_id}"),
        Subject::MlPeer => String::from("ml_peer"),
        Subject::CommandBar => String::from("command_bar"),
        Subject::Subscription => String::from("subscription"),
        Subject::Tap => String::from("tap"),
        _ => String::from("unlisted_subject"),
    }
}

/// Stable label for an emit path.
const fn emitter_label(emitter: Emitter) -> &'static str {
    match emitter {
        Emitter::Widget => "widget",
        Emitter::Intent => "intent",
        Emitter::Prediction => "prediction",
        _ => "unlisted_emitter",
    }
}

/// Stable label for a requested reach.
const fn reach_label(reach: Reach) -> &'static str {
    match reach {
        Reach::Own => "own",
        Reach::AllTenants => "all_tenants",
        _ => "unlisted_reach",
    }
}

/// The action name and its one detail field. Only ids appear, never a payload.
fn action_parts(action: &AuditAction) -> (&'static str, String) {
    match *action {
        AuditAction::Route { ref signal_id, .. } => ("route", signal_id.clone()),
        AuditAction::Register(ref subject) => ("register", subject_label(subject)),
        AuditAction::Revoke(ref subject) => ("revoke", subject_label(subject)),
        AuditAction::Inspect(inspector) => ("inspect", format!("{inspector:?}")),
        AuditAction::Emit(emitter) => ("emit", String::from(emitter_label(emitter))),
        _ => ("unlisted_action", String::new()),
    }
}

/// The decision as text: `allowed`, or `denied:` plus the typed reason.
fn decision_label(decision: &Decision) -> String {
    match *decision {
        Decision::Allowed => String::from("allowed"),
        Decision::Denied(ref error) => format!("denied:{error}"),
        _ => String::from("unlisted_decision"),
    }
}

/// One audit event as one JSON line.
fn event_line(event: &AuditEvent) -> Result<String, Failure> {
    let who: &Caller = event.caller();
    let scope = who.scope();
    let (action, detail) = action_parts(event.action());
    let missing = match event.decision().missing() {
        Some(permission) => json_text(&permission.to_string())?,
        None => String::from("null"),
    };
    Ok(format!(
        "{{\"seq\":{},\"at_ms\":{},\"tenant\":{},\"session\":{},\"user\":{},\
         \"action\":\"{action}\",\"detail\":{},\"reach\":\"{}\",\"decision\":{},\
         \"missing\":{missing}}}",
        event.seq(),
        event.at_ms(),
        json_text(scope.tenant_id())?,
        json_text(scope.session_id())?,
        json_text(who.user_sub())?,
        json_text(&detail)?,
        reach_label(event.reach()),
        json_text(&decision_label(event.decision()))?,
    ))
}

/// Append one JSON line per captured audit event, in sink order.
pub(super) fn trail(digest: &mut String, events: &Events) -> Result<(), Failure> {
    for event in captured(events) {
        let line = event_line(&event)?;
        writeln!(digest, "{line}")
            .map_err(|error| Failure::from(format!("digest unwritable: {error}")))?;
    }
    Ok(())
}

/// Append the audited outcome of the call just made: the newest captured
/// audit event under `audited.<step>=`, as the same JSON line the trail prints.
/// A call that left no audit event fails the drive.
pub(super) fn outcome(digest: &mut String, step: &str, events: &Events) -> Result<(), Failure> {
    let newest = captured(events)
        .pop()
        .ok_or_else(|| Failure::from(format!("{step} left no audit event")))?;
    let line = event_line(&newest)?;
    writeln!(digest, "audited.{step}={line}")
        .map_err(|error| Failure::from(format!("digest unwritable: {error}")))
}

/// A fan-out measured at a second or more is a defect. The tap measures real
/// time, so the digest prints whether the fan-out stayed under this bound
/// and never the measured value: the digest must stay the same on every run.
const FANOUT_BOUND_NS: u64 = 1_000_000_000;

/// Tap the caller's own scope, emit once, untap, and print the events the tap
/// held (at most the one emit, since the tap is revoked right after).
pub(super) fn tap_events(
    digest: &mut String,
    broker: &Broker,
    handle: &RegisteredHandle,
) -> Result<(), Failure> {
    let taps: Arc<Mutex<Vec<TapEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let tap_slot = Arc::clone(&taps);
    let tap_listener: TapListener = Arc::new(move |event: TapEvent| {
        let mut guard = match tap_slot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.push(event);
    });
    let tap_id: TapId = must(
        digest,
        "tap-id",
        broker.tap(handle, Reach::Own, tap_listener),
    )?;
    must(
        digest,
        "tapped-emit",
        broker.emit(handle, "drive.tap", JsonValue::Null),
    )?;
    must(digest, "untapped", broker.untap(handle, &tap_id))?;
    let held = match taps.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    note(digest, "tapped-events", &held.len())?;
    for event in &held {
        note(
            digest,
            "tap",
            &(
                event.matched_subscriber_count(),
                event.fanout_duration_ns() < FANOUT_BOUND_NS,
                event.rate_limited_count(),
                event.timestamp_ms(),
                event.envelope().signal_id(),
            ),
        )?;
    }
    Ok(())
}

/// Read the caller's own scope through the inspectors: list the
/// subscriptions, tap the stream for one emit, untap, and refuse a second
/// untap. Prints the counts.
pub(super) fn scope_counts(
    digest: &mut String,
    broker: &Broker,
    handle: &RegisteredHandle,
) -> Result<(), Failure> {
    let subscriptions = broker
        .registered_subscriptions(handle, Reach::Own)
        .map_err(|error| Failure::from(format!("own subscription read refused: {error}")))?;
    let heard = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&heard);
    let listener: TapListener = Arc::new(move |_event: TapEvent| {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let tap_id = must(
        digest,
        "scope-tap",
        broker.tap(handle, Reach::Own, listener),
    )?;
    broker
        .emit(handle, "drive.counted", JsonValue::Null)
        .map_err(|error| Failure::from(format!("counted emit refused: {error}")))?;
    must(digest, "scope-untap", broker.untap(handle, &tap_id))?;
    let second: BrokerError = refused(
        broker.untap(handle, &tap_id),
        "a revoked tap must not untap twice",
    )?;
    note(
        digest,
        "scope-counts",
        &(subscriptions.len(), heard.load(Ordering::SeqCst), second),
    )
}

/// Start of the drive day, 2026-11-03T00:00:00Z in epoch milliseconds: the
/// stamp of the drive's governance fixtures.
const DRIVE_DAY_START_MS: u64 = 1_793_664_000_000;

/// One day in milliseconds, the widest window a governance query allows.
const DRIVE_DAY_MS: u64 = 86_400_000;

/// Read the newest governance rows of `tenant_id` and `session_id` for the
/// drive day under the [`MAX_RECENT_ROWS`] cap, then the single newest row,
/// and print the counts.
pub(super) fn recent_tape(
    digest: &mut String,
    store: &GovernanceStore,
    tenant_id: &str,
    session_id: &str,
) -> Result<(), Failure> {
    let query = GovernanceQuery::new(
        String::from(tenant_id),
        String::from(session_id),
        DRIVE_DAY_START_MS,
        DRIVE_DAY_START_MS.saturating_add(DRIVE_DAY_MS),
        None,
        None,
    )
    .map_err(|error| Failure::from(format!("recent query refused: {error}")))?;
    let rows = store
        .recent(&query, MAX_RECENT_ROWS)
        .map_err(|error| Failure::from(format!("recent read refused: {error}")))?;
    let newest = store
        .recent(&query, 1)
        .map_err(|error| Failure::from(format!("newest read refused: {error}")))?;
    let newest_id = newest.first().map(|row| row.signal_id());
    note(
        digest,
        "recent-rows",
        &(rows.len(), MAX_RECENT_ROWS, newest.len(), newest_id),
    )
}

#[cfg(test)]
mod tests {
    use super::json_text;

    /// A hostile id cannot close its string or split a line (#9 audit stream).
    #[test]
    fn json_text_escapes_quote_backslash_and_control() -> Result<(), String> {
        let quoted = json_text("a\"b\\c\nd\u{7}").map_err(|error| error.to_string())?;
        if quoted != "\"a\\\"b\\\\c\\u000ad\\u0007\"" {
            return Err(format!("unexpected escape: {quoted}"));
        }
        Ok(())
    }

    /// An empty id stays an empty JSON string (#9 forged-handle caller).
    #[test]
    fn json_text_keeps_the_empty_string() -> Result<(), String> {
        let quoted = json_text("").map_err(|error| error.to_string())?;
        if quoted != "\"\"" {
            return Err(format!("unexpected empty form: {quoted}"));
        }
        Ok(())
    }
}
