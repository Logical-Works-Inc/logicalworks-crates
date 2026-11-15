//! Versioned, renderer-neutral workspace presentation contract.
//!
//! Absorbed from `kernel/crates/canvas-protocol/src/workspace_layout.rs`
//! (kernel#850 row 15). The JSON Schema
//! (`assets/canvas-protocol/schema/WorkspaceLayout.json`) remains the
//! wire-format source of truth. These types give Rust consumers a fail-closed
//! decoder and a deterministic web-renderer projection without granting
//! storage, broker, or effect authority.

use lgwks_std::json::serde::{Deserialize, Serialize};

use super::WidgetSurface;

mod descriptor;
mod error;
mod surface;
/// The contract checks behind [`WorkspaceLayout::validate`], split out so the
/// data types and the rules that judge them are separate jobs.
mod validate;

pub use descriptor::{WorkspaceSignalPattern, WorkspaceWidgetDescriptor};
pub use error::{WorkspaceLayoutError, WorkspaceLayoutIssue, WorkspaceLoadError};
pub use surface::{WorkspaceChrome, WorkspacePresentation, WorkspaceSurface, WorkspaceTool};

use validate::{
    ensure, is_qualified_id, valid_optional_text, valid_text, validate_chrome, validate_surfaces,
    validate_tools,
};

/// Schema version accepted by [`WorkspaceLayout::validate`].
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::workspace_layout::WORKSPACE_LAYOUT_SCHEMA_VERSION;
///
/// assert_eq!(WORKSPACE_LAYOUT_SCHEMA_VERSION, 1);
/// ```
pub const WORKSPACE_LAYOUT_SCHEMA_VERSION: u16 = 1;

/// A validated workspace: chrome, ordered surfaces, and command tools.
///
/// Fields are private: construction runs through the validating
/// [`WorkspaceLayout::new`] or the strict [`load_validated`], reads go
/// through the accessors, and the test/simulation mutation surface is the
/// explicit setter and `*_mut` set below — never direct field writes.
///
/// # Examples
///
/// ```
/// # use drishti::canvas_protocol::workspace_layout::*;
/// # use drishti::canvas_protocol::{ChromePlacement, WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole, WorkspaceTheme, WorkspaceToolIcon};
/// # let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
/// # let view = WorkspacePresentation::new("Panel".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 0);
/// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
/// # let tool = WorkspaceTool::new("canvas.drive".into(), "Drive".into(), WorkspaceToolIcon::Cursor);
/// let layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![WorkspaceSurface::new(widget, view)], vec![tool]);
/// let wire = lgwks_std::json::to_string(&layout).map_err(|error| error.to_string())?;
/// assert_eq!(load_validated(wire.as_bytes()), Ok(layout));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkspaceLayout {
    /// Wire schema version; must equal [`WORKSPACE_LAYOUT_SCHEMA_VERSION`].
    schema_version: u16,
    /// Qualified workspace id (for example `workspace.landing`).
    layout_id: String,
    /// Human title, 1..=80 code points.
    title: String,
    /// Human subtitle, 0..=120 code points.
    subtitle: String,
    /// Chrome placement contract.
    chrome: WorkspaceChrome,
    /// Ordered widget surfaces, 1..=32.
    surfaces: Vec<WorkspaceSurface>,
    /// Command tools, 1..=16.
    tools: Vec<WorkspaceTool>,
}

impl WorkspaceLayout {
    /// Assemble a layout from its validated parts.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole, WorkspaceTheme, WorkspaceToolIcon};
    /// # let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// # let view = WorkspacePresentation::new("Panel".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 0);
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// # let tool = WorkspaceTool::new("canvas.drive".into(), "Drive".into(), WorkspaceToolIcon::Cursor);
    /// let layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![WorkspaceSurface::new(widget, view)], vec![tool]);
    /// assert_eq!(layout.title(), "Drive");
    /// assert_eq!(layout.surfaces().len(), 1);
    /// ```
    #[must_use]
    pub fn new(
        schema_version: u16,
        title: String,
        layout_id: String,
        subtitle: String,
        chrome: WorkspaceChrome,
        surfaces: Vec<WorkspaceSurface>,
        tools: Vec<WorkspaceTool>,
    ) -> Self {
        Self {
            schema_version,
            layout_id,
            title,
            subtitle,
            chrome,
            surfaces,
            tools,
        }
    }

