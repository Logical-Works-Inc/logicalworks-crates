//! Host conformance driver: constructs every native-host type, calls every
//! operation, and prints one canonical digest.
//!
//! Part of the api-drive suite covering the absorbed surface for the surface
//! gate: every new `pub` item needs a caller outside its crate, so these
//! binaries drive the full API end to end against frozen fixtures. A rejected
//! construction or a failed negative control fails the drive loudly. Exit
//! codes keep kernel parity: 0 success, 1 reported failure, 2 when the
//! report could not print.

#[path = "api_drive_host/audit_drive.rs"]
mod audit_drive;
#[path = "api_drive_host/lifecycle.rs"]
mod lifecycle;

use std::process::ExitCode;
use std::sync::atomic::Ordering;

use drishti::canvas_protocol::{Capability, SessionInvariant};
use drishti::cli::drive::{
    POLICY_BUNDLE, SESSION_ID, USER_SUB, WORK_OBJECT, ensure, forged_nonce, frozen_broker, must,
    note, refused, session, widget_owner,
};
use drishti::cli::{Failure, Streams};
use drishti::native_host::attest::{
    AppAttestAttestation, AppAttestError, AppAttestProductionError,
    AppleSecureEnclaveEvidencePackage, ArmTrustZoneEvidencePackage, AttestError,
    BootChallengeReceipt, app_attest_is_supported, produce_app_attest_evidence,
};
use drishti::native_host::attest::{apple, der, trustzone};
use drishti::native_host::broker::{
    BrokerError, Inspector, Limit, Permission, Reach, RegisteredHandle, Scope, Subscription,
    SubscriptionId,
};
use drishti::native_host::cbor::decode_attestation;
use drishti::native_host::governance::{
    AttributionSource, GovernanceEntrySnapshot, GovernanceError, GovernanceQuery, GovernanceStore,
};
use drishti::native_host::protocol::{
    Attribution, AttributionKind, EnvelopeError, GovernanceEntry, IntentEnvelope, JsonValue,
    PredictionEnvelope, SignalEnvelope, SignalPattern, SourceKind, WidgetDescriptor, WidgetSurface,
    validate_envelope, validate_session_invariant,
};
use drishti::native_host::telemetry::{
    LatencyTracker, MAX_RING_CAPACITY, RingBuffer, TapeReader, TelemetryError, TelemetryMetrics,
    TelemetryTap,
};
use drishti::preview::check::TraceCheckError;
use lgwks_deps::p256;

/// Human description of a subscription snapshot for the digest.
fn describe_subscription(subscription: &Subscription) -> String {
    format!("{}:{}", subscription.session_id(), subscription.tenant_id())
}

/// Deterministic device key for attestation fixtures (below the curve order).
fn device_key() -> Result<p256::ecdsa::SigningKey, Failure> {
    p256::ecdsa::SigningKey::from_slice(&[0x42; 32])
        .map_err(|error| Failure::from(format!("fixture key rejected: {error}")))
}

/// Deterministic 32-byte challenge fixture.
fn challenge(byte: u8) -> Vec<u8> {
    vec![byte; 32]
}

/// Receipt fixture.
fn receipt() -> Result<BootChallengeReceipt, AttestError> {
    BootChallengeReceipt::new(
        &challenge(0x11),
        1_000,
        2_000,
        &challenge(0x22),
        1_500,
        &challenge(0x33),
    )
}

/// Compressed SEC1 point for a verifying key.
fn compressed_point(key: &p256::ecdsa::VerifyingKey) -> Vec<u8> {
    key.to_sec1_point(true).as_bytes().to_vec()
}

/// Build a stamped governance entry fixture.
fn drive_entry(
    entry_id: &str,
    signal_id: &str,
    event_type: &str,
    recorded_at: &str,
) -> GovernanceEntry {
    let mut entry = GovernanceEntry::new(
        String::from(SESSION_ID),
        String::from(entry_id),
        String::from(WORK_OBJECT),
        String::from(signal_id),
        String::from(event_type),
        String::from("drive capture"),
        Attribution::new(
            AttributionKind::Widget,
            String::from("widget.drive"),
            None,
            None,
        ),
    );
    entry.set_recorded_at(String::from(recorded_at));
    entry.set_tenant_id(String::from("tenant-drive"));
    entry
}

