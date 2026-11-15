//! Scene and layout conformance driver: constructs every canvas-protocol and
//! scene type, calls every operation, and prints one canonical digest.
//!
//! Part of the api-drive suite covering the absorbed surface for the surface
//! gate: every new `pub` item needs a caller outside its crate, so these
//! binaries drive the full API end to end against frozen fixtures. A rejected
//! construction or a failed negative control fails the drive loudly. Exit
//! codes keep kernel parity: 0 success, 1 reported failure, 2 when the
//! report could not print.

use std::process::ExitCode;

use drishti::canvas_protocol::workspace_layout::{
    WORKSPACE_LAYOUT_SCHEMA_VERSION, WorkspaceChrome, WorkspaceLayout, WorkspaceLayoutError,
    WorkspaceLayoutIssue, WorkspaceLoadError, WorkspacePresentation, WorkspaceSignalPattern,
    WorkspaceSurface, WorkspaceTool, WorkspaceWidgetDescriptor, load_validated,
};
use drishti::canvas_protocol::{
    ActorType, Capability, ChromePlacement, CompensationStrategy, EgressDestination,
    LocalActionStatus, NodeKind as ProtocolNodeKind, OfflineBehavior, SessionInvariant, SyncStatus,
    TrustBin, WidgetSurface as ProtocolWidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole,
    WorkspaceTheme, WorkspaceToolIcon,
};
use drishti::canvas_scene::{
    Action, Artifact, Canvas, CommitStatus, Delta, Error as SceneError, Frame, FrameToken, Limits,
    PresentationLease, Statistics, View,
};
use drishti::cli::drive::{WORK_OBJECT, ensure, must, note, refused, session};
use drishti::cli::{Failure, Streams};
use drishti::native_host::attest::AppAttestError;
use drishti::native_host::capability::{
    CapabilityError, assert_capability, is_ml_only, is_privileged,
};
use drishti::native_host::cbor::{CborValue, MAX_ATTEST_BYTES, decode_attestation};
use drishti::native_host::command_bar::{CommandSuggestions, IntentParseError, IntentPipeline};
use drishti::native_host::pattern::{glob_match, match_pattern, validate_pattern};
use drishti::native_host::protocol::{
    JsonValue, PayloadPredicate, SignalEnvelope, SignalPattern, SourceKind,
};
use drishti::native_host::session::{
    MountDecision, MountDenialReason, SessionStore, ZoneContainer, ZoneId,
};
use drishti::native_host::time::{MAX_WINDOW_MILLIS, TimeError, format_millis, parse_millis};

