//! Session state and zone mounting.
//!
//! Absorbed from `surfaces/native-host/Sources/CanvasStores/SessionStore.swift`
//! and `Sources/CanvasHost/ZoneContainer.swift` (kernel#850 row 15). The
//! Swift observation machinery (`@Published`, `@MainActor`) is replaced by
//! plain state: Drishti's own presentation observes it through the host
//! contract rather than SwiftUI.

use crate::canvas_protocol::SessionInvariant;

/// Host zones in a canvas layout.
///
/// # Examples
///
/// ```
/// use drishti::native_host::session::{ZoneContainer, ZoneId};
///
/// // Zones are plain copyable identities; a container reports the one it holds.
/// let zone = ZoneId::Overlay;
/// assert_eq!(ZoneContainer::new(zone).zone_id(), ZoneId::Overlay);
/// assert_ne!(ZoneId::Header, ZoneId::Footer);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ZoneId {
    /// Header zone.
    Header,
    /// Primary zone.
    Primary,
    /// Secondary zone.
    Secondary,
    /// Tertiary zone.
    Tertiary,
    /// Footer zone.
    Footer,
    /// Overlay zone.
    Overlay,
}

/// Active session invariant plus UI-level focus state.
///
/// # Examples
///
/// ```
/// use drishti::native_host::session::SessionStore;
///
/// let mut store = SessionStore::new();
/// store.set_focus(Some(String::from("primary")));
/// assert_eq!(store.focused_zone(), Some("primary"));
/// assert!(store.invariant().is_none());
/// ```
#[derive(Debug, Clone)]
pub struct SessionStore {
    /// Session context set by `start_session`; none until a session begins.
    invariant: Option<SessionInvariant>,
    /// Focused zone id; none when no zone is focused.
    focused_zone: Option<String>,
    /// Active workspace id; none when no workspace is active.
    active_workspace: Option<String>,
    /// Millisecond timestamp the current session began; none until start.
    session_started_ms: Option<u64>,
    /// Human-readable session label; never a raw session id.
    display_name: Option<String>,
    /// Whether the shell exposes governance affordances. Hidden by default.
    governance_visible: bool,
}