/// Drive the broker: registry, fan-out, ML peers, and the command bar.
fn drive_broker(digest: &mut String, session: &SessionInvariant) -> Result<(), Failure> {
    let (broker, events, open) = frozen_broker();
    note(digest, "broker", &"built")?;
    let mut descriptor = WidgetDescriptor::new(
        String::from("demo.kind"),
        String::from("widget.drive"),
        String::from("1.0.0"),
        WidgetSurface::Native,
        vec![SignalPattern::new(String::from("drive.**"), None)],
        vec![SignalPattern::new(String::from("drive.**"), None)],
        vec![Capability::SignalEmit, Capability::SignalSubscribe],
    );
    descriptor.set_self_listen(true);
    descriptor.set_pinned(true);
    note(
        digest,
        "descriptor",
        &(
            descriptor.widget_id(),
            descriptor.kind(),
            descriptor.version(),
            descriptor.surface(),
            descriptor.emits().len(),
            descriptor.listens().len(),
            descriptor.capabilities().len(),
            descriptor.self_listen(),
            descriptor.pinned(),
        ),
    )?;
    let handle: RegisteredHandle = must(
        digest,
        "registered",
        broker.register(descriptor, session.clone()),
    )?;
    note(
        digest,
        "handle",
        &(
            handle.nonce(),
            handle.descriptor_id(),
            handle.session_id(),
            handle.tenant_id(),
            handle.user_sub(),
        ),
    )?;
    let subscription: SubscriptionId = lifecycle::subscribe(digest, &broker, &events, &handle)?;
    let envelope = must(
        digest,
        "emitted",
        broker.emit(&handle, "drive.ping", JsonValue::Bool(true)),
    )?;
    note(
        digest,
        "widget-envelope",
        &(
            envelope.signal_id(),
            envelope.kind(),
            envelope.source_kind(),
            envelope.source_id(),
            envelope.session_invariant().session_id(),
            envelope.session_invariant().tenant_id(),
            envelope.session_invariant().work_object_id(),
            envelope.session_invariant().policy_bundle_id(),
            envelope.session_invariant().user_sub(),
            envelope.emitted_at(),
        ),
    )?;
    must(digest, "envelope-valid", validate_envelope(&envelope))?;
    must(
        digest,
        "invariant-valid",
        validate_session_invariant(envelope.session_invariant()),
    )?;
    let bad_envelope = SignalEnvelope::new(
        String::from("not-a-uuid"),
        String::from("drive.bad"),
        SourceKind::Widget,
        String::from("widget.drive"),
        session.clone(),
        JsonValue::Null,
        String::from("2026-11-03T00:00:00Z"),
    );
    let envelope_error: EnvelopeError = refused(
        validate_envelope(&bad_envelope),
        "bad signal id must fail validation",
    )?;
    note(digest, "envelope-error", &envelope_error)?;
    audit_drive::scope_counts(digest, &broker, &handle)?;
    lifecycle::unsubscribe(digest, &broker, &events, &handle, &subscription)?;
    audit_drive::tap_events(digest, &broker, &handle)?;
    let wide = broker.registered_subscriptions(&handle, Reach::AllTenants);
    ensure(
        matches!(wide, Err(BrokerError::CrossTenantDenied)),
        "an ungranted wide read must be refused",
    )?;
    open.store(true, Ordering::SeqCst);
    let listed = broker
        .registered_subscriptions(&handle, Reach::AllTenants)
        .map_err(|error| Failure::from(format!("granted wide read refused: {error}")))?;
    for subscription in listed {
        note(
            digest,
            "snapshot",
            &(
                subscription.id(),
                describe_subscription(&subscription),
                subscription.pattern().kind_glob(),
                subscription.descriptor_id(),
            ),
        )?;
    }
    lifecycle::ml_peer(digest, &broker, &events, session)?;
    let mut intent_envelope = IntentEnvelope::new(
        String::from("intent.drive"),
        String::from("33333333-3333-3333-3333-333333333333"),
        SourceKind::User,
        String::from("command-bar:tenant-drive"),
        session.clone(),
        String::from("2026-11-03T00:00:00Z"),
        JsonValue::Null,
    );
    intent_envelope.set_raw_text(Some(String::from("drive")));
    intent_envelope.set_resolved_kind(Some(String::from("intent.drive")));
    note(
        digest,
        "intent-envelope",
        &(
            intent_envelope.signal_id(),
            intent_envelope.kind(),
            intent_envelope.payload(),
            intent_envelope.emitted_at(),
            intent_envelope.session_invariant().session_id(),
            intent_envelope.source_id(),
            intent_envelope.source_kind(),
            intent_envelope.raw_text(),
            intent_envelope.resolved_kind(),
        ),
    )?;
    let mut prediction_envelope = PredictionEnvelope::new(
        String::from("44444444-4444-4444-4444-444444444444"),
        String::from("ml.drive"),
        SourceKind::Ml,
        String::from("tenant-drive"),
        String::from("2026-11-03T00:00:00Z"),
        session.clone(),
        JsonValue::Null,
    );
    prediction_envelope.set_model_version(String::from("drive-1"));
    prediction_envelope.set_confidence(0.5);
    prediction_envelope.set_reasoning(String::from("drive"));
    prediction_envelope.set_view_directive(Some(JsonValue::Bool(false)));
    note(
        digest,
        "prediction-envelope",
        &(
            prediction_envelope.signal_id(),
            prediction_envelope.session_invariant().session_id(),
            prediction_envelope.payload(),
            prediction_envelope.kind(),
            prediction_envelope.emitted_at(),
            prediction_envelope.source_id(),
            prediction_envelope.source_kind(),
            prediction_envelope.model_version(),
            prediction_envelope.confidence(),
            prediction_envelope.reasoning(),
            prediction_envelope.view_directive(),
        ),
    )?;
    lifecycle::command_bar(digest, &broker, &events, &open, session)?;
    let forged_widget = forged_nonce(&handle.nonce());
    note(digest, "forged-widget", &forged_widget)?;
    let forged = RegisteredHandle::new(
        forged_widget,
        String::from(SESSION_ID),
        String::from("widget.forged"),
        String::from("tenant-drive"),
        String::from(USER_SUB),
    );
    let broker_error: BrokerError = refused(
        broker.emit(&forged, "drive.forged", JsonValue::Null),
        "forged widget handle must fail",
    )?;
    note(digest, "broker-error", &broker_error)?;
    lifecycle::unregister(digest, &broker, &events, &handle)?;
    let bounds = (
        Limit::Widgets.cap(),
        Limit::TapsPerTenant.cap(),
        Inspector::ALL,
    );
    note(digest, "bounds", &bounds)?;
    note(
        digest,
        "permission",
        &Permission::CrossTenantInspect.to_string(),
    )?;
    audit_drive::trail(digest, &events)?;
    Ok(())
}

