//! Governance log entries and their source attribution.

use lgwks_std::json::serde::{Deserialize, Serialize};

/// Attribution source kind for governance entries.
/// # Examples
/// ```
/// use drishti::native_host::protocol::AttributionKind;
/// assert_ne!(AttributionKind::User, AttributionKind::System);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum AttributionKind {
    /// A person.
    User,
    /// A widget.
    Widget,
    /// An ML peer.
    Ml,
    /// The system.
    System,
}

/// Source attribution on a governance entry.
///
/// Fields are private: construction runs through [`Attribution::new`] and
/// reads go through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::protocol::{Attribution, AttributionKind};
/// let a = Attribution::new(AttributionKind::Ml, "peer".into(), Some("m1".into()), Some(0.5));
/// assert_eq!(a.kind(), AttributionKind::Ml);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub struct Attribution {
    /// Source kind.
    kind: AttributionKind,
    /// Source identity.
    source_id: String,
    /// Model version, for ML attributions.
    model_version: Option<String>,
    /// Confidence, for ML attributions.
    confidence: Option<f64>,
}

impl Attribution {
    /// Assemble attribution with optional model detail.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::{Attribution, AttributionKind};
    /// let a = Attribution::new(AttributionKind::Widget, "w".into(), None, None);
    /// assert_eq!((a.source_id(), a.model_version(), a.confidence()), ("w", None, None));
    /// ```
    #[must_use]
    pub fn new(
        kind: AttributionKind,
        source_id: String,
        model_version: Option<String>,
        confidence: Option<f64>,
    ) -> Self {
        Self {
            kind,
            source_id,
            model_version,
            confidence,
        }
    }

    /// Source kind.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind};
    /// let a = Attribution::new(AttributionKind::User, "u".into(), None, None);
    /// assert_eq!(a.kind(), AttributionKind::User);
    /// ```
    #[must_use]
    pub const fn kind(&self) -> AttributionKind {
        self.kind
    }

    /// Source identity.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind};
    /// let a = Attribution::new(AttributionKind::User, "u".into(), None, None);
    /// assert_eq!(a.source_id(), "u");
    /// ```
    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Model version, for ML attributions.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind};
    /// let a = Attribution::new(AttributionKind::Ml, "p".into(), Some("m1".into()), None);
    /// assert_eq!(a.model_version().map(String::as_str), Some("m1"));
    /// ```
    #[must_use]
    pub const fn model_version(&self) -> Option<&String> {
        self.model_version.as_ref()
    }

    /// Confidence, for ML attributions.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind};
    /// let a = Attribution::new(AttributionKind::Ml, "p".into(), None, Some(0.25));
    /// assert_eq!(a.confidence(), Some(0.25));
    /// ```
    #[must_use]
    pub const fn confidence(&self) -> Option<f64> {
        self.confidence
    }
}

/// Append-only governance log entry. Attribution is mandatory.
///
/// Fields are private: construction runs through [`GovernanceEntry::new`]
/// and reads go through the accessors. Store admission additionally checks
/// shapes in [`crate::native_host::governance::GovernanceStore::append`].
/// # Examples
/// ```
/// use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
/// let a = Attribution::new(AttributionKind::System, "s".into(), None, None);
/// let e = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig".into(), "k".into(), "d".into(), a);
/// assert_eq!(e.entry_id(), "e1");
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
#[non_exhaustive]
pub struct GovernanceEntry {
    /// Entry identity.
    entry_id: String,
    /// Session the entry belongs to.
    session_id: String,
    /// Work object the entry belongs to.
    work_object_id: String,
    /// Signal the entry records.
    signal_id: String,
    /// Event type (the signal kind).
    event_type: String,
    /// Human description.
    description: String,
    /// Source attribution.
    attribution: Attribution,
    /// Record time as ISO-8601.
    recorded_at: String,
    /// Tenant the entry belongs to. A wire entry without one decodes empty
    /// and the store refuses it, so an unscoped entry is never stored.
    #[serde(default)]
    tenant_id: String,
}

impl GovernanceEntry {
    /// Assemble a governance entry. The record time defaults empty;
    /// [`GovernanceEntry::set_recorded_at`] stamps it, keeping this
    /// constructor at the seven-argument clipping bound. Appends parse the
    /// stamp and refuse empties as malformed, so an unstamped entry can
    /// never be stored.
    /// # Examples
    /// ```
    /// use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// let a = Attribution::new(AttributionKind::System, "s".into(), None, None);
    /// let e = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig".into(), "k".into(), "d".into(), a);
    /// assert_eq!((e.session_id(), e.recorded_at(), e.tenant_id()), ("sess", "", ""));
    /// ```
    #[must_use]
    pub fn new(
        session_id: String,
        entry_id: String,
        work_object_id: String,
        signal_id: String,
        event_type: String,
        description: String,
        attribution: Attribution,
    ) -> Self {
        Self {
            session_id,
            entry_id,
            work_object_id,
            signal_id,
            event_type,
            description,
            attribution,
            recorded_at: String::new(),
            tenant_id: String::new(),
        }
    }