impl SessionStore {
    /// Empty store: no invariant, no focus, no workspace, no start time, no
    /// display name, governance hidden.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::SessionStore;
    ///
    /// let store = SessionStore::new();
    /// assert!(store.invariant().is_none());
    /// assert_eq!(store.session_started_ms(), None);
    /// assert!(!store.governance_visible());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            invariant: None,
            focused_zone: None,
            active_workspace: None,
            session_started_ms: None,
            display_name: None,
            governance_visible: false,
        }
    }

    /// Establish a session invariant, stamping the start time and optionally
    /// setting the human-readable label. Overwrites any previous session.
    /// Takes the already-validated invariant (built by
    /// [`SessionInvariant::new`](crate::canvas_protocol::SessionInvariant::new)),
    /// so no failure remains: validation happened at the boundary, keeping
    /// this method at the four-argument clipping bound.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::canvas_protocol::SessionInvariant;
    /// use drishti::cli::drive::{POLICY_BUNDLE, SESSION_ID, USER_SUB, WORK_OBJECT};
    /// use drishti::native_host::session::SessionStore;
    ///
    /// let session = SessionInvariant::new(
    ///     WORK_OBJECT.into(), SESSION_ID.into(), POLICY_BUNDLE.into(), "tenant".into(), USER_SUB.into(),
    /// )?;
    /// let mut store = SessionStore::new();
    /// store.start_session(session.clone(), Some(String::from("Ada")), 1_000);
    /// assert_eq!(store.invariant(), Some(&session));
    /// assert_eq!(store.session_started_ms(), Some(1_000));
    /// assert_eq!(store.display_name(), Some("Ada"));
    /// # Ok::<(), drishti::canvas_protocol::SessionInvariantError>(())
    /// ```
    pub fn start_session(
        &mut self,
        invariant: SessionInvariant,
        display_name: Option<String>,
        now_ms: u64,
    ) {
        // Focus, workspace and the governance toggle belong to one identity:
        // a different session must not inherit the previous one's view.
        if self.invariant.as_ref() != Some(&invariant) {
            self.focused_zone = None;
            self.active_workspace = None;
            self.governance_visible = false;
        }
        self.invariant = Some(invariant);
        self.session_started_ms = Some(now_ms);
        self.display_name = display_name;
    }

    /// Current session invariant, if a session began.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::SessionStore;
    ///
    /// assert!(SessionStore::new().invariant().is_none());
    /// ```
    #[must_use]
    pub fn invariant(&self) -> Option<&SessionInvariant> {
        self.invariant.as_ref()
    }

    /// Id of the zone holding focus; none until `set_focus` names one and
    /// again after a different session starts.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::SessionStore;
    ///
    /// let mut store = SessionStore::new();
    /// assert_eq!(store.focused_zone(), None);
    /// store.set_focus(Some(String::from("header")));
    /// assert_eq!(store.focused_zone(), Some("header"));
    /// ```
    #[must_use]
    pub fn focused_zone(&self) -> Option<&str> {
        self.focused_zone.as_deref()
    }

    /// Id of the workspace the shell shows; none until `set_active_workspace`
    /// names one and again after a different session starts.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::SessionStore;
    ///
    /// let mut store = SessionStore::new();
    /// store.set_active_workspace(Some(String::from("ws-1")));
    /// assert_eq!(store.active_workspace(), Some("ws-1"));
    /// ```
    #[must_use]
    pub fn active_workspace(&self) -> Option<&str> {
        self.active_workspace.as_deref()
    }

    /// Millisecond timestamp the current session began, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::SessionStore;
    ///
    /// assert_eq!(SessionStore::new().session_started_ms(), None);
    /// ```
    #[must_use]
    pub const fn session_started_ms(&self) -> Option<u64> {
        self.session_started_ms
    }

    /// Human-readable session label, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::SessionStore;
    ///
    /// let mut store = SessionStore::new();
    /// store.set_display_name(Some(String::from("Ada")));
    /// assert_eq!(store.display_name(), Some("Ada"));
    /// ```
    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    /// Whether the shell exposes governance affordances.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::SessionStore;
    ///
    /// let mut store = SessionStore::new();
    /// assert!(!store.governance_visible());
    /// store.set_governance_visible(true);
    /// assert!(store.governance_visible());
    /// ```
    #[must_use]
    pub const fn governance_visible(&self) -> bool {
        self.governance_visible
    }

    /// Update the focused zone; none clears focus.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::SessionStore;
    ///
    /// let mut store = SessionStore::new();
    /// store.set_focus(Some(String::from("primary")));
    /// store.set_focus(None);
    /// assert_eq!(store.focused_zone(), None);
    /// ```
    pub fn set_focus(&mut self, zone: Option<String>) {
        self.focused_zone = zone;
    }

    /// Update the active workspace; none clears it.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::SessionStore;
    ///
    /// let mut store = SessionStore::new();
    /// store.set_active_workspace(Some(String::from("ws-1")));
    /// store.set_active_workspace(None);
    /// assert_eq!(store.active_workspace(), None);
    /// ```
    pub fn set_active_workspace(&mut self, workspace_id: Option<String>) {
        self.active_workspace = workspace_id;
    }

    /// Update the human-readable label; none clears it.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::SessionStore;
    ///
    /// let mut store = SessionStore::new();
    /// store.set_display_name(Some(String::from("Ada")));
    /// store.set_display_name(None);
    /// assert_eq!(store.display_name(), None);
    /// ```
    pub fn set_display_name(&mut self, name: Option<String>) {
        self.display_name = name;
    }

    /// Show or hide the governance affordances.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::SessionStore;
    ///
    /// let mut store = SessionStore::new();
    /// store.set_governance_visible(true);
    /// assert!(store.governance_visible());
    /// ```
    pub fn set_governance_visible(&mut self, visible: bool) {
        self.governance_visible = visible;
    }
}

impl Default for SessionStore {
    /// Empty store.
    fn default() -> Self {
        Self::new()
    }
}

/// Mount decision for a zone mount request.
///
/// # Examples
///
/// ```
/// use drishti::native_host::session::{MountDecision, ZoneContainer, ZoneId};
///
/// let mut zone = ZoneContainer::new(ZoneId::Primary);
/// assert_eq!(zone.request_mount(String::from("w1")), MountDecision::Allowed);
/// zone.pin();
/// assert!(matches!(zone.request_mount(String::from("w2")), MountDecision::Denied(_)));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MountDecision {
    /// The widget may mount.
    Allowed,
    /// The widget may not mount.
    Denied(MountDenialReason),
}

/// Why a mount was denied.
///
/// # Examples
///
/// ```
/// use drishti::native_host::session::MountDenialReason;
///
/// let reason = MountDenialReason::zone_pinned(Some(String::from("w1")));
/// assert_eq!(reason.existing(), Some(&String::from("w1")));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MountDenialReason {
    /// The zone is pinned to another widget.
    ZonePinned {
        /// Widget currently holding the zone; none when pinned empty.
        existing: Option<String>,
    },
}

impl MountDenialReason {
    /// Deny because the zone is pinned; `existing` names the holder, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::MountDenialReason;
    ///
    /// assert_eq!(MountDenialReason::zone_pinned(None).existing(), None);
    /// ```
    #[must_use]
    pub fn zone_pinned(existing: Option<String>) -> Self {
        Self::ZonePinned { existing }
    }