/// Drive the host stores, telemetry, governance, and attestation.
fn drive_host(digest: &mut String) -> Result<(), Failure> {
    let governance = must(digest, "governance", GovernanceStore::open_in_memory())?;
    let entry = drive_entry(
        "entry-drive-1",
        "drive.ping",
        "drive.ping",
        "2026-11-03T00:00:00Z",
    );
    note(
        digest,
        "entry",
        &(
            entry.entry_id(),
            entry.session_id(),
            entry.work_object_id(),
            entry.signal_id(),
            entry.event_type(),
            GovernanceEntry::description(&entry),
            entry.attribution().kind(),
            entry.attribution().source_id(),
            entry.attribution().model_version(),
            entry.attribution().confidence(),
            entry.recorded_at(),
        ),
    )?;
    must(digest, "appended", governance.append(&entry))?;
    let second = drive_entry(
        "entry-drive-2",
        "drive.pong",
        "drive.pong",
        "2026-11-03T00:00:01Z",
    );
    must(
        digest,
        "appended-policy",
        governance.append_with_policy(&second, String::from(POLICY_BUNDLE)),
    )?;
    refused(governance.append(&entry), "duplicate signal id must fail")?;
    let query = must(
        digest,
        "query",
        GovernanceQuery::new(
            String::from("tenant-drive"),
            String::from(SESSION_ID),
            0,
            86_400_000,
            Some(AttributionSource::Widget),
            Some(String::from("drive.*")),
        ),
    )?;
    note(
        digest,
        "query-shape",
        &(
            query.tenant_id(),
            query.session_id(),
            query.from_ms(),
            query.to_ms(),
            query.attribution_filter(),
            query.kind_glob(),
        ),
    )?;
    refused(
        GovernanceQuery::new(String::new(), String::new(), 10, 0, None, None),
        "unbounded query must fail",
    )?;
    for snapshot in must(digest, "queried", governance.query(&query))? {
        note(
            digest,
            "snapshot",
            &(
                snapshot.signal_id(),
                snapshot.session_id(),
                snapshot.source_kind(),
                snapshot.kind(),
                snapshot.attribution(),
                snapshot.timestamp_ms(),
                snapshot.recorded_at(),
                snapshot.policy_bundle_id(),
                snapshot.work_object_id(),
            ),
        )?;
    }
    audit_drive::recent_tape(digest, &governance, "tenant-drive", SESSION_ID)?;
    let mut bare_snapshot = GovernanceEntrySnapshot::new(
        String::from(SESSION_ID),
        String::from("entry-drive-9"),
        String::from("widget"),
        String::from("drive.ping"),
        String::from("widget"),
        1_000,
        String::from("2026-11-03T00:00:00Z"),
    );
    bare_snapshot.set_policy_bundle_id(String::from(POLICY_BUNDLE));
    bare_snapshot.set_work_object_id(String::from(WORK_OBJECT));
    note(digest, "bare-snapshot", &bare_snapshot.signal_id())?;
    note(
        digest,
        "attribution-sources",
        &(
            AttributionSource::resolve("user").map_err(|error| format!("{error}")),
            AttributionSource::resolve("ml").map_err(|error| format!("{error}")),
            AttributionSource::resolve("widget").map_err(|error| format!("{error}")),
            AttributionSource::resolve("nope").map_err(|error| format!("{error}")),
            AttributionSource::Widget.raw_value(),
        ),
    )?;
    let governance_error: GovernanceError = refused(
        governance.append(&drive_entry("", "", "drive.ping", "")),
        "empty signal id must fail",
    )?;
    note(digest, "governance-error", &governance_error)?;
    must(
        digest,
        "governance-count",
        governance.count("tenant-drive", SESSION_ID),
    )?;
    let file_store_path = std::env::temp_dir().join("api-drive-governance.sqlite");
    let file_store = must(
        digest,
        "file-store",
        GovernanceStore::open(&file_store_path),
    )?;
    must(
        digest,
        "file-count",
        file_store.count("tenant-drive", SESSION_ID),
    )?;
    std::fs::remove_file(&file_store_path)
        .map_err(|error| Failure::from(format!("temp store cleanup failed: {error}")))?;
    let tap_store = must(digest, "tap-store", GovernanceStore::open_in_memory())?;
    let tap = TelemetryTap::new(tap_store, vec![Capability::SignalSubscribe], 100)
        .map_err(|error| Failure::from(format!("tap refused: {error}")))?;
    note(digest, "tap-built", &"ok")?;
    let (broker, _events, _open) = frozen_broker();
    let owner_session = session(digest, "tenant-drive")?;
    let owner = widget_owner(digest, &broker, &owner_session)?;
    must(digest, "tap-start", tap.start(&broker, &owner))?;
    note(
        digest,
        "tap-ring",
        &(
            tap.snapshot(None).len(),
            tap.snapshot(Some(SESSION_ID)).len(),
        ),
    )?;
    let metrics: TelemetryMetrics = tap.metrics();
    note(
        digest,
        "metrics",
        &(
            metrics.emits_observed(),
            metrics.append_errors(),
            metrics.tap_latency_p99_microseconds(),
        ),
    )?;
    tap.stop(&broker);
    let (ml_bound, payload_bound) = (Limit::MlPeersPerTenant.cap(), Limit::PayloadBytes.cap());
    note(
        digest,
        "tap-bounds",
        &(MAX_RING_CAPACITY, ml_bound, payload_bound),
    )?;
    note(digest, "tap-error", &tap.last_error())?;
    let denied_tap = TelemetryTap::new(
        must(digest, "denied-store", GovernanceStore::open_in_memory())?,
        Vec::new(),
        10,
    )
    .map_err(|error| Failure::from(format!("denied tap refused: {error}")))?;
    note(digest, "denied-tap-built", &"ok")?;
    refused(
        denied_tap.start(&broker, &owner),
        "tap without capability must fail",
    )?;
    let mut ring = RingBuffer::<u64>::new(4)
        .map_err(|error| Failure::from(format!("ring refused: {error}")))?;
    note(digest, "ring-built", &"ok")?;
    ring.push(1);
    ring.push(2);
    note(
        digest,
        "ring",
        &(ring.count(), ring.total_pushed(), ring.snapshot()),
    )?;
    let telemetry_error: TelemetryError =
        refused(RingBuffer::<u64>::new(0), "zero ring capacity must fail")?;
    note(digest, "telemetry-error", &telemetry_error)?;
    let mut tracker = LatencyTracker::new();
    tracker.record(1_500);
    let sized_tracker = LatencyTracker::with_capacity(16);
    note(
        digest,
        "p99",
        &(sized_tracker.p99_microseconds(), tracker.p99_microseconds()),
    )?;
    let reader = TapeReader::new(must(
        digest,
        "tape-store",
        GovernanceStore::open_in_memory(),
    )?);
    let scope = Scope::new("tenant-drive", SESSION_ID);
    let entries = must(
        digest,
        "tape-entries",
        reader.recent_entries(&scope, 10, 1.0, 86_400_000),
    )?;
    note(digest, "tape-len", &entries.len())?;
    let attest_error: AppAttestError =
        refused(decode_attestation(&[]), "empty attestation must fail")?;
    note(digest, "attest-error", &attest_error)?;
    let minimal: &[u8] = &[
        0xA2, 0x67, 0x61, 0x74, 0x74, 0x53, 0x74, 0x6D, 0x74, 0xA1, 0x63, 0x78, 0x35, 0x63, 0x81,
        0x41, 0x00, 0x68, 0x61, 0x75, 0x74, 0x68, 0x44, 0x61, 0x74, 0x61, 0x41, 0x00,
    ];
    let attestation = must(digest, "attestation", AppAttestAttestation::parse(minimal))?;
    note(
        digest,
        "attestation-parts",
        &(
            attestation.leaf_cert_der().len(),
            attestation.intermediate_certs_der().len(),
            attestation.chain().len(),
            attestation.auth_data().len(),
            attestation.receipt().is_none(),
        ),
    )?;
    let receipt_error: AttestError = refused(
        BootChallengeReceipt::new(&[0u8; 31], 0, 0, &[0u8; 32], 0, &[0u8; 32]),
        "short challenge must fail",
    )?;
    note(digest, "receipt-error", &receipt_error)?;
    let receipt_value = must(digest, "receipt", receipt())?;
    note(
        digest,
        "receipt-fields",
        &(
            receipt_value.issued_challenge(),
            receipt_value.issued_at_epoch(),
            receipt_value.expires_at_epoch(),
            receipt_value.consumed_challenge(),
            receipt_value.consumed_at_epoch(),
            receipt_value.issue_mac(),
        ),
    )?;
    let signing = device_key()?;
    let point = compressed_point(signing.verifying_key());
    let package: AppleSecureEnclaveEvidencePackage = must(
        digest,
        "apple-package",
        apple::make_package(
            vec![0x30, 0x03, 0x01, 0x02],
            vec![vec![0x30, 0x03, 0x01, 0x03]],
            &receipt_value,
            point.as_slice(),
            &signing,
        ),
    )?;
    note(
        digest,
        "package",
        &(
            package.key_cert_der().len(),
            package.intermediate_certs_der().len(),
            package.challenge(),
            package.attested_key_compressed_sec1(),
            package.challenge_receipt().issued_at_epoch(),
            package.challenge_pop_signature_der().len(),
        ),
    )?;
    refused(
        apple::make_package(vec![1], vec![], &receipt_value, &[0x04; 33], &signing),
        "uncompressed key must fail",
    )?;
    let quote = must(
        digest,
        "quote",
        trustzone::make_quote("drive-oem", &challenge(0x44), &[0x30, 0x03], &signing),
    )?;
    note(digest, "quote-len", &quote.len())?;
    let tz_package: ArmTrustZoneEvidencePackage = must(
        digest,
        "tz-package",
        trustzone::make_package(
            "drive-oem",
            &receipt_value,
            &[0x30, 0x03],
            point.as_slice(),
            vec![vec![0x30, 0x03, 0x01, 0x04]],
            &signing,
        ),
    )?;
    note(
        digest,
        "tz-package",
        &(
            tz_package.quote_der().len(),
            tz_package.challenge_receipt().issued_at_epoch(),
            tz_package.challenge(),
            tz_package.cert_chain_der().len(),
            tz_package.attested_key_compressed_sec1(),
        ),
    )?;
    note(
        digest,
        "der",
        &(
            der::sequence(&[vec![1], vec![2]]).len(),
            der::integer(1).len(),
            der::utf8_string("drive").len(),
            der::octet_string(&[9]).len(),
        ),
    )?;
    note(digest, "attest-supported", &app_attest_is_supported())?;
    let production_error: AppAttestProductionError = refused(
        produce_app_attest_evidence(&receipt_value),
        "off-device production must fail",
    )?;
    note(digest, "production-error", &production_error)?;
    note(digest, "trace-error", &TraceCheckError::MissingSummary)?;
    Ok(())
}

/// Run the drive and print the digest.
fn drive(streams: &mut Streams<'_>) -> Result<u8, Failure> {
    let mut digest = String::new();
    let session = session(&mut digest, "tenant-drive")?;
    drive_broker(&mut digest, &session)?;
    drive_host(&mut digest)?;
    streams.tell(&digest)?;
    Ok(0)
}

/// Driver entry point: exit code, never `process::exit`.
fn main() -> ExitCode {
    drishti::cli::run("api-drive-host", drive)
}
