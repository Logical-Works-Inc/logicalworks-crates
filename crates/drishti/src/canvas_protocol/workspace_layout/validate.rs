//! The workspace layout contract checks: chrome, surfaces, widgets,
//! presentations and tools, each failing with the issue and path the schema
//! names. Split from the data types so the rules that judge a layout and the
//! layout itself are separate jobs; the types stay the only owners of state.

use std::collections::BTreeSet;

use super::{
    WorkspaceChrome, WorkspaceLayoutError, WorkspaceLayoutIssue, WorkspacePresentation,
    WorkspaceSurface, WorkspaceTool, WorkspaceWidgetDescriptor,
};
use crate::canvas_protocol::{Capability, ChromePlacement, WorkspaceSlot, WorkspaceSurfaceRole};

/// Fail the validation with `issue` at `at` when `condition` is false.
pub(super) fn ensure(
    condition: bool,
    issue: WorkspaceLayoutIssue,
    at: &'static str,
) -> Result<(), WorkspaceLayoutError> {
    if condition {
        Ok(())
    } else {
        Err(WorkspaceLayoutError::new(issue, at))
    }
}

/// Check the chrome placement contract.
pub(super) fn validate_chrome(chrome: &WorkspaceChrome) -> Result<(), WorkspaceLayoutError> {
    let unique: BTreeSet<ChromePlacement> = chrome.allowed_placements.iter().copied().collect();
    let valid_count = (1..=4).contains(&chrome.allowed_placements.len());
    let includes_default = unique.contains(&chrome.default_placement);
    let valid = valid_count && unique.len() == chrome.allowed_placements.len() && includes_default;
    ensure(valid, WorkspaceLayoutIssue::InvalidChrome, "chrome")
}

/// Check the surface list: count, descriptors, presentations, identities.
pub(super) fn validate_surfaces(surfaces: &[WorkspaceSurface]) -> Result<(), WorkspaceLayoutError> {
    ensure(
        (1..=32).contains(&surfaces.len()),
        WorkspaceLayoutIssue::InvalidSurfaceCount,
        "surfaces",
    )?;
    let mut widget_ids = BTreeSet::new();
    let mut orders = BTreeSet::new();
    for surface in surfaces {
        validate_surface(surface, &mut widget_ids, &mut orders)?;
    }
    Ok(())
}

/// Check one surface against the running identity sets.
fn validate_surface<'a>(
    surface: &'a WorkspaceSurface,
    widget_ids: &mut BTreeSet<&'a str>,
    orders: &mut BTreeSet<u8>,
) -> Result<(), WorkspaceLayoutError> {
    validate_widget(&surface.descriptor)?;
    validate_presentation(&surface.presentation)?;
    validate_surface_identity(surface, widget_ids, orders)
}

/// Check one surface's widget id and presentation order are unique.
fn validate_surface_identity<'a>(
    surface: &'a WorkspaceSurface,
    widget_ids: &mut BTreeSet<&'a str>,
    orders: &mut BTreeSet<u8>,
) -> Result<(), WorkspaceLayoutError> {
    ensure(
        widget_ids.insert(surface.descriptor.widget_id.as_str()),
        WorkspaceLayoutIssue::DuplicateWidgetId,
        "surfaces[].descriptor.widget_id",
    )?;
    ensure(
        orders.insert(surface.presentation.order),
        WorkspaceLayoutIssue::DuplicateSurfaceOrder,
        "surfaces[].presentation.order",
    )
}

/// Check one widget descriptor, including the presentation-only capability gate.
fn validate_widget(widget: &WorkspaceWidgetDescriptor) -> Result<(), WorkspaceLayoutError> {
    let pattern_count_ok = widget.emits.len() <= 64 && widget.listens.len() <= 64;
    let patterns_ok = widget
        .emits
        .iter()
        .chain(widget.listens.iter())
        .all(|pattern| valid_text(&pattern.kind_glob, 160));
    let descriptor_is_valid = is_qualified_id(&widget.widget_id)
        && is_kind(&widget.kind)
        && is_semver_triplet(&widget.version)
        && pattern_count_ok
        && patterns_ok;
    ensure(
        descriptor_is_valid,
        WorkspaceLayoutIssue::InvalidWidgetDescriptor,
        "surfaces[].descriptor",
    )?;
    let capability_set: BTreeSet<u8> = widget.capabilities.iter().map(capability_rank).collect();
    let capabilities_are_unique = capability_set.len() == widget.capabilities.len();
    let capabilities_are_presentational = widget.capabilities.iter().all(|capability| {
        matches!(
            capability,
            Capability::SignalEmit | Capability::SignalSubscribe
        )
    });
    ensure(
        capabilities_are_unique && capabilities_are_presentational,
        WorkspaceLayoutIssue::PrivilegedWidgetCapability,
        "surfaces[].descriptor.capabilities",
    )
}

