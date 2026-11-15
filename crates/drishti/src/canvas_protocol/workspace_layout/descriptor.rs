//! Widget identity and signal contract of one workspace surface.

use lgwks_std::json::serde::{Deserialize, Serialize};

use crate::canvas_protocol::{Capability, WidgetSurface};

/// Widget identity and its signal contract.
///
/// Fields are private: construction runs through
/// [`WorkspaceWidgetDescriptor::new`] and reads go through the accessors;
/// mutation runs through the explicit setter and `capabilities_mut`.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::{Capability, WidgetSurface};
/// use drishti::canvas_protocol::workspace_layout::{WorkspaceSignalPattern, WorkspaceWidgetDescriptor};
///
/// let widget = WorkspaceWidgetDescriptor::new(
///     "demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Web,
///     vec![WorkspaceSignalPattern::new("demo.**".into())], vec![], vec![Capability::SignalEmit],
/// );
/// assert_eq!((widget.widget_id(), widget.kind()), ("widget.demo", "demo.widget"));
/// assert_eq!(widget.emits().len(), 1);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkspaceWidgetDescriptor {
    /// Qualified widget id, unique within the layout.
    pub(super) widget_id: String,
    /// Widget kind (`^[a-z][a-z0-9]*(?:[.-][a-z0-9]+)*$`).
    pub(super) kind: String,
    /// Widget version as an `N.N.N` digit triplet.
    pub(super) version: String,
    /// Surface the widget can render on.
    pub(super) surface: WidgetSurface,
    /// Signal patterns the widget emits (at most 64).
    pub(super) emits: Vec<WorkspaceSignalPattern>,
    /// Signal patterns the widget listens for (at most 64).
    pub(super) listens: Vec<WorkspaceSignalPattern>,
    /// Presentation-only capabilities; anything else fails closed.
    pub(super) capabilities: Vec<Capability>,
    /// Whether the widget receives its own emissions.
    self_listen: bool,
    /// Whether the widget is pinned in place.
    pinned: bool,
}

impl WorkspaceWidgetDescriptor {
    /// Assemble a widget descriptor from its parts. Mount flags default
    /// off; [`WorkspaceWidgetDescriptor::set_self_listen`] and
    /// [`WorkspaceWidgetDescriptor::set_pinned`] opt in, keeping this
    /// constructor at the seven-argument clipping bound.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::WidgetSurface;
    /// use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    ///
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// assert!(!widget.self_listen() && !widget.pinned());
    /// assert_eq!(widget.surface(), WidgetSurface::Both);
    /// ```
    #[must_use]
    pub fn new(
        kind: String,
        widget_id: String,
        version: String,
        surface: WidgetSurface,
        emits: Vec<WorkspaceSignalPattern>,
        listens: Vec<WorkspaceSignalPattern>,
        capabilities: Vec<Capability>,
    ) -> Self {
        Self {
            widget_id,
            kind,
            capabilities,
            version,
            surface,
            emits,
            listens,
            self_listen: false,
            pinned: false,
        }
    }

    /// Qualified widget id, unique within the layout.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WidgetSurface;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// assert_eq!(widget.widget_id(), "widget.demo");
    /// ```
    #[must_use]
    pub fn widget_id(&self) -> &str {
        &self.widget_id
    }

    /// Replace the widget id (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WidgetSurface;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    /// let mut widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// widget.set_widget_id("widget.other".into());
    /// assert_eq!(widget.widget_id(), "widget.other");
    /// ```
    pub fn set_widget_id(&mut self, widget_id: String) {
        self.widget_id = widget_id;
    }