/// Drive the workspace-layout vocabulary.
fn drive_layout(digest: &mut String) -> Result<(), Failure> {
    note(digest, "schema-version", &WORKSPACE_LAYOUT_SCHEMA_VERSION)?;
    let chrome = WorkspaceChrome::new(
        ChromePlacement::Top,
        vec![ChromePlacement::Top, ChromePlacement::Bottom],
        true,
        WorkspaceTheme::Calm,
    );
    note(digest, "chrome-placement", &chrome.default_placement())?;
    note(digest, "chrome-allowed", &chrome.allowed_placements().len())?;
    note(digest, "chrome-collapsible", &chrome.collapsible())?;
    note(digest, "chrome-theme", &chrome.default_theme())?;
    let mut descriptor = WorkspaceWidgetDescriptor::new(
        String::from("demo.widget"),
        String::from("widget.demo"),
        String::from("1.0.0"),
        ProtocolWidgetSurface::Both,
        vec![WorkspaceSignalPattern::new(String::from("demo.**"))],
        vec![WorkspaceSignalPattern::new(String::from("demo.**"))],
        vec![Capability::SignalEmit],
    );
    descriptor.set_self_listen(true);
    descriptor.set_pinned(true);
    descriptor.set_widget_id(String::from("widget.demo2"));
    descriptor.set_kind(String::from("demo.widget2"));
    descriptor.set_version(String::from("1.0.1"));
    note(digest, "widget-id", &descriptor.widget_id())?;
    note(digest, "widget-kind", &descriptor.kind())?;
    note(digest, "widget-version", &descriptor.version())?;
    note(digest, "widget-surface", &descriptor.surface())?;
    note(digest, "widget-emits", &descriptor.emits().len())?;
    note(digest, "widget-listens", &descriptor.listens().len())?;
    note(
        digest,
        "widget-capabilities",
        &descriptor.capabilities().len(),
    )?;
    note(digest, "widget-self-listen", &descriptor.self_listen())?;
    note(digest, "widget-pinned", &descriptor.pinned())?;
    descriptor
        .capabilities_mut()
        .push(Capability::SignalSubscribe);
    let mut surface = WorkspaceSurface::new(
        descriptor,
        WorkspacePresentation::new(
            String::from("Panel"),
            WorkspaceSurfaceRole::Panel,
            WorkspaceSlot::Primary,
            0,
        ),
    );
    surface.presentation_mut().set_order(1);
    note(digest, "surface-role", &surface.presentation().role())?;
    note(digest, "surface-slot", &surface.presentation().slot())?;
    note(digest, "surface-order", &surface.presentation().order())?;
    note(digest, "surface-title", &surface.presentation().title())?;
    let mut layout = WorkspaceLayout::new(
        WORKSPACE_LAYOUT_SCHEMA_VERSION,
        String::from("Drive"),
        String::from("workspace.drive"),
        String::from("conformance"),
        chrome,
        vec![surface],
        vec![WorkspaceTool::new(
            String::from("canvas.drive"),
            String::from("Drive"),
            WorkspaceToolIcon::Cursor,
        )],
    );
    must(digest, "layout-valid", layout.validate())?;
    let projected = must(digest, "layout-projected", layout.web_renderer_input())?;
    note(digest, "projected-surfaces", &projected.surfaces().len())?;
    note(digest, "layout-schema", &layout.schema_version())?;
    note(digest, "layout-id", &layout.layout_id())?;
    note(digest, "layout-title", &layout.title())?;
    note(digest, "layout-subtitle", &layout.subtitle())?;
    note(
        digest,
        "layout-chrome",
        &layout.chrome().default_placement(),
    )?;
    note(digest, "layout-tools", &layout.tools().len())?;
    layout.set_schema_version(WORKSPACE_LAYOUT_SCHEMA_VERSION);
    layout.set_layout_id(String::from("workspace.drive2"));
    layout.set_title(String::from("Drive 2"));
    layout.set_subtitle(String::from("second"));
    layout.surfaces_mut()[0]
        .descriptor_mut()
        .set_version(String::from("1.0.2"));
    layout.tools_mut()[0].set_tool_id(String::from("canvas.drive2"));
    let tool = &layout.tools()[0];
    note(digest, "tool-id", &tool.tool_id())?;
    note(digest, "tool-label", &tool.label())?;
    note(digest, "tool-icon", &tool.icon())?;
    let wire = lgwks_std::json::to_string(&layout)?;
    let loaded = must(digest, "layout-round-trip", load_validated(wire.as_bytes()))?;
    note(digest, "loaded-id", &loaded.layout_id())?;
    let load_error: WorkspaceLoadError = refused(
        load_validated(b"{\"nope\":true}"),
        "bogus layout must fail closed",
    )?;
    note(digest, "load-error", &load_error)?;
    let mut bad = loaded;
    bad.set_schema_version(999);
    let layout_error: WorkspaceLayoutError =
        refused(bad.validate(), "bad schema must fail validation")?;
    note(digest, "layout-issue", &layout_error.issue())?;
    note(digest, "layout-at", &layout_error.at())?;
    let layout_error_built = WorkspaceLayoutError::new(WorkspaceLayoutIssue::InvalidTitle, "title");
    note(digest, "layout-error-built", &layout_error_built)?;
    let issue: WorkspaceLayoutIssue = layout_error.issue();
    note(digest, "layout-issue-again", &issue)?;
    Ok(())
}

