//! Chrome, surface, presentation, and tool parts of a workspace layout.

use lgwks_std::json::serde::{Deserialize, Serialize};

use super::WorkspaceWidgetDescriptor;
use crate::canvas_protocol::{
    ChromePlacement, WorkspaceSlot, WorkspaceSurfaceRole, WorkspaceTheme, WorkspaceToolIcon,
};

/// Chrome placement contract: default plus the allowed set.
///
/// Fields are private: construction runs through [`WorkspaceChrome::new`]
/// and reads go through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
/// use drishti::canvas_protocol::workspace_layout::WorkspaceChrome;
///
/// let chrome = WorkspaceChrome::new(
///     ChromePlacement::Left, vec![ChromePlacement::Left, ChromePlacement::Right], false, WorkspaceTheme::Retro,
/// );
/// assert_eq!(chrome.default_placement(), ChromePlacement::Left);
/// assert_eq!(chrome.allowed_placements().len(), 2);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkspaceChrome {
    /// Placement used when no override applies.
    pub(super) default_placement: ChromePlacement,
    /// Allowed placements, 1..=4 unique entries containing the default.
    pub(super) allowed_placements: Vec<ChromePlacement>,
    /// Whether the chrome can collapse.
    collapsible: bool,
    /// Default visual theme.
    default_theme: WorkspaceTheme,
}

impl WorkspaceChrome {
    /// Assemble a chrome contract from its parts.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// use drishti::canvas_protocol::workspace_layout::WorkspaceChrome;
    ///
    /// let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// assert!(chrome.collapsible());
    /// assert_eq!(chrome.default_theme(), WorkspaceTheme::Calm);
    /// ```
    #[must_use]
    pub fn new(
        default_placement: ChromePlacement,
        allowed_placements: Vec<ChromePlacement>,
        collapsible: bool,
        default_theme: WorkspaceTheme,
    ) -> Self {
        Self {
            default_placement,
            allowed_placements,
            collapsible,
            default_theme,
        }
    }

    /// Placement used when no override applies; validation requires it to
    /// be one of the allowed placements.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceChrome;
    /// let chrome = WorkspaceChrome::new(ChromePlacement::Bottom, vec![ChromePlacement::Bottom], true, WorkspaceTheme::Calm);
    /// assert_eq!(chrome.default_placement(), ChromePlacement::Bottom);
    /// ```
    #[must_use]
    pub const fn default_placement(&self) -> ChromePlacement {
        self.default_placement
    }

    /// Placements the chrome may dock at: 1..=4 unique entries once validated.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceChrome;
    /// let both = vec![ChromePlacement::Top, ChromePlacement::Bottom];
    /// let chrome = WorkspaceChrome::new(ChromePlacement::Top, both.clone(), true, WorkspaceTheme::Calm);
    /// assert_eq!(chrome.allowed_placements(), both.as_slice());
    /// ```
    #[must_use]
    pub fn allowed_placements(&self) -> &[ChromePlacement] {
        &self.allowed_placements
    }

    /// Whether the chrome can collapse.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceChrome;
    /// let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], false, WorkspaceTheme::Calm);
    /// assert!(!chrome.collapsible());
    /// ```
    #[must_use]
    pub const fn collapsible(&self) -> bool {
        self.collapsible
    }

    /// Look the chrome starts in until the user picks another (calm or retro).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceChrome;
    /// let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Retro);
    /// assert_eq!(chrome.default_theme(), WorkspaceTheme::Retro);
    /// ```
    #[must_use]
    pub const fn default_theme(&self) -> WorkspaceTheme {
        self.default_theme
    }
}

/// One widget surface: descriptor plus its presentation metadata.
///
/// Fields are private: construction runs through [`WorkspaceSurface::new`]
/// and reads go through the accessors; mutation runs through the explicit
/// `*_mut` set.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::{WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole};
/// use drishti::canvas_protocol::workspace_layout::{WorkspacePresentation, WorkspaceSurface, WorkspaceWidgetDescriptor};
///
/// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
/// let view = WorkspacePresentation::new("Panel".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 0);
/// let surface = WorkspaceSurface::new(widget, view);
/// assert_eq!(surface.presentation().title(), "Panel");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkspaceSurface {
    /// Widget identity, kind, version, and signal contract.
    pub(super) descriptor: WorkspaceWidgetDescriptor,
    /// Title, role, slot, and ordering.
    pub(super) presentation: WorkspacePresentation,
}

impl WorkspaceSurface {
    /// Assemble a surface from descriptor and presentation.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::{WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole};
    /// use drishti::canvas_protocol::workspace_layout::{WorkspacePresentation, WorkspaceSurface, WorkspaceWidgetDescriptor};
    ///
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// let view = WorkspacePresentation::new("Panel".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 0);
    /// let surface = WorkspaceSurface::new(widget.clone(), view);
    /// assert_eq!(surface.descriptor(), &widget);
    /// ```
    #[must_use]
    pub fn new(descriptor: WorkspaceWidgetDescriptor, presentation: WorkspacePresentation) -> Self {
        Self {
            descriptor,
            presentation,
        }
    }