    /// Fail closed on the first contract violation.
    ///
    /// # Errors
    ///
    /// Returns the first [`WorkspaceLayoutIssue`] found, with the path in
    /// [`WorkspaceLayoutError::at`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole, WorkspaceTheme, WorkspaceToolIcon};
    /// # let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// # let view = WorkspacePresentation::new("Panel".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 0);
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// # let tool = WorkspaceTool::new("canvas.drive".into(), "Drive".into(), WorkspaceToolIcon::Cursor);
    /// let mut layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![WorkspaceSurface::new(widget, view)], vec![tool]);
    /// assert_eq!(layout.validate(), Ok(()));
    /// layout.set_schema_version(2);
    /// let refusal = WorkspaceLayoutError::new(WorkspaceLayoutIssue::UnsupportedSchemaVersion, "schema_version");
    /// assert_eq!(layout.validate(), Err(refusal));
    /// ```
    pub fn validate(&self) -> Result<(), WorkspaceLayoutError> {
        ensure(
            self.schema_version == WORKSPACE_LAYOUT_SCHEMA_VERSION,
            WorkspaceLayoutIssue::UnsupportedSchemaVersion,
            "schema_version",
        )?;
        ensure(
            is_qualified_id(&self.layout_id),
            WorkspaceLayoutIssue::InvalidLayoutId,
            "layout_id",
        )?;
        ensure(
            valid_text(&self.title, 80),
            WorkspaceLayoutIssue::InvalidTitle,
            "title",
        )?;
        ensure(
            valid_optional_text(&self.subtitle, 120),
            WorkspaceLayoutIssue::InvalidSubtitle,
            "subtitle",
        )?;
        validate_chrome(&self.chrome)?;
        validate_surfaces(&self.surfaces)?;
        validate_tools(&self.tools)
    }

