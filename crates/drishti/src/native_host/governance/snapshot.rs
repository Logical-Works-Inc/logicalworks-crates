//! Immutable governance read model.

/// Immutable snapshot of a governance entry for cross-handle reads.
///
/// Fields are private: the store builds snapshots and readers go through the
/// accessors.
/// # Examples
/// ```
/// use drishti::native_host::governance::GovernanceEntrySnapshot;
/// let s = GovernanceEntrySnapshot::new("sess".into(), "sig".into(), "widget".into(), "k".into(), "widget".into(), 7, "t".into());
/// assert_eq!((s.signal_id(), s.timestamp_ms()), ("sig", 7));
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct GovernanceEntrySnapshot {
    /// Signal the entry records.
    signal_id: String,
    /// Session the entry belongs to.
    session_id: String,
    /// Raw source kind (`widget`, `ml`, `user`, `system`).
    source_kind: String,
    /// Event type (the signal kind).
    kind: String,
    /// Resolved attribution source.
    attribution: String,
    /// Record time in millisecond time.
    timestamp_ms: u64,
    /// Record time as the original ISO-8601 string.
    recorded_at: String,
    /// Policy bundle the entry was appended with.
    policy_bundle_id: String,
    /// Work object the entry belongs to.
    work_object_id: String,
}

impl GovernanceEntrySnapshot {
    /// Assemble a snapshot. Admission scope (policy bundle and work object)
    /// defaults empty; [`GovernanceEntrySnapshot::set_policy_bundle_id`] and
    /// [`GovernanceEntrySnapshot::set_work_object_id`] attach it, keeping
    /// this constructor at the seven-argument clipping bound.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::GovernanceEntrySnapshot;
    /// let s = GovernanceEntrySnapshot::new("sess".into(), "sig".into(), "ml".into(), "k".into(), "ml".into(), 1, "t".into());
    /// assert_eq!((s.session_id(), s.policy_bundle_id(), s.work_object_id()), ("sess", "", ""));
    /// ```
    #[must_use]
    pub fn new(
        session_id: String,
        signal_id: String,
        source_kind: String,
        kind: String,
        attribution: String,
        timestamp_ms: u64,
        recorded_at: String,
    ) -> Self {
        Self {
            session_id,
            signal_id,
            source_kind,
            kind,
            attribution,
            timestamp_ms,
            recorded_at,
            policy_bundle_id: String::new(),
            work_object_id: String::new(),
        }
    }

    /// Attach the policy bundle the entry was appended with.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceEntrySnapshot;
    /// let mut s = GovernanceEntrySnapshot::new("s".into(), "g".into(), "ml".into(), "k".into(), "ml".into(), 1, "t".into());
    /// s.set_policy_bundle_id("policy-1".into());
    /// assert_eq!(s.policy_bundle_id(), "policy-1");
    /// ```
    pub fn set_policy_bundle_id(&mut self, policy_bundle_id: String) {
        self.policy_bundle_id = policy_bundle_id;
    }

    /// Attach the work object the entry belongs to.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceEntrySnapshot;
    /// let mut s = GovernanceEntrySnapshot::new("s".into(), "g".into(), "ml".into(), "k".into(), "ml".into(), 1, "t".into());
    /// s.set_work_object_id("wo".into());
    /// assert_eq!(s.work_object_id(), "wo");
    /// ```
    pub fn set_work_object_id(&mut self, work_object_id: String) {
        self.work_object_id = work_object_id;
    }

    /// Id of the signal envelope the entry records.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceEntrySnapshot;
    /// let s = GovernanceEntrySnapshot::new("s".into(), "sig-9".into(), "ml".into(), "k".into(), "ml".into(), 1, "t".into());
    /// assert_eq!(s.signal_id(), "sig-9");
    /// ```
    #[must_use]
    pub fn signal_id(&self) -> &str {
        &self.signal_id
    }

    /// Session the entry belongs to.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceEntrySnapshot;
    /// let s = GovernanceEntrySnapshot::new("sess".into(), "g".into(), "ml".into(), "k".into(), "ml".into(), 1, "t".into());
    /// assert_eq!(s.session_id(), "sess");
    /// ```
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Raw source kind (`widget`, `ml`, `user`, `system`).
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceEntrySnapshot;
    /// let s = GovernanceEntrySnapshot::new("s".into(), "g".into(), "widget".into(), "k".into(), "widget".into(), 1, "t".into());
    /// assert_eq!(s.source_kind(), "widget");
    /// ```
    #[must_use]
    pub fn source_kind(&self) -> &str {
        &self.source_kind
    }

    /// Event type (the signal kind).
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceEntrySnapshot;
    /// let s = GovernanceEntrySnapshot::new("s".into(), "g".into(), "ml".into(), "forms.submitted".into(), "ml".into(), 1, "t".into());
    /// assert_eq!(s.kind(), "forms.submitted");
    /// ```
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Resolved attribution source.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceEntrySnapshot;
    /// let s = GovernanceEntrySnapshot::new("s".into(), "g".into(), "ml".into(), "k".into(), "user".into(), 1, "t".into());
    /// assert_eq!(s.attribution(), "user");
    /// ```
    #[must_use]
    pub fn attribution(&self) -> &str {
        &self.attribution
    }

    /// Record time in whole milliseconds since the Unix epoch, as the store
    /// parsed it from `recorded_at`.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceEntrySnapshot;
    /// let s = GovernanceEntrySnapshot::new("s".into(), "g".into(), "ml".into(), "k".into(), "ml".into(), 42, "t".into());
    /// assert_eq!(s.timestamp_ms(), 42);
    /// ```
    #[must_use]
    pub const fn timestamp_ms(&self) -> u64 {
        self.timestamp_ms
    }

    /// Record time as the original ISO-8601 string.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceEntrySnapshot;
    /// let s = GovernanceEntrySnapshot::new("s".into(), "g".into(), "ml".into(), "k".into(), "ml".into(), 1, "2026-11-03T00:00:00Z".into());
    /// assert_eq!(s.recorded_at(), "2026-11-03T00:00:00Z");
    /// ```
    #[must_use]
    pub fn recorded_at(&self) -> &str {
        &self.recorded_at
    }

    /// Id of the policy bundle the entry was appended under; empty when the
    /// append named none.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceEntrySnapshot;
    /// let s = GovernanceEntrySnapshot::new("s".into(), "g".into(), "ml".into(), "k".into(), "ml".into(), 1, "t".into());
    /// assert_eq!(s.policy_bundle_id(), "");
    /// ```
    #[must_use]
    pub fn policy_bundle_id(&self) -> &str {
        &self.policy_bundle_id
    }

    /// Id of the work object the entry's session is scoped to.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceEntrySnapshot;
    /// let s = GovernanceEntrySnapshot::new("s".into(), "g".into(), "ml".into(), "k".into(), "ml".into(), 1, "t".into());
    /// assert_eq!(s.work_object_id(), "");
    /// ```
    #[must_use]
    pub fn work_object_id(&self) -> &str {
        &self.work_object_id
    }
}
