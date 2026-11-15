//! Workspace layout validation contract.
//!
//! Counterpart of `kernel/crates/canvas-protocol/tests/workspace_layout.rs`
//! (kernel#850 row 15): the canonical landing workspace validates, the schema
//! enums match the Rust wire strings, web-renderer output is deterministic and
//! byte-identical to the tracked artifact, and malformed layouts fail closed.

use std::path::{Path, PathBuf};

use drishti::canvas_protocol::workspace_layout::{
    WORKSPACE_LAYOUT_SCHEMA_VERSION, WorkspaceLayout, WorkspaceLayoutError, WorkspaceLayoutIssue,
};
use drishti::canvas_protocol::{
    Capability, ChromePlacement, WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole,
    WorkspaceTheme, WorkspaceToolIcon,
};

/// Locate the absorbed protocol tree regardless of the caller's cwd.
fn protocol_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets/canvas-protocol")
        .join(relative)
}

/// Read a file or fail the test with its path attached.
fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

/// Decode the canonical landing workspace.
fn canonical_layout() -> Result<WorkspaceLayout, String> {
    let path = protocol_path("workspaces/workspace-landing.json");
    let bytes = read_file(&path)?;
    lgwks_std::json::from_slice(&bytes)
        .map_err(|error| format!("cannot decode {}: {error}", path.display()))
}

/// Serialize wire enums to their JSON strings.
fn wire_values<T: lgwks_std::json::serde::Serialize>(values: &[T]) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        let encoded =
            lgwks_std::json::to_value(value).map_err(|error| format!("serialize: {error}"))?;
        let wire = encoded
            .as_str()
            .ok_or_else(|| String::from("wire enum must serialize as a string"))?;
        out.push(wire.to_owned());
    }
    Ok(out)
}

/// Read a schema enum array at a JSON pointer.
fn schema_enum(schema: &lgwks_std::json::Value, pointer: &str) -> Result<Vec<String>, String> {
    let node = pointer
        .split('/')
        .filter(|segment| !segment.is_empty())
        .try_fold(schema, |parent, segment| parent.get(segment))
        .ok_or_else(|| format!("schema enum missing at {pointer}"))?;
    let array = node
        .as_array()
        .ok_or_else(|| format!("schema enum at {pointer} is not an array"))?;
    let mut out = Vec::with_capacity(array.len());
    for value in array {
        let entry = value
            .as_str()
            .ok_or_else(|| format!("schema enum value at {pointer} is not a string"))?;
        out.push(entry.to_owned());
    }
    Ok(out)
}

/// Read and decode a JSON file.
fn read_json(path: &Path) -> Result<lgwks_std::json::Value, String> {
    let bytes = read_file(path)?;
    lgwks_std::json::from_slice(&bytes)
        .map_err(|error| format!("cannot decode {}: {error}", path.display()))
}

/// The canonical workspace validates and keeps renderer order.
#[test]
fn canonical_workspace_validates_and_preserves_renderer_order() -> Result<(), String> {
    fn assert_standard_error<T: std::error::Error>() {}
    assert_standard_error::<WorkspaceLayoutError>();

    let layout = canonical_layout()?;
    layout
        .validate()
        .map_err(|error| format!("canonical workspace must validate: {error}"))?;
    if layout.schema_version() != WORKSPACE_LAYOUT_SCHEMA_VERSION {
        return Err(String::from("canonical schema version drifted"));
    }
    if layout.layout_id() != "workspace.landing" {
        return Err(format!(
            "canonical layout id drifted: {}",
            layout.layout_id()
        ));
    }
    if layout.surfaces().len() != 4 {
        return Err(format!(
            "canonical surface count drifted: {}",
            layout.surfaces().len()
        ));
    }
    if layout.tools().len() != 4 {
        return Err(format!(
            "canonical tool count drifted: {}",
            layout.tools().len()
        ));
    }
    let presentational = layout.surfaces().iter().all(|surface| {
        surface
            .descriptor()
            .capabilities()
            .iter()
            .all(|capability| {
                matches!(
                    capability,
                    Capability::SignalEmit | Capability::SignalSubscribe
                )
            })
    });
    if !presentational {
        return Err(String::from(
            "canonical workspace holds a non-presentation capability",
        ));
    }
    Ok(())
}