    /// Widget identity, kind, version, and signal contract.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole};
    /// # use drishti::canvas_protocol::workspace_layout::{WorkspacePresentation, WorkspaceSurface, WorkspaceWidgetDescriptor};
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// let view = WorkspacePresentation::new("Panel".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 0);
    /// let surface = WorkspaceSurface::new(widget, view);
    /// assert_eq!(surface.descriptor().widget_id(), "widget.demo");
    /// ```
    #[must_use]
    pub const fn descriptor(&self) -> &WorkspaceWidgetDescriptor {
        &self.descriptor
    }

    /// Mutable descriptor (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole};
    /// # use drishti::canvas_protocol::workspace_layout::{WorkspacePresentation, WorkspaceSurface, WorkspaceWidgetDescriptor};
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// let view = WorkspacePresentation::new("Panel".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 0);
    /// let mut surface = WorkspaceSurface::new(widget, view);
    /// surface.descriptor_mut().set_version("2.0.0".into());
    /// assert_eq!(surface.descriptor().version(), "2.0.0");
    /// ```
    pub fn descriptor_mut(&mut self) -> &mut WorkspaceWidgetDescriptor {
        &mut self.descriptor
    }

    /// Title, role, slot, and ordering.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole};
    /// # use drishti::canvas_protocol::workspace_layout::{WorkspacePresentation, WorkspaceSurface, WorkspaceWidgetDescriptor};
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// let view = WorkspacePresentation::new("Panel".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Tertiary, 4);
    /// let surface = WorkspaceSurface::new(widget, view);
    /// assert_eq!(surface.presentation().order(), 4);
    /// ```
    #[must_use]
    pub const fn presentation(&self) -> &WorkspacePresentation {
        &self.presentation
    }

    /// Mutable presentation (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole};
    /// # use drishti::canvas_protocol::workspace_layout::{WorkspacePresentation, WorkspaceSurface, WorkspaceWidgetDescriptor};
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// let view = WorkspacePresentation::new("Panel".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 0);
    /// let mut surface = WorkspaceSurface::new(widget, view);
    /// surface.presentation_mut().set_order(9);
    /// assert_eq!(surface.presentation().order(), 9);
    /// ```
    pub fn presentation_mut(&mut self) -> &mut WorkspacePresentation {
        &mut self.presentation
    }
}

/// Presentation metadata for one surface.
///
/// Fields are private: construction runs through
/// [`WorkspacePresentation::new`], reads go through the accessors, and order
/// mutation runs through [`WorkspacePresentation::set_order`].
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::{WorkspaceSlot, WorkspaceSurfaceRole};
/// use drishti::canvas_protocol::workspace_layout::WorkspacePresentation;
///
/// let view = WorkspacePresentation::new("Context".into(), WorkspaceSurfaceRole::Context, WorkspaceSlot::Overlay, 2);
/// assert_eq!((view.role(), view.slot()), (WorkspaceSurfaceRole::Context, WorkspaceSlot::Overlay));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkspacePresentation {
    /// Surface title, 1..=80 code points.
    pub(super) title: String,
    /// Panel or context role.
    pub(super) role: WorkspaceSurfaceRole,
    /// Slot the surface occupies.
    pub(super) slot: WorkspaceSlot,
    /// Ordering key, unique within the layout and at most 31.
    pub(super) order: u8,
}

impl WorkspacePresentation {
    /// Assemble presentation metadata from its parts.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::{WorkspaceSlot, WorkspaceSurfaceRole};
    /// use drishti::canvas_protocol::workspace_layout::WorkspacePresentation;
    ///
    /// let view = WorkspacePresentation::new("Panel".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 3);
    /// assert_eq!((view.title(), view.order()), ("Panel", 3));
    /// ```
    #[must_use]
    pub fn new(title: String, role: WorkspaceSurfaceRole, slot: WorkspaceSlot, order: u8) -> Self {
        Self {
            title,
            role,
            slot,
            order,
        }
    }

    /// Surface title shown on its panel header: 1..=80 code points and no
    /// control characters once validated.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{WorkspaceSlot, WorkspaceSurfaceRole};
    /// # use drishti::canvas_protocol::workspace_layout::WorkspacePresentation;
    /// let view = WorkspacePresentation::new("Notes".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 0);
    /// assert_eq!(view.title(), "Notes");
    /// ```
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Panel or context role; a context surface must sit in the overlay slot.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{WorkspaceSlot, WorkspaceSurfaceRole};
    /// # use drishti::canvas_protocol::workspace_layout::WorkspacePresentation;
    /// let view = WorkspacePresentation::new("Notes".into(), WorkspaceSurfaceRole::Context, WorkspaceSlot::Overlay, 0);
    /// assert_eq!(view.role(), WorkspaceSurfaceRole::Context);
    /// ```
    #[must_use]
    pub const fn role(&self) -> WorkspaceSurfaceRole {
        self.role
    }