/// Drive the capability and pattern vocabulary.
fn drive_capability(digest: &mut String, session: &SessionInvariant) -> Result<(), Failure> {
    note(digest, "capabilities", &Capability::all())?;
    note(digest, "egress", &EgressDestination::CloudRelay)?;
    note(digest, "actor", &ActorType::Agent)?;
    note(digest, "trust", &TrustBin::Managed)?;
    note(digest, "local-status", &LocalActionStatus::Running)?;
    note(digest, "sync-status", &SyncStatus::Uploading)?;
    note(digest, "compensation", &CompensationStrategy::Rollback)?;
    note(digest, "offline", &OfflineBehavior::QueueLocally)?;
    note(
        digest,
        "protocol-tier",
        &ProtocolNodeKind::App.max_action_tier(),
    )?;
    note(
        digest,
        "protocol-exposure",
        &ProtocolNodeKind::App.data_exposure_level(),
    )?;
    note(digest, "ml-only-tape", &is_ml_only(&Capability::TapeRead))?;
    note(digest, "ml-only-emit", &is_ml_only(&Capability::SignalEmit))?;
    note(
        digest,
        "privileged-emit",
        &is_privileged(&Capability::IntentEmit),
    )?;
    note(
        digest,
        "privileged-subscribe",
        &is_privileged(&Capability::SignalSubscribe),
    )?;
    must(
        digest,
        "assert-ok",
        assert_capability(
            &[Capability::SignalEmit, Capability::SignalSubscribe],
            Capability::SignalEmit,
        ),
    )?;
    let capability_error: CapabilityError = refused(
        assert_capability(&[Capability::SignalEmit], Capability::TapeRead),
        "missing capability must fail",
    )?;
    note(digest, "capability-error", &capability_error)?;
    let envelope = SignalEnvelope::new(
        String::from("66666666-6666-6666-6666-666666666666"),
        String::from("drive.ping"),
        SourceKind::Widget,
        String::from("widget.drive"),
        session.clone(),
        JsonValue::Bool(true),
        String::from("2026-11-03T00:00:00Z"),
    );
    let pattern = SignalPattern::new(
        String::from("drive.**"),
        Some(PayloadPredicate::eq(
            String::from("level"),
            JsonValue::Number(1.0),
        )),
    );
    note(digest, "pattern-valid", &validate_pattern(&pattern))?;
    note(
        digest,
        "pattern-matches",
        &match_pattern(&envelope, &pattern),
    )?;
    note(digest, "glob", &glob_match("intent.*", "intent.find"))?;
    note(
        digest,
        "neq",
        &PayloadPredicate::neq(String::from("a"), JsonValue::Null),
    )?;
    note(
        digest,
        "is-in",
        &PayloadPredicate::is_in(String::from("a"), vec![JsonValue::Bool(true)]),
    )?;
    note(
        digest,
        "exists",
        &PayloadPredicate::exists(String::from("a")),
    )?;
    note(
        digest,
        "pattern-predicate",
        &pattern.payload_predicate().is_some(),
    )?;
    note(digest, "pattern-glob", &pattern.kind_glob())?;
    Ok(())
}