    /// Stamp the record time as ISO-8601.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// # let a = Attribution::new(AttributionKind::System, "s".into(), None, None);
    /// let mut e = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig".into(), "k".into(), "d".into(), a);
    /// e.set_recorded_at("2026-11-03T00:00:00Z".into());
    /// assert_eq!(e.recorded_at(), "2026-11-03T00:00:00Z");
    /// ```
    pub fn set_recorded_at(&mut self, recorded_at: String) {
        self.recorded_at = recorded_at;
    }

    /// Bind the entry to its tenant. Appends refuse an entry with none, and
    /// every governance read names a tenant, so sessions of two tenants that
    /// share a session id never share a tape.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// # let a = Attribution::new(AttributionKind::System, "s".into(), None, None);
    /// let mut e = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig".into(), "k".into(), "d".into(), a);
    /// e.set_tenant_id("tenant-a".into());
    /// assert_eq!(e.tenant_id(), "tenant-a");
    /// ```
    pub fn set_tenant_id(&mut self, tenant_id: String) {
        self.tenant_id = tenant_id;
    }

    /// Tenant the entry belongs to.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// # let a = Attribution::new(AttributionKind::System, "s".into(), None, None);
    /// let e = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig".into(), "k".into(), "d".into(), a);
    /// assert_eq!(e.tenant_id(), "");
    /// ```
    #[must_use]
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// Identity the log keys this entry by; unique within its session.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// # let a = Attribution::new(AttributionKind::System, "s".into(), None, None);
    /// let e = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig".into(), "k".into(), "d".into(), a);
    /// assert_eq!(e.entry_id(), "e1");
    /// ```
    #[must_use]
    pub fn entry_id(&self) -> &str {
        &self.entry_id
    }

    /// Session the entry belongs to.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// # let a = Attribution::new(AttributionKind::System, "s".into(), None, None);
    /// let e = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig".into(), "k".into(), "d".into(), a);
    /// assert_eq!(e.session_id(), "sess");
    /// ```
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Work object the entry's session is scoped to.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// # let a = Attribution::new(AttributionKind::System, "s".into(), None, None);
    /// let e = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig".into(), "k".into(), "d".into(), a);
    /// assert_eq!(e.work_object_id(), "wo");
    /// ```
    #[must_use]
    pub fn work_object_id(&self) -> &str {
        &self.work_object_id
    }

    /// Id of the signal envelope this entry records.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// # let a = Attribution::new(AttributionKind::System, "s".into(), None, None);
    /// let e = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig".into(), "k".into(), "d".into(), a);
    /// assert_eq!(e.signal_id(), "sig");
    /// ```
    #[must_use]
    pub fn signal_id(&self) -> &str {
        &self.signal_id
    }

    /// Signal kind of the recorded envelope; the governance kind filters match it.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// # let a = Attribution::new(AttributionKind::System, "s".into(), None, None);
    /// let e = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig".into(), "forms.submitted".into(), "d".into(), a);
    /// assert_eq!(e.event_type(), "forms.submitted");
    /// ```
    #[must_use]
    pub fn event_type(&self) -> &str {
        &self.event_type
    }

    /// Free-text summary shown beside the entry on the tape.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// # let a = Attribution::new(AttributionKind::System, "s".into(), None, None);
    /// let e = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig".into(), "k".into(), "form sent".into(), a);
    /// assert_eq!(e.description(), "form sent");
    /// ```
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Who produced the recorded signal; mandatory on every entry.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// # let a = Attribution::new(AttributionKind::System, "s".into(), None, None);
    /// let e = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig".into(), "k".into(), "d".into(), a);
    /// assert_eq!(e.attribution().kind(), AttributionKind::System);
    /// ```
    #[must_use]
    pub const fn attribution(&self) -> &Attribution {
        &self.attribution
    }

    /// Record time as ISO-8601.
    /// # Examples
    /// ```
    /// # use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// # let a = Attribution::new(AttributionKind::System, "s".into(), None, None);
    /// let e = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig".into(), "k".into(), "d".into(), a);
    /// assert_eq!(e.recorded_at(), "");
    /// ```
    #[must_use]
    pub fn recorded_at(&self) -> &str {
        &self.recorded_at
    }
}