    /// Project the layout for the external web renderer.
    ///
    /// Native-only surfaces are omitted and source ordering is preserved, so
    /// repeated generation serializes to byte-identical JSON. The projection
    /// is itself validated: a returned layout always passes
    /// [`WorkspaceLayout::validate`].
    ///
    /// # Errors
    ///
    /// Returns the validation failure when the layout is not valid, and
    /// [`WorkspaceLayoutIssue::InvalidSurfaceCount`] at `surfaces` when every
    /// surface is native-only, because a web renderer then has nothing to show
    /// and an empty surface list is not a valid layout.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole, WorkspaceTheme, WorkspaceToolIcon};
    /// # let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// # let view = WorkspacePresentation::new("Panel".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 0);
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// # let tool = WorkspaceTool::new("canvas.drive".into(), "Drive".into(), WorkspaceToolIcon::Cursor);
    /// let mut layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![WorkspaceSurface::new(widget, view)], vec![tool]);
    /// let native = WorkspaceWidgetDescriptor::new("demo.native".into(), "widget.native".into(), "1.0.0".into(), WidgetSurface::Native, vec![], vec![], vec![]);
    /// let page = WorkspacePresentation::new("Native".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Secondary, 1);
    /// layout.surfaces_mut().push(WorkspaceSurface::new(native, page));
    /// assert_eq!(layout.surfaces().len(), 2);
    /// assert_eq!(layout.web_renderer_input()?.surfaces().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn web_renderer_input(&self) -> Result<WorkspaceLayout, WorkspaceLayoutError> {
        self.validate()?;
        let surfaces: Vec<WorkspaceSurface> = self
            .surfaces
            .iter()
            .filter(|surface| surface.descriptor.surface != WidgetSurface::Native)
            .cloned()
            .collect();
        let projected = WorkspaceLayout::new(
            self.schema_version,
            self.title.clone(),
            self.layout_id.clone(),
            self.subtitle.clone(),
            self.chrome.clone(),
            surfaces,
            self.tools.clone(),
        );
        projected.validate()?;
        Ok(projected)
    }

    /// Wire schema version as stored; [`WorkspaceLayout::validate`] accepts
    /// only [`WORKSPACE_LAYOUT_SCHEMA_VERSION`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme, WorkspaceToolIcon};
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// let layout = WorkspaceLayout::new(7, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![], vec![]);
    /// assert_eq!(layout.schema_version(), 7);
    /// assert!(layout.validate().is_err());
    /// ```
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Replace the wire schema version (falsification and simulation only;
    /// re-validate afterwards).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// let mut layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![], vec![]);
    /// layout.set_schema_version(2);
    /// assert_eq!(layout.schema_version(), 2);
    /// ```
    pub fn set_schema_version(&mut self, schema_version: u16) {
        self.schema_version = schema_version;
    }

    /// Qualified workspace id: dot- or dash-separated lowercase segments
    /// such as `workspace.landing`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// let layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![], vec![]);
    /// assert_eq!(layout.layout_id(), "workspace.drive");
    /// ```
    #[must_use]
    pub fn layout_id(&self) -> &str {
        &self.layout_id
    }

    /// Replace the workspace id (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// let mut layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![], vec![]);
    /// layout.set_layout_id("workspace.other".into());
    /// assert_eq!(layout.layout_id(), "workspace.other");
    /// ```
    pub fn set_layout_id(&mut self, layout_id: String) {
        self.layout_id = layout_id;
    }

    /// Title shown in the workspace header: 1..=80 code points and no
    /// control characters once validated.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// let layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![], vec![]);
    /// assert_eq!(layout.title(), "Drive");
    /// ```
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Replace the title (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// let mut layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![], vec![]);
    /// layout.set_title("Renamed".into());
    /// assert_eq!(layout.title(), "Renamed");
    /// ```
    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    /// Secondary header line: empty is valid, at most 120 code points and no
    /// control characters.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// let layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), "second line".into(), chrome, vec![], vec![]);
    /// assert_eq!(layout.subtitle(), "second line");
    /// ```
    #[must_use]
    pub fn subtitle(&self) -> &str {
        &self.subtitle
    }

    /// Replace the subtitle (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// let mut layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![], vec![]);
    /// layout.set_subtitle("changed".into());
    /// assert_eq!(layout.subtitle(), "changed");
    /// ```
    pub fn set_subtitle(&mut self, subtitle: String) {
        self.subtitle = subtitle;
    }

    /// Chrome placement contract.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Left, vec![ChromePlacement::Left], true, WorkspaceTheme::Calm);
    /// let layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![], vec![]);
    /// assert_eq!(layout.chrome().default_placement(), ChromePlacement::Left);
    /// ```
    #[must_use]
    pub const fn chrome(&self) -> &WorkspaceChrome {
        &self.chrome
    }

    /// Ordered widget surfaces.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme};
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// let layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![], vec![]);
    /// assert!(layout.surfaces().is_empty());
    /// ```
    #[must_use]
    pub fn surfaces(&self) -> &[WorkspaceSurface] {
        &self.surfaces
    }

    /// Mutable widget surfaces (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole, WorkspaceTheme};
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// # let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// # let view = WorkspacePresentation::new("Panel".into(), WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 0);
    /// let mut layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![], vec![]);
    /// layout.surfaces_mut().push(WorkspaceSurface::new(widget, view));
    /// assert_eq!(layout.surfaces().len(), 1);
    /// ```
    pub fn surfaces_mut(&mut self) -> &mut Vec<WorkspaceSurface> {
        &mut self.surfaces
    }

    /// Command tools in declaration order; validation requires 1..=16 with
    /// unique ids.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme, WorkspaceToolIcon};
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// let tool = WorkspaceTool::new("canvas.drive".into(), "Drive".into(), WorkspaceToolIcon::Cursor);
    /// let layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![], vec![tool]);
    /// assert_eq!(layout.tools()[0].tool_id(), "canvas.drive");
    /// ```
    #[must_use]
    pub fn tools(&self) -> &[WorkspaceTool] {
        &self.tools
    }

    /// Mutable command tools (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::workspace_layout::*;
    /// # use drishti::canvas_protocol::{ChromePlacement, WorkspaceTheme, WorkspaceToolIcon};
    /// # let chrome = WorkspaceChrome::new(ChromePlacement::Top, vec![ChromePlacement::Top], true, WorkspaceTheme::Calm);
    /// let mut layout = WorkspaceLayout::new(1, "Drive".into(), "workspace.drive".into(), String::new(), chrome, vec![], vec![]);
    /// layout.tools_mut().push(WorkspaceTool::new("canvas.drive".into(), "Drive".into(), WorkspaceToolIcon::Cursor));
    /// assert_eq!(layout.tools().len(), 1);
    /// ```
    pub fn tools_mut(&mut self) -> &mut Vec<WorkspaceTool> {
        &mut self.tools
    }
}

/// Strictly load and validate a workspace document: serde rejects unknown
/// fields and mistyped values (`deny_unknown_fields` plus typed fields, the
/// equivalent of the kernel's exact key sets and bool/int separation), then
/// [`WorkspaceLayout::validate`] enforces the contract.
///
/// # Errors
///
/// Returns [`WorkspaceLoadError::Decode`] for undecodable bytes and
/// [`WorkspaceLoadError::Invalid`] for contract violations.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::workspace_layout::{load_validated, WorkspaceLoadError};
///
/// let refused = load_validated(b"{\"nope\":true}");
/// assert!(matches!(refused, Err(WorkspaceLoadError::Decode(_))));
/// ```
pub fn load_validated(bytes: &[u8]) -> Result<WorkspaceLayout, WorkspaceLoadError> {
    let layout: WorkspaceLayout = lgwks_std::json::from_slice(bytes)
        .map_err(|error| WorkspaceLoadError::Decode(format!("{error}")))?;
    layout.validate().map_err(WorkspaceLoadError::Invalid)?;
    Ok(layout)
}