/// Schema enums match the Rust wire values in order.
#[test]
fn workspace_schema_enums_match_rust_wire_values() -> Result<(), String> {
    let path = protocol_path("schema/WorkspaceLayout.json");
    let schema = read_json(&path)?;

    if schema["properties"]["schema_version"]["const"] != WORKSPACE_LAYOUT_SCHEMA_VERSION {
        return Err(String::from("schema_version const drifted"));
    }
    let cases: &[(&str, Vec<String>)] = &[
        (
            "/definitions/ChromePlacement/enum",
            wire_values(&[
                ChromePlacement::Top,
                ChromePlacement::Right,
                ChromePlacement::Bottom,
                ChromePlacement::Left,
            ])?,
        ),
        (
            "/definitions/WorkspaceChrome/properties/default_theme/enum",
            wire_values(&[WorkspaceTheme::Calm, WorkspaceTheme::Retro])?,
        ),
        (
            "/definitions/WorkspacePresentation/properties/role/enum",
            wire_values(&[WorkspaceSurfaceRole::Panel, WorkspaceSurfaceRole::Context])?,
        ),
        (
            "/definitions/WorkspacePresentation/properties/slot/enum",
            wire_values(&[
                WorkspaceSlot::Primary,
                WorkspaceSlot::Secondary,
                WorkspaceSlot::Tertiary,
                WorkspaceSlot::Overlay,
            ])?,
        ),
        (
            "/definitions/WorkspaceTool/properties/icon/enum",
            wire_values(&[
                WorkspaceToolIcon::Cursor,
                WorkspaceToolIcon::Compose,
                WorkspaceToolIcon::Duplicate,
                WorkspaceToolIcon::Remove,
            ])?,
        ),
    ];
    for case in cases {
        let schema_vals = schema_enum(&schema, case.0)?;
        if schema_vals != case.1 {
            return Err(format!("schema enum drifted at {}", case.0));
        }
    }
    Ok(())
}

/// Web-renderer output is deterministic and matches the tracked artifact.
#[test]
fn web_renderer_input_is_deterministic_and_matches_tracked_artifact() -> Result<(), String> {
    let layout = canonical_layout()?;
    let renderer_input = layout
        .web_renderer_input()
        .map_err(|error| format!("canonical workspace must produce web input: {error}"))?;
    let native_leak = renderer_input
        .surfaces()
        .iter()
        .any(|surface| surface.descriptor().surface() == WidgetSurface::Native);
    if native_leak {
        return Err(String::from("native-only surface leaked into web input"));
    }

    let first = lgwks_std::json::to_string_pretty(&renderer_input)
        .map(|text| {
            let mut bytes = text.into_bytes();
            bytes.push(b'\n');
            bytes
        })
        .map_err(|error| format!("serialize: {error}"))?;
    let second = lgwks_std::json::to_string_pretty(&renderer_input)
        .map(|text| {
            let mut bytes = text.into_bytes();
            bytes.push(b'\n');
            bytes
        })
        .map_err(|error| format!("serialize: {error}"))?;
    if first != second {
        return Err(String::from("web generation is not deterministic"));
    }

    let tracked = read_file(&protocol_path("workspaces/workspace-landing.web.json"))?;
    if first != tracked {
        return Err(String::from("tracked web renderer input drifted"));
    }
    Ok(())
}

/// Privileged, duplicated, or future-versioned layouts fail closed.
#[test]
fn privileged_or_ambiguous_layouts_fail_closed() -> Result<(), String> {
    let mut privileged = canonical_layout()?;
    privileged.surfaces_mut()[0]
        .descriptor_mut()
        .capabilities_mut()
        .push(Capability::TapeRead);
    let failure = match privileged.web_renderer_input() {
        Ok(_) => return Err(String::from("privileged capability validated")),
        Err(error) => error,
    };
    if failure
        != WorkspaceLayoutError::new(
            WorkspaceLayoutIssue::PrivilegedWidgetCapability,
            "surfaces[].descriptor.capabilities",
        )
    {
        return Err(format!(
            "wrong failure for privileged capability: {failure}"
        ));
    }

    let mut duplicate = canonical_layout()?;
    let twin = duplicate.surfaces()[0].descriptor().widget_id().to_owned();
    duplicate.surfaces_mut()[1]
        .descriptor_mut()
        .set_widget_id(twin);
    let failure = match duplicate.validate() {
        Ok(()) => return Err(String::from("duplicate widget id validated")),
        Err(error) => error,
    };
    if failure
        != WorkspaceLayoutError::new(
            WorkspaceLayoutIssue::DuplicateWidgetId,
            "surfaces[].descriptor.widget_id",
        )
    {
        return Err(format!("wrong failure for duplicate widget id: {failure}"));
    }

    let mut future = canonical_layout()?;
    future.set_schema_version(WORKSPACE_LAYOUT_SCHEMA_VERSION.saturating_add(1));
    let failure = match future.validate() {
        Ok(()) => return Err(String::from("future schema version validated")),
        Err(error) => error,
    };
    if failure
        != WorkspaceLayoutError::new(
            WorkspaceLayoutIssue::UnsupportedSchemaVersion,
            "schema_version",
        )
    {
        return Err(format!(
            "wrong failure for future schema version: {failure}"
        ));
    }
    Ok(())
}