/// Drive the scene reducer.
fn drive_scene(digest: &mut String, session: &SessionInvariant) -> Result<(), Failure> {
    let limits = must(digest, "scene-limits", Limits::new(262_144, 32, 8_847_360))?;
    note(digest, "limits-text", &limits.text_bytes())?;
    note(digest, "limits-queue", &limits.queued_commands())?;
    note(digest, "limits-pixels", &limits.pixels())?;
    must(digest, "limits-valid", limits.validate())?;
    must(digest, "pixel-count", limits.pixel_count(900, 600))?;
    refused(Limits::new(0, 32, 100), "zero text envelope must fail")?;
    for view in View::ALL {
        note(digest, "view-index", &view.index())?;
        note(digest, "view-label", &view.label())?;
    }
    for status in CommitStatus::ALL {
        note(digest, "status-label", &status.label())?;
    }
    let artifact = must(
        digest,
        "artifact",
        Artifact::new(
            WORK_OBJECT,
            1,
            "first\nsecond\nthird",
            Some("proposal"),
            Some("evidence"),
            limits,
        ),
    )?;
    note(digest, "artifact-key", &artifact.key())?;
    note(digest, "artifact-revision", &artifact.revision())?;
    note(digest, "artifact-status", &artifact.commit_status())?;
    note(digest, "artifact-text", &artifact.text(View::Source))?;
    note(
        digest,
        "artifact-supplied",
        &artifact.supplied(View::Proposal),
    )?;
    note(digest, "artifact-bytes", &artifact.payload_bytes())?;
    let stated = must(
        digest,
        "artifact-stated",
        Artifact::with_status(
            WORK_OBJECT,
            2,
            "s",
            None,
            None,
            CommitStatus::Failed,
            limits,
        ),
    )?;
    note(digest, "stated-status", &stated.commit_status())?;
    let lease = must(
        digest,
        "lease",
        PresentationLease::new("lease-drive", "surface-drive", session.clone(), 1, limits),
    )?;
    must(digest, "lease-valid", lease.validate())?;
    note(digest, "lease-schema", &lease.schema_version())?;
    note(digest, "lease-id", &lease.lease_id())?;
    note(digest, "lease-surface", &lease.surface_id())?;
    note(digest, "lease-generation", &lease.generation())?;
    note(digest, "lease-limits", &lease.limits().pixels())?;
    let mut scene = Canvas::with_lease(lease, artifact)
        .map_err(|error| Failure::from(format!("scene refused: {error}")))?;
    note(digest, "scene-built", &"ok")?;
    note(digest, "scene-generation", &scene.generation())?;
    note(digest, "scene-revision", &scene.revision())?;
    note(digest, "scene-status", &scene.commit_status())?;
    note(digest, "scene-confirmed", &scene.confirmed_commit_status())?;
    note(digest, "scene-optimistic", &scene.is_optimistic())?;
    note(digest, "scene-limits", &scene.limits().text_bytes())?;
    note(digest, "scene-target", &scene.target())?;
    note(digest, "scene-reduced", &scene.reduced_motion())?;
    note(digest, "scene-needs-frame", &scene.needs_frame())?;
    let stats: Statistics = scene.stats();
    note(digest, "stats-accepted", &stats.accepted())?;
    note(digest, "stats-drained", &stats.drained())?;
    note(digest, "stats-sampled", &stats.sampled())?;
    note(digest, "stats-prepared", &stats.prepared())?;
    note(digest, "stats-presented", &stats.presented())?;
    let base = must(digest, "base-revision", scene.next_base_revision())?;
    let delta = Delta::new(
        session.clone(),
        scene.generation(),
        base,
        Action::Navigate(View::Proposal),
    );
    note(digest, "delta-session", &delta.session().session_id())?;
    note(digest, "delta-generation", &delta.generation())?;
    note(digest, "delta-base", &delta.base_revision())?;
    note(digest, "delta-action-navigate", &delta.action())?;
    must(digest, "enqueue", scene.enqueue(delta))?;
    let frame: Frame = scene
        .prepare(0.0, 900, 600, false)?
        .ok_or_else(|| Failure::from("drive scene must prepare a frame"))?;
    let token: FrameToken = frame.token();
    must(
        digest,
        "acknowledge",
        scene.acknowledge_present(token.clone()),
    )?;
    let presented: FrameToken = scene
        .presented_token()
        .ok_or_else(|| Failure::from("drive scene must present a token"))?;
    must(digest, "input", scene.input(presented, Action::Step(1)))?;
    refused(
        scene.input(token.clone(), Action::Transition(CommitStatus::Committed)),
        "transition input must be WrongScope",
    )?;
    let stepped = scene
        .prepare(0.04, 900, 600, false)?
        .ok_or_else(|| Failure::from("drive scene must step a frame"))?;
    note(digest, "stepped-target", &stepped.target())?;
    note(digest, "stepped-position", &stepped.position())?;
    note(digest, "stepped-velocity", &stepped.velocity())?;
    note(digest, "stepped-animating", &stepped.animating())?;
    note(digest, "stepped-reduced", &stepped.reduced_motion())?;
    note(digest, "stepped-width", &stepped.width())?;
    note(digest, "stepped-height", &stepped.height())?;
    note(digest, "stepped-scroll", &stepped.scroll())?;
    note(digest, "stepped-optimistic", &stepped.optimistic())?;
    note(
        digest,
        "stepped-confirmed",
        &stepped.confirmed_commit_status(),
    )?;
    note(digest, "stepped-status-label", &stepped.status_label())?;
    note(digest, "stepped-scene-revision", &stepped.scene_revision())?;
    note(digest, "stepped-hit", &stepped.hit_test(450.0, 590.0))?;
    note(digest, "stepped-artifact-key", &stepped.artifact().key())?;
    let mut rebuilt = Frame::new(
        token,
        std::sync::Arc::clone(stepped.artifact()),
        CommitStatus::Running,
        CommitStatus::Proposed,
        false,
        0.0,
        1.0,
    );
    rebuilt.set_target(View::Evidence);
    rebuilt.set_scroll([0, 1, 0]);
    rebuilt.set_viewport(900, 600);
    rebuilt.set_reduced_motion(true);
    rebuilt.set_animating(false);
    note(digest, "rebuilt-target", &rebuilt.target())?;
    must(digest, "deny", scene.deny_optimistic())?;
    let scroll_base = must(digest, "scroll-base", scene.next_base_revision())?;
    must(
        digest,
        "scroll-enqueue",
        scene.enqueue(Delta::new(
            session.clone(),
            scene.generation(),
            scroll_base,
            Action::scroll(View::Source, 3),
        )),
    )?;
    scene.invalidate_surface();
    scene.resume_without_replay();
    scene.revoke();
    let scene_error: SceneError = refused(
        scene.prepare(0.0, 900, 600, false),
        "revoked prepare must fail",
    )?;
    note(digest, "scene-error", &scene_error)?;
    ensure(scene.lease().is_some(), "drive scene must retain its lease")
}