    /// Widget kind, a dot- or dash-separated lowercase slug such as
    /// `demo.widget`; signal routing keys on it.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WidgetSurface;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// assert_eq!(widget.kind(), "demo.widget");
    /// ```
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Replace the widget kind (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WidgetSurface;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    /// let mut widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// widget.set_kind("demo.other".into());
    /// assert_eq!(widget.kind(), "demo.other");
    /// ```
    pub fn set_kind(&mut self, kind: String) {
        self.kind = kind;
    }

    /// Widget version as an `N.N.N` digit triplet.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WidgetSurface;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.2.3".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// assert_eq!(widget.version(), "1.2.3");
    /// ```
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Replace the widget version (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WidgetSurface;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    /// let mut widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// widget.set_version("2.0.0".into());
    /// assert_eq!(widget.version(), "2.0.0");
    /// ```
    pub fn set_version(&mut self, version: String) {
        self.version = version;
    }

    /// Surface the widget can render on.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WidgetSurface;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Native, vec![], vec![], vec![]);
    /// assert_eq!(widget.surface(), WidgetSurface::Native);
    /// ```
    #[must_use]
    pub const fn surface(&self) -> WidgetSurface {
        self.surface
    }

    /// Signal patterns the widget emits.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WidgetSurface;
    /// # use drishti::canvas_protocol::workspace_layout::{WorkspaceSignalPattern, WorkspaceWidgetDescriptor};
    /// let out = vec![WorkspaceSignalPattern::new("demo.**".into())];
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, out, vec![], vec![]);
    /// assert_eq!(widget.emits().len(), 1);
    /// assert!(widget.listens().is_empty());
    /// ```
    #[must_use]
    pub fn emits(&self) -> &[WorkspaceSignalPattern] {
        &self.emits
    }

    /// Signal patterns the widget listens for.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WidgetSurface;
    /// # use drishti::canvas_protocol::workspace_layout::{WorkspaceSignalPattern, WorkspaceWidgetDescriptor};
    /// let heard = vec![WorkspaceSignalPattern::new("a.*".into()), WorkspaceSignalPattern::new("b.*".into())];
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], heard, vec![]);
    /// assert_eq!(widget.listens().len(), 2);
    /// ```
    #[must_use]
    pub fn listens(&self) -> &[WorkspaceSignalPattern] {
        &self.listens
    }

    /// Presentation-only capabilities: validation admits only signal emit
    /// and subscribe, each at most once.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{Capability, WidgetSurface};
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![Capability::SignalEmit]);
    /// assert_eq!(widget.capabilities(), &[Capability::SignalEmit]);
    /// ```
    #[must_use]
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// Mutable capabilities (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::{Capability, WidgetSurface};
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    /// let mut widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// widget.capabilities_mut().push(Capability::SignalSubscribe);
    /// assert_eq!(widget.capabilities(), &[Capability::SignalSubscribe]);
    /// ```
    pub fn capabilities_mut(&mut self) -> &mut Vec<Capability> {
        &mut self.capabilities
    }

    /// Whether the widget receives its own emissions.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WidgetSurface;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// assert!(!widget.self_listen());
    /// ```
    #[must_use]
    pub const fn self_listen(&self) -> bool {
        self.self_listen
    }

    /// Receive our own emissions (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WidgetSurface;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    /// let mut widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// widget.set_self_listen(true);
    /// assert!(widget.self_listen());
    /// ```
    pub fn set_self_listen(&mut self, self_listen: bool) {
        self.self_listen = self_listen;
    }

    /// Whether the widget is pinned in place.
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WidgetSurface;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    /// let widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// assert!(!widget.pinned());
    /// ```
    #[must_use]
    pub const fn pinned(&self) -> bool {
        self.pinned
    }

    /// Pin the widget in place (falsification and simulation only).
    ///
    /// # Examples
    ///
    /// ```
    /// # use drishti::canvas_protocol::WidgetSurface;
    /// # use drishti::canvas_protocol::workspace_layout::WorkspaceWidgetDescriptor;
    /// let mut widget = WorkspaceWidgetDescriptor::new("demo.widget".into(), "widget.demo".into(), "1.0.0".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// widget.set_pinned(true);
    /// assert!(widget.pinned());
    /// ```
    pub fn set_pinned(&mut self, pinned: bool) {
        self.pinned = pinned;
    }
}

/// A signal glob a widget emits or listens for.
///
/// Fields are private: construction runs through
/// [`WorkspaceSignalPattern::new`] and reads go through the accessor.
///
/// # Examples
///
/// ```
/// use drishti::canvas_protocol::workspace_layout::WorkspaceSignalPattern;
///
/// let pattern = WorkspaceSignalPattern::new("intent.*".into());
/// assert_eq!(pattern.kind_glob(), "intent.*");
/// assert_eq!(pattern.clone(), pattern);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkspaceSignalPattern {
    /// Kind glob, 1..=160 code points, no control characters.
    pub(super) kind_glob: String,
}

impl WorkspaceSignalPattern {
    /// Wrap a kind glob as a signal pattern.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::workspace_layout::WorkspaceSignalPattern;
    ///
    /// let pattern = WorkspaceSignalPattern::new("demo.**".into());
    /// assert_eq!(pattern.kind_glob(), "demo.**");
    /// ```
    #[must_use]
    pub fn new(kind_glob: String) -> Self {
        Self { kind_glob }
    }

    /// The glob over signal kinds, as given to [`WorkspaceSignalPattern::new`];
    /// validation caps it at 160 code points without control characters.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::workspace_layout::WorkspaceSignalPattern;
    ///
    /// assert_eq!(WorkspaceSignalPattern::new("a.*".into()).kind_glob(), "a.*");
    /// ```
    #[must_use]
    pub fn kind_glob(&self) -> &str {
        &self.kind_glob
    }
}
