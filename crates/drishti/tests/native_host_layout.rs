//! Strict workspace loading.
//!
//! Counterpart of `CanvasProtocolTests/WorkspaceLayoutTests.swift` plus the
//! drift cases (kernel#850 row 15): the canonical file loads validated,
//! extra keys and bool/int swaps fail at decode, privileged capabilities
//! fail at validation, and duplicate orders report the order issue (where
//! the Swift validator reported presentation, the canonical Rust contract
//! reports `DuplicateSurfaceOrder`).

use drishti::canvas_protocol::workspace_layout::{
    WorkspaceLayout, WorkspaceLayoutError, WorkspaceLayoutIssue, load_validated,
};
use std::path::PathBuf;

/// Absorbed protocol tree.
fn protocol_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets/canvas-protocol")
        .join(relative)
}

/// Read a file or fail with its path.
fn read_file(path: &PathBuf) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

/// The canonical workspace loads validated.
#[test]
fn canonical_loads_validated() -> Result<(), String> {
    let bytes = read_file(&protocol_path("workspaces/workspace-landing.json"))?;
    let layout = load_validated(&bytes).map_err(|error| format!("{error}"))?;
    if layout.layout_id() != "workspace.landing" {
        return Err(String::from("canonical id must load"));
    }
    Ok(())
}

/// Extra keys fail at decode, not validation.
#[test]
fn extra_keys_fail_at_decode() -> Result<(), String> {
    let bytes = read_file(&protocol_path("workspaces/workspace-landing.json"))?;
    let text = String::from_utf8(bytes).map_err(|error| format!("{error}"))?;
    let unknown = text.replacen(
        "\"schema_version\": 1,",
        "\"schema_version\": 1, \"unexpected\": true,",
        1,
    );
    match load_validated(unknown.as_bytes()) {
        Err(drishti::canvas_protocol::workspace_layout::WorkspaceLoadError::Decode(_)) => Ok(()),
        Err(error) => Err(format!("wrong failure: {error}")),
        Ok(_) => Err(String::from("unknown field must fail")),
    }
}

/// Boolean/integer swaps fail at decode through typed fields.
#[test]
fn bool_int_swaps_fail_at_decode() -> Result<(), String> {
    let bytes = read_file(&protocol_path("workspaces/workspace-landing.json"))?;
    let text = String::from_utf8(bytes).map_err(|error| format!("{error}"))?;
    // `collapsible` is a bool; 1 must not decode as one.
    let swapped = text.replacen("\"collapsible\": true", "\"collapsible\": 1", 1);
    if load_validated(swapped.as_bytes()).is_ok() {
        return Err(String::from("int-for-bool must fail"));
    }
    // `order` is an integer; true must not decode as one.
    let swapped = text.replacen("\"order\": 0", "\"order\": true", 1);
    if load_validated(swapped.as_bytes()).is_ok() {
        return Err(String::from("bool-for-int must fail"));
    }
    Ok(())
}

/// Privileged capabilities and duplicate orders fail at validation.
#[test]
fn privileged_and_duplicate_orders_fail_at_validation() -> Result<(), String> {
    let bytes = read_file(&protocol_path("workspaces/workspace-landing.json"))?;
    let mut layout: WorkspaceLayout = lgwks_std::json::from_slice(&bytes)
        .map_err(|error| format!("canonical must decode: {error}"))?;
    layout.surfaces_mut()[0]
        .descriptor_mut()
        .capabilities_mut()
        .push(drishti::canvas_protocol::Capability::TapeRead);
    match layout.validate() {
        Err(error)
            if error
                == WorkspaceLayoutError::new(
                    WorkspaceLayoutIssue::PrivilegedWidgetCapability,
                    "surfaces[].descriptor.capabilities",
                ) => {}
        Err(error) => return Err(format!("wrong failure: {error}")),
        Ok(()) => return Err(String::from("privileged cap must fail")),
    }
    let bytes = read_file(&protocol_path("workspaces/workspace-landing.json"))?;
    let mut layout: WorkspaceLayout =
        lgwks_std::json::from_slice(&bytes).map_err(|error| format!("{error}"))?;
    let order = layout.surfaces()[0].presentation().order();
    layout.surfaces_mut()[1].presentation_mut().set_order(order);
    match layout.validate() {
        Err(error)
            if error
                == WorkspaceLayoutError::new(
                    WorkspaceLayoutIssue::DuplicateSurfaceOrder,
                    "surfaces[].presentation.order",
                ) =>
        {
            Ok(())
        }
        Err(error) => Err(format!("wrong failure: {error}")),
        Ok(()) => Err(String::from("duplicate order must fail")),
    }
}