/// Drive the session shell: session store plus mountable zones.
fn drive_session_shell(digest: &mut String, session: &SessionInvariant) -> Result<(), Failure> {
    let mut store = SessionStore::new();
    note(digest, "store-empty", &store.invariant().is_none())?;
    store.start_session(session.clone(), Some(String::from("drive")), 1_000);
    note(
        digest,
        "store",
        &(
            store.invariant().is_some(),
            store.focused_zone(),
            store.active_workspace(),
            store.session_started_ms(),
            store.display_name(),
            store.governance_visible(),
        ),
    )?;
    store.set_focus(Some(String::from("primary")));
    store.set_active_workspace(Some(String::from("workspace.drive")));
    store.set_display_name(Some(String::from("Drive")));
    store.set_governance_visible(true);
    note(
        digest,
        "store-set",
        &(
            store.focused_zone(),
            store.active_workspace(),
            store.display_name(),
            store.governance_visible(),
        ),
    )?;
    let mut zone = ZoneContainer::new(ZoneId::Header);
    note(
        digest,
        "zone",
        &(zone.zone_id(), zone.is_pinned(), zone.current_widget_id()),
    )?;
    let first_mount: MountDecision = zone.request_mount(String::from("widget.drive"));
    note(digest, "zone-mount", &first_mount)?;
    zone.pin();
    let denied = zone.request_mount(String::from("widget.other"));
    note(digest, "zone-denied", &denied)?;
    zone.unpin();
    zone.clear();
    note(digest, "zone-cleared", &zone.current_widget_id())?;
    let reason = MountDenialReason::zone_pinned(Some(String::from("widget.holder")));
    note(digest, "denial-holder", &reason.existing())?;
    note(
        digest,
        "zones",
        &(
            ZoneId::Primary,
            ZoneId::Secondary,
            ZoneId::Tertiary,
            ZoneId::Footer,
            ZoneId::Overlay,
        ),
    )?;
    Ok(())
}

/// Drive the wire helpers: command-bar parsing, time, and CBOR.
fn drive_wires(digest: &mut String) -> Result<(), Failure> {
    let intent = must(digest, "freeform", IntentPipeline::parse("hello drive"))?;
    note(digest, "freeform", &(intent.kind(), intent.text()))?;
    let parse_error: IntentParseError = refused(
        IntentPipeline::parse("/bogus-command"),
        "unknown command must fail",
    )?;
    note(digest, "parse-error", &parse_error)?;
    refused(
        IntentPipeline::parse(&"x".repeat(513)),
        "overlong input must fail",
    )?;
    let suggestions = CommandSuggestions::suggestions("/p");
    note(digest, "suggestions", &suggestions)?;
    note(digest, "window", &MAX_WINDOW_MILLIS)?;
    note(digest, "stamped", &format_millis(1_000))?;
    let stamp_ms = must(digest, "parsed-stamp", parse_millis("2026-11-03T00:00:00Z"))?;
    note(digest, "stamp-round-trip", &format_millis(stamp_ms))?;
    let time_error: TimeError =
        refused(parse_millis("bogus timestamp"), "bogus timestamp must fail")?;
    note(digest, "time-error", &time_error)?;
    note(digest, "attest-bytes", &MAX_ATTEST_BYTES)?;
    let decoded: CborValue = must(digest, "cbor-uint", decode_attestation(&[0x00]))?;
    note(
        digest,
        "cbor-shapes",
        &(
            decoded.map_value().is_none(),
            decoded.array_value().is_none(),
            decoded.bytes_value().is_none(),
        ),
    )?;
    let converted = JsonValue::from_json(&lgwks_std::json::Value::Bool(true));
    note(digest, "converted-bool", &converted)?;
    let sample = JsonValue::Object(
        [(String::from("level"), JsonValue::Number(1.0))]
            .into_iter()
            .collect(),
    );
    note(digest, "sample-string", &sample.string_value().is_none())?;
    note(digest, "sample-object", &sample.object_value().is_some())?;
    let attest_error: AppAttestError =
        refused(decode_attestation(&[]), "empty attestation must fail")?;
    note(digest, "attest-error", &attest_error)?;
    Ok(())
}
/// Run the drive and print the digest.
fn drive(streams: &mut Streams<'_>) -> Result<u8, Failure> {
    let mut digest = String::new();
    let session = session(&mut digest, "tenant-drive")?;
    drive_layout(&mut digest)?;
    drive_capability(&mut digest, &session)?;
    drive_scene(&mut digest, &session)?;
    drive_session_shell(&mut digest, &session)?;
    drive_wires(&mut digest)?;
    streams.tell(&digest)?;
    Ok(0)
}

/// Driver entry point: exit code, never `process::exit`.
fn main() -> ExitCode {
    drishti::cli::run("api-drive-scene", drive)
}
