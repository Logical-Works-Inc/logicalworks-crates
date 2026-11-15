//! Widget registration descriptor and its render surface.

use lgwks_std::json::serde::{Deserialize, Serialize};

use super::SignalPattern;
use crate::canvas_protocol::Capability;

/// Surface a widget can render on.
/// # Examples
/// ```
/// use drishti::native_host::protocol::WidgetSurface;
/// assert_ne!(WidgetSurface::Web, WidgetSurface::Native);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum WidgetSurface {
    /// Web renderer.
    Web,
    /// Native host.
    Native,
    /// Both renderers.
    Both,
}

/// Widget registration descriptor.
///
/// Fields are private: construction runs through [`WidgetDescriptor::new`]
/// (which strips `intent.emit` as defense-in-depth) and reads go through the
/// accessors.
/// # Examples
/// ```
/// use drishti::canvas_protocol::Capability;
/// use drishti::native_host::protocol::{WidgetDescriptor, WidgetSurface};
/// let d = WidgetDescriptor::new("k".into(), "w".into(), "1".into(), WidgetSurface::Web, vec![], vec![], vec![Capability::IntentEmit]);
/// assert!(d.capabilities().is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub struct WidgetDescriptor {
    /// Qualified widget id.
    widget_id: String,
    /// Widget kind.
    kind: String,
    /// Widget version.
    version: String,
    /// Render surface.
    surface: WidgetSurface,
    /// Emitted signal patterns.
    emits: Vec<SignalPattern>,
    /// Listened signal patterns.
    listens: Vec<SignalPattern>,
    /// Granted capabilities (`intent.emit` is stripped here as
    /// defense-in-depth: widgets never hold it).
    capabilities: Vec<Capability>,
    /// Whether the widget receives its own emissions.
    self_listen: bool,
    /// Whether the widget is pinned.
    pinned: bool,
}

impl WidgetDescriptor {
    /// Assemble a descriptor, stripping `intent.emit` from the capabilities.
    /// Mount flags default off; [`WidgetDescriptor::set_self_listen`] and
    /// [`WidgetDescriptor::set_pinned`] opt in, keeping this constructor at
    /// the seven-argument clipping bound.
    /// # Examples
    /// ```
    /// use drishti::canvas_protocol::Capability;
    /// use drishti::native_host::protocol::{WidgetDescriptor, WidgetSurface};
    /// let caps = vec![Capability::SignalEmit, Capability::IntentEmit];
    /// let d = WidgetDescriptor::new("k".into(), "w".into(), "1".into(), WidgetSurface::Web, vec![], vec![], caps);
    /// assert_eq!(d.capabilities(), [Capability::SignalEmit]);
    /// assert!(!d.self_listen() && !d.pinned());
    /// ```
    #[must_use]
    pub fn new(
        kind: String,
        widget_id: String,
        version: String,
        surface: WidgetSurface,
        emits: Vec<SignalPattern>,
        listens: Vec<SignalPattern>,
        capabilities: Vec<Capability>,
    ) -> Self {
        let capabilities = capabilities
            .into_iter()
            .filter(|capability| *capability != Capability::IntentEmit)
            .collect();
        Self {
            kind,
            widget_id,
            version,
            surface,
            emits,
            listens,
            capabilities,
            self_listen: false,
            pinned: false,
        }
    }

    /// Receive our own emissions (the broker skips self-delivery otherwise).
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{WidgetDescriptor, WidgetSurface};
    /// let mut d = WidgetDescriptor::new("k".into(), "w".into(), "1".into(), WidgetSurface::Web, vec![], vec![], vec![]);
    /// d.set_self_listen(true);
    /// assert!(d.self_listen());
    /// ```
    pub fn set_self_listen(&mut self, self_listen: bool) {
        self.self_listen = self_listen;
    }

    /// Pin the widget in place.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{WidgetDescriptor, WidgetSurface};
    /// let mut d = WidgetDescriptor::new("k".into(), "w".into(), "1".into(), WidgetSurface::Web, vec![], vec![], vec![]);
    /// d.set_pinned(true);
    /// assert!(d.pinned());
    /// ```
    pub fn set_pinned(&mut self, pinned: bool) {
        self.pinned = pinned;
    }