/// Check presentation metadata and the role/slot coupling.
fn validate_presentation(presentation: &WorkspacePresentation) -> Result<(), WorkspaceLayoutError> {
    let role_matches_slot = match presentation.role {
        WorkspaceSurfaceRole::Panel => presentation.slot != WorkspaceSlot::Overlay,
        WorkspaceSurfaceRole::Context => presentation.slot == WorkspaceSlot::Overlay,
    };
    let valid =
        valid_text(&presentation.title, 80) && presentation.order <= 31 && role_matches_slot;
    ensure(
        valid,
        WorkspaceLayoutIssue::InvalidPresentation,
        "surfaces[].presentation",
    )
}

/// Check the tool list: count, ids, labels, and uniqueness.
pub(super) fn validate_tools(tools: &[WorkspaceTool]) -> Result<(), WorkspaceLayoutError> {
    ensure(
        (1..=16).contains(&tools.len()),
        WorkspaceLayoutIssue::InvalidToolCount,
        "tools",
    )?;
    let mut tool_ids = BTreeSet::new();
    for tool in tools {
        let id_ok = tool
            .tool_id
            .strip_prefix("canvas.")
            .is_some_and(is_tool_slug);
        ensure(
            id_ok && valid_text(&tool.label, 40),
            WorkspaceLayoutIssue::InvalidTool,
            "tools[]",
        )?;
        ensure(
            tool_ids.insert(tool.tool_id.as_str()),
            WorkspaceLayoutIssue::DuplicateToolId,
            "tools[].tool_id",
        )?;
    }
    Ok(())
}

/// Rank capabilities so duplicates collapse in a set.
fn capability_rank(capability: &Capability) -> u8 {
    match *capability {
        Capability::SignalEmit => 0,
        Capability::SignalSubscribe => 1,
        Capability::TapeRead => 2,
        Capability::ViewInject => 3,
        Capability::IntentEmit => 4,
        Capability::MotionSchedule => 5,
        Capability::MotionObserve => 6,
        Capability::MotionPatch => 7,
        Capability::MotionPluginRegister => 8,
        Capability::MotionReplay => 9,
        Capability::Shred => 10,
        Capability::Rtbf => 11,
        Capability::RemoteCompute => 12,
    }
}

/// Required text: non-empty, at most `maximum` code points, no controls.
pub(super) fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && valid_optional_text(value, maximum)
}

/// Optional text: at most `maximum` code points, no controls.
pub(super) fn valid_optional_text(value: &str, maximum: usize) -> bool {
    value.chars().count() <= maximum && !value.chars().any(char::is_control)
}

/// Qualified id: two or more dot/dash segments, slug first, lowercase rest.
pub(super) fn is_qualified_id(value: &str) -> bool {
    let mut segments = value.split(['.', '-']);
    let first = segments.next();
    let remaining: Vec<&str> = segments.collect();
    first.is_some_and(is_slug) && !remaining.is_empty() && remaining.into_iter().all(is_id_segment)
}

/// Kind: one or more dot/dash segments, slug first, lowercase rest.
fn is_kind(value: &str) -> bool {
    let mut segments = value.split(['.', '-']);
    segments.next().is_some_and(is_slug) && segments.all(is_id_segment)
}

/// Lowercase alphanumeric segment, non-empty.
fn is_id_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Slug: starts lowercase, then lowercase or digits.
fn is_slug(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Tool slug: starts lowercase, then lowercase, digits, or dash.
fn is_tool_slug(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// `N.N.N` digit triplet with exactly two dots.
fn is_semver_triplet(value: &str) -> bool {
    let mut segments = value.split('.');
    let triple_ok = (&mut segments)
        .take(3)
        .all(|segment| !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit()));
    triple_ok && segments.next().is_none() && value.matches('.').count() == 2
}
