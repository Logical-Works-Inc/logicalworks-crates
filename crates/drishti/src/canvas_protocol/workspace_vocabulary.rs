//! Workspace chrome, surface, and tool vocabulary for validated layouts.
//!
//! These value types sit beside the protocol vocabulary because the layout
//! validator, the web-renderer projection, and the schema-lock tests all name
//! them directly; [`super::workspace_layout`] owns assembly, validation, and
//! the projection itself.

use lgwks_std::json::serde::{Deserialize, Serialize};

/// Where chrome may dock.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::ChromePlacement;
///
/// let wire = lgwks_std::json::to_string(&ChromePlacement::Top)
///     .map_err(|error| error.to_string())?;
/// assert_eq!(wire, "\"top\"");
/// assert!(lgwks_std::json::from_str::<ChromePlacement>("\"center\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", rename_all = "lowercase")]
#[non_exhaustive]
pub enum ChromePlacement {
    /// Top edge.
    Top,
    /// Right edge.
    Right,
    /// Bottom edge.
    Bottom,
    /// Left edge.
    Left,
}

/// Visual theme for the workspace chrome.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::WorkspaceTheme;
///
/// let wire = lgwks_std::json::to_string(&WorkspaceTheme::Retro)
///     .map_err(|error| error.to_string())?;
/// assert_eq!(wire, "\"retro\"");
/// assert!(lgwks_std::json::from_str::<WorkspaceTheme>("\"neon\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", rename_all = "lowercase")]
#[non_exhaustive]
pub enum WorkspaceTheme {
    /// Calm theme.
    Calm,
    /// Retro theme.
    Retro,
}

/// Surface a widget can render on.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::WidgetSurface;
///
/// let wire = lgwks_std::json::to_string(&WidgetSurface::Both)
///     .map_err(|error| error.to_string())?;
/// assert_eq!(wire, "\"both\"");
/// assert!(lgwks_std::json::from_str::<WidgetSurface>("\"tv\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", rename_all = "lowercase")]
#[non_exhaustive]
pub enum WidgetSurface {
    /// Web renderer only.
    Web,
    /// Native host only.
    Native,
    /// Both renderers.
    Both,
}

/// Role a surface plays in the layout.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::WorkspaceSurfaceRole;
///
/// let wire = lgwks_std::json::to_string(&WorkspaceSurfaceRole::Context)
///     .map_err(|error| error.to_string())?;
/// assert_eq!(wire, "\"context\"");
/// assert!(lgwks_std::json::from_str::<WorkspaceSurfaceRole>("\"admin\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", rename_all = "lowercase")]
#[non_exhaustive]
pub enum WorkspaceSurfaceRole {
    /// Regular panel; never in the overlay slot.
    Panel,
    /// Context surface; always in the overlay slot.
    Context,
}

/// Slot a surface occupies.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::WorkspaceSlot;
///
/// let wire = lgwks_std::json::to_string(&WorkspaceSlot::Overlay)
///     .map_err(|error| error.to_string())?;
/// assert_eq!(wire, "\"overlay\"");
/// assert!(lgwks_std::json::from_str::<WorkspaceSlot>("\"quaternary\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", rename_all = "lowercase")]
#[non_exhaustive]
pub enum WorkspaceSlot {
    /// Primary slot.
    Primary,
    /// Secondary slot.
    Secondary,
    /// Tertiary slot.
    Tertiary,
    /// Overlay slot, reserved for context surfaces.
    Overlay,
}

/// Icon for a workspace tool.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::WorkspaceToolIcon;
///
/// let wire = lgwks_std::json::to_string(&WorkspaceToolIcon::Compose)
///     .map_err(|error| error.to_string())?;
/// assert_eq!(wire, "\"compose\"");
/// assert!(lgwks_std::json::from_str::<WorkspaceToolIcon>("\"skull\"").is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", rename_all = "lowercase")]
#[non_exhaustive]
pub enum WorkspaceToolIcon {
    /// Cursor icon.
    Cursor,
    /// Compose icon.
    Compose,
    /// Duplicate icon.
    Duplicate,
    /// Remove icon.
    Remove,
}