/// Unknown fields are rejected by the fail-closed decoder.
#[test]
fn unknown_workspace_fields_fail_closed() -> Result<(), String> {
    let path = protocol_path("workspaces/workspace-landing.json");
    let bytes = read_file(&path)?;
    let canonical =
        String::from_utf8(bytes).map_err(|error| format!("canonical workspace: {error}"))?;
    let unknown = canonical.replacen(
        "\"schema_version\": 1,",
        "\"schema_version\": 1, \"unexpected\": true,",
        1,
    );
    if lgwks_std::json::from_str::<WorkspaceLayout>(&unknown).is_ok() {
        return Err(String::from("unknown field was accepted"));
    }
    Ok(())
}

/// Length limits count Unicode code points, matching the JSON Schema.
#[test]
fn unicode_lengths_follow_json_schema_code_points() -> Result<(), String> {
    let mut accepted = canonical_layout()?;
    accepted.set_title("👩\u{200d}💻".to_owned());
    accepted
        .validate()
        .map_err(|error| format!("format characters must not count as controls: {error}"))?;
    accepted.set_title("é".repeat(80));
    accepted
        .validate()
        .map_err(|error| format!("80 code points must satisfy maxLength 80: {error}"))?;

    accepted.set_title(format!("{}é", accepted.title()));
    let failure = match accepted.validate() {
        Ok(()) => return Err(String::from("81 code points validated")),
        Err(error) => error,
    };
    if failure != WorkspaceLayoutError::new(WorkspaceLayoutIssue::InvalidTitle, "title") {
        return Err(format!("wrong failure for overlong title: {failure}"));
    }
    Ok(())
}

/// Mount flags default off, opt in through setters, and keep validation.
#[test]
fn mount_flags_default_off_opt_in_and_keep_validation() -> Result<(), String> {
    let mut layout = canonical_layout()?;
    let descriptor = layout.surfaces_mut()[0].descriptor_mut();
    if descriptor.self_listen() || descriptor.pinned() {
        return Err(String::from("mount flags must default off"));
    }
    descriptor.set_self_listen(true);
    descriptor.set_pinned(true);
    if !descriptor.self_listen() || !descriptor.pinned() {
        return Err(String::from("mount flags must opt in"));
    }
    layout
        .validate()
        .map_err(|error| format!("flags must keep validation: {error}"))?;
    Ok(())
}

/// Numeric identifier segments follow the wire schema.
#[test]
fn numeric_identifier_segments_follow_the_wire_schema() -> Result<(), String> {
    let mut layout = canonical_layout()?;
    layout.set_layout_id("workspace.2026".to_owned());
    layout.surfaces_mut()[0]
        .descriptor_mut()
        .set_widget_id("canvas.123".to_owned());
    layout.surfaces_mut()[0]
        .descriptor_mut()
        .set_kind("panel.2-d".to_owned());
    layout
        .web_renderer_input()
        .map_err(|error| format!("numeric suffix segments must be schema-valid: {error}"))?;
    for invalid in [
        "1.canvas",
        "canvas..1",
        "canvas.-1",
        "canvas.1_2",
        "canvas.1A",
    ] {
        layout.set_layout_id(invalid.to_owned());
        if layout.validate().is_ok() {
            return Err(format!("invalid identifier validated: {invalid}"));
        }
    }
    Ok(())
}