    /// Qualified widget id the broker registers this descriptor under.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{WidgetDescriptor, WidgetSurface};
    /// let d = WidgetDescriptor::new("k".into(), "widget.a".into(), "1".into(), WidgetSurface::Web, vec![], vec![], vec![]);
    /// assert_eq!(d.widget_id(), "widget.a");
    /// ```
    #[must_use]
    pub fn widget_id(&self) -> &str {
        &self.widget_id
    }

    /// Widget kind, the category the descriptor was declared under.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{WidgetDescriptor, WidgetSurface};
    /// let d = WidgetDescriptor::new("forms".into(), "w".into(), "1".into(), WidgetSurface::Web, vec![], vec![], vec![]);
    /// assert_eq!(d.kind(), "forms");
    /// ```
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Declared widget version string, carried verbatim.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{WidgetDescriptor, WidgetSurface};
    /// let d = WidgetDescriptor::new("k".into(), "w".into(), "1.2.3".into(), WidgetSurface::Web, vec![], vec![], vec![]);
    /// assert_eq!(d.version(), "1.2.3");
    /// ```
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Renderer the widget targets (web, native, or both).
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{WidgetDescriptor, WidgetSurface};
    /// let d = WidgetDescriptor::new("k".into(), "w".into(), "1".into(), WidgetSurface::Both, vec![], vec![], vec![]);
    /// assert_eq!(d.surface(), WidgetSurface::Both);
    /// ```
    #[must_use]
    pub const fn surface(&self) -> WidgetSurface {
        self.surface
    }

    /// Emitted signal patterns.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{SignalPattern, WidgetDescriptor, WidgetSurface};
    /// let emits = vec![SignalPattern::new("a.**".into(), None)];
    /// let d = WidgetDescriptor::new("k".into(), "w".into(), "1".into(), WidgetSurface::Web, emits, vec![], vec![]);
    /// assert_eq!(d.emits().len(), 1);
    /// ```
    #[must_use]
    pub fn emits(&self) -> &[SignalPattern] {
        &self.emits
    }

    /// Listened signal patterns.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{SignalPattern, WidgetDescriptor, WidgetSurface};
    /// let listens = vec![SignalPattern::new("a.**".into(), None)];
    /// let d = WidgetDescriptor::new("k".into(), "w".into(), "1".into(), WidgetSurface::Web, vec![], listens, vec![]);
    /// assert_eq!(d.listens()[0].kind_glob(), "a.**");
    /// ```
    #[must_use]
    pub fn listens(&self) -> &[SignalPattern] {
        &self.listens
    }

    /// Capabilities the widget holds, with `intent.emit` already stripped.
    /// # Examples
    /// ```
    /// # use drishti::canvas_protocol::Capability;
    /// # use drishti::native_host::protocol::{WidgetDescriptor, WidgetSurface};
    /// let d = WidgetDescriptor::new("k".into(), "w".into(), "1".into(), WidgetSurface::Web, vec![], vec![], vec![Capability::SignalEmit]);
    /// assert_eq!(d.capabilities(), [Capability::SignalEmit]);
    /// ```
    #[must_use]
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// Whether the widget receives its own emissions.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{WidgetDescriptor, WidgetSurface};
    /// let d = WidgetDescriptor::new("k".into(), "w".into(), "1".into(), WidgetSurface::Web, vec![], vec![], vec![]);
    /// assert!(!d.self_listen());
    /// ```
    #[must_use]
    pub const fn self_listen(&self) -> bool {
        self.self_listen
    }

    /// Whether the widget is pinned.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{WidgetDescriptor, WidgetSurface};
    /// let d = WidgetDescriptor::new("k".into(), "w".into(), "1".into(), WidgetSurface::Web, vec![], vec![], vec![]);
    /// assert!(!d.pinned());
    /// ```
    #[must_use]
    pub const fn pinned(&self) -> bool {
        self.pinned
    }
}