    /// Widget currently holding the zone, if pinned.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::MountDenialReason;
    ///
    /// let reason = MountDenialReason::zone_pinned(Some(String::from("w1")));
    /// assert_eq!(reason.existing().map(String::as_str), Some("w1"));
    /// ```
    #[must_use]
    pub fn existing(&self) -> Option<&String> {
        match *self {
            Self::ZonePinned { ref existing } => existing.as_ref(),
        }
    }
}

/// One mountable zone: pin state plus the currently mounted widget.
///
/// # Examples
///
/// ```
/// use drishti::native_host::session::{ZoneContainer, ZoneId};
///
/// let mut zone = ZoneContainer::new(ZoneId::Header);
/// zone.request_mount(String::from("w1"));
/// assert_eq!(zone.current_widget_id(), Some("w1"));
/// ```
#[derive(Debug, Clone)]
pub struct ZoneContainer {
    /// Zone identity.
    zone_id: ZoneId,
    /// Whether the zone is pinned.
    is_pinned: bool,
    /// Currently mounted widget, if any.
    current_widget_id: Option<String>,
}

impl ZoneContainer {
    /// Empty unpinned zone.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::{ZoneContainer, ZoneId};
    ///
    /// let zone = ZoneContainer::new(ZoneId::Footer);
    /// assert!(!zone.is_pinned());
    /// assert_eq!(zone.current_widget_id(), None);
    /// ```
    #[must_use]
    pub const fn new(zone_id: ZoneId) -> Self {
        Self {
            zone_id,
            is_pinned: false,
            current_widget_id: None,
        }
    }

    /// The zone this container was built for; fixed for its lifetime.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::{ZoneContainer, ZoneId};
    ///
    /// assert_eq!(ZoneContainer::new(ZoneId::Tertiary).zone_id(), ZoneId::Tertiary);
    /// ```
    #[must_use]
    pub const fn zone_id(&self) -> ZoneId {
        self.zone_id
    }

    /// Whether the zone is pinned.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::{ZoneContainer, ZoneId};
    ///
    /// let mut zone = ZoneContainer::new(ZoneId::Primary);
    /// zone.pin();
    /// assert!(zone.is_pinned());
    /// ```
    #[must_use]
    pub const fn is_pinned(&self) -> bool {
        self.is_pinned
    }

    /// Id of the widget that last mounted here; none while empty or after
    /// `clear`.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::{ZoneContainer, ZoneId};
    ///
    /// let mut zone = ZoneContainer::new(ZoneId::Primary);
    /// zone.request_mount(String::from("w1"));
    /// assert_eq!(zone.current_widget_id(), Some("w1"));
    /// zone.clear();
    /// assert_eq!(zone.current_widget_id(), None);
    /// ```
    #[must_use]
    pub fn current_widget_id(&self) -> Option<&str> {
        self.current_widget_id.as_deref()
    }

    /// Request a mount. A pinned zone denies with the holding widget, if any;
    /// otherwise the widget mounts (replacing any previous one).
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::{MountDecision, MountDenialReason, ZoneContainer, ZoneId};
    ///
    /// let mut zone = ZoneContainer::new(ZoneId::Primary);
    /// assert_eq!(zone.request_mount(String::from("w1")), MountDecision::Allowed);
    /// zone.pin();
    /// assert_eq!(
    ///     zone.request_mount(String::from("w2")),
    ///     MountDecision::Denied(MountDenialReason::zone_pinned(Some(String::from("w1"))))
    /// );
    /// ```
    pub fn request_mount(&mut self, widget_id: String) -> MountDecision {
        if self.is_pinned {
            return MountDecision::Denied(MountDenialReason::zone_pinned(
                self.current_widget_id.clone(),
            ));
        }
        self.current_widget_id = Some(widget_id);
        MountDecision::Allowed
    }

    /// Lock the zone: later mount requests are denied until `unpin`.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::{ZoneContainer, ZoneId};
    ///
    /// let mut zone = ZoneContainer::new(ZoneId::Primary);
    /// zone.pin();
    /// assert!(zone.is_pinned());
    /// ```
    pub fn pin(&mut self) {
        self.is_pinned = true;
    }

    /// Release a pin so mount requests are accepted again.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::{ZoneContainer, ZoneId};
    ///
    /// let mut zone = ZoneContainer::new(ZoneId::Primary);
    /// zone.pin();
    /// zone.unpin();
    /// assert!(!zone.is_pinned());
    /// ```
    pub fn unpin(&mut self) {
        self.is_pinned = false;
    }

    /// Clear the mounted widget (pin state is kept).
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::native_host::session::{ZoneContainer, ZoneId};
    ///
    /// let mut zone = ZoneContainer::new(ZoneId::Primary);
    /// zone.request_mount(String::from("w1"));
    /// zone.pin();
    /// zone.clear();
    /// assert_eq!(zone.current_widget_id(), None);
    /// assert!(zone.is_pinned());
    /// ```
    pub fn clear(&mut self) {
        self.current_widget_id = None;
    }
}