    /// Slot the surface occupies.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{WorkspaceSlot, WorkspaceSurfaceRole};
    /// # use drishti::canvas_protocol::workspace_layout::WorkspacePresentation;
    /// let view = WorkspacePresentation::new("Notes".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Secondary, 0);
    /// assert_eq!(view.slot(), WorkspaceSlot::Secondary);
    /// ```
    #[must_use]
    pub const fn slot(&self) -> WorkspaceSlot {
        self.slot
    }

    /// Ordering key: unique within the layout and at most 31 once validated.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{WorkspaceSlot, WorkspaceSurfaceRole};
    /// # use drishti::canvas_protocol::workspace_layout::WorkspacePresentation;
    /// let view = WorkspacePresentation::new("Notes".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 31);
    /// assert_eq!(view.order(), 31);
    /// ```
    #[must_use]
    pub const fn order(&self) -> u8 {
        self.order
    }

    /// Replace the ordering key (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{WorkspaceSlot, WorkspaceSurfaceRole};
    /// # use drishti::canvas_protocol::workspace_layout::WorkspacePresentation;
    /// let mut view = WorkspacePresentation::new("Notes".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 0);
    /// view.set_order(5);
    /// assert_eq!(view.order(), 5);
    /// ```
    pub const fn set_order(&mut self, order: u8) {
        self.order = order;
    }
}

/// One command tool in the workspace.
///
/// Fields are private: construction runs through [`WorkspaceTool::new`],
/// reads go through the accessors, and id mutation runs through
/// [`WorkspaceTool::set_tool_id`].
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::WorkspaceToolIcon;
/// use drishti::canvas_protocol::workspace_layout::WorkspaceTool;
///
/// let tool = WorkspaceTool::new("canvas.compose".into(), "Compose".into(), WorkspaceToolIcon::Compose);
/// assert_eq!((tool.tool_id(), tool.label()), ("canvas.compose", "Compose"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkspaceTool {
    /// Tool id (`canvas.` plus a slug).
    pub(super) tool_id: String,
    /// Human label, 1..=40 code points.
    pub(super) label: String,
    /// Icon for the tool.
    icon: WorkspaceToolIcon,
}

impl WorkspaceTool {
    /// Assemble a tool from its parts.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::WorkspaceToolIcon;
    /// use drishti::canvas_protocol::workspace_layout::WorkspaceTool;
    ///
    /// let tool = WorkspaceTool::new("canvas.drive".into(), "Drive".into(), WorkspaceToolIcon::Cursor);
    /// assert_eq!(tool.icon(), WorkspaceToolIcon::Cursor);
    /// ```
    #[must_use]
    pub fn new(tool_id: String, label: String, icon: WorkspaceToolIcon) -> Self {
        Self {
            tool_id,
            label,
            icon,
        }
    }

    /// Tool id: `canvas.` plus a lowercase slug, unique within the layout.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WorkspaceToolIcon;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceTool;
    /// let tool = WorkspaceTool::new("canvas.drive".into(), "Drive".into(), WorkspaceToolIcon::Cursor);
    /// assert_eq!(tool.tool_id(), "canvas.drive");
    /// ```
    #[must_use]
    pub fn tool_id(&self) -> &str {
        &self.tool_id
    }

    /// Replace the tool id (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WorkspaceToolIcon;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceTool;
    /// let mut tool = WorkspaceTool::new("canvas.drive".into(), "Drive".into(), WorkspaceToolIcon::Cursor);
    /// tool.set_tool_id("canvas.other".into());
    /// assert_eq!(tool.tool_id(), "canvas.other");
    /// ```
    pub fn set_tool_id(&mut self, tool_id: String) {
        self.tool_id = tool_id;
    }

    /// Label shown beside the tool's icon: 1..=40 code points once validated.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WorkspaceToolIcon;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceTool;
    /// let tool = WorkspaceTool::new("canvas.drive".into(), "Drive".into(), WorkspaceToolIcon::Cursor);
    /// assert_eq!(tool.label(), "Drive");
    /// ```
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Glyph drawn for the tool in the command strip.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WorkspaceToolIcon;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceTool;
    /// let tool = WorkspaceTool::new("canvas.rm".into(), "Remove".into(), WorkspaceToolIcon::Remove);
    /// assert_eq!(tool.icon(), WorkspaceToolIcon::Remove);
    /// ```
    #[must_use]
    pub const fn icon(&self) -> WorkspaceToolIcon {
        self.icon
    }
}
