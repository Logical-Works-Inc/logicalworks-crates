//! The rows a caller writes and reads: run starts, policies, closes, run and lane rows, and open runs.

/// What a run records before its first lane.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RunStart<'a> {
    /// The run id, unique across tenants.
    pub run_id: &'a str,
    /// The tenant that owns the run.
    pub tenant: &'a str,
    /// The repository under test.
    pub repo: &'a str,
    /// The host the coordinator runs on.
    pub host: &'a str,
    /// The coordinator's process id.
    pub coordinator_pid: u32,
    /// The coordinator's lock, relative to the state directory.
    pub coordinator_lock: &'a str,
    /// When the run started, as an opaque string the caller mints.
    pub started_at: &'a str,
}

impl<'a> RunStart<'a> {
    /// The run's opening record, borrowing every field from the caller.
    ///
    /// Borrowed rather than owned because the record is framed and written
    /// inside [`TenantStore::begin_run`](super::TenantStore::begin_run) and never
    /// outlives the call.
    #[must_use]
    pub fn new(
        run_id: &'a str,
        tenant: &'a str,
        repo: &'a str,
        host: &'a str,
        coordinator_pid: u32,
        coordinator_lock: &'a str,
        started_at: &'a str,
    ) -> Self {
        Self {
            run_id,
            tenant,
            repo,
            host,
            coordinator_pid,
            coordinator_lock,
            started_at,
        }
    }
}

/// A tenant's immutable policy: the gates as stored, the hardness level, and
/// when it was recorded.
///
/// The gates are opaque here — the string `logical_ci` wrote — because
/// decoding them against that crate's family ladder is its job, and this store
/// stays decoupled from it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Policy {
    /// The gate set, exactly as recorded. Private because a policy is
    /// written once and never rewritten; readers use [`gates_json`](Self::gates_json).
    gates_json: String,
    /// The hardness level, exactly as recorded. Private; readers use [`hardness`](Self::hardness).
    hardness: u8,
    /// When the policy was recorded. Private for the same reason as the gates.
    created_at: String,
}

impl Policy {
    /// A policy exactly as the store holds it.
    #[must_use]
    pub fn new(gates_json: String, hardness: u8, created_at: String) -> Self {
        Self {
            gates_json,
            hardness,
            created_at,
        }
    }

    /// The gate set, exactly as recorded.
    #[must_use]
    pub fn gates_json(&self) -> &str {
        &self.gates_json
    }

    /// The hardness level, exactly as recorded.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::tenant_store::Policy;
    ///
    /// let policy = Policy::new("[]".to_owned(), 3, "t0".to_owned());
    /// assert_eq!(policy.hardness(), 3);
    /// ```
    #[must_use]
    pub const fn hardness(&self) -> u8 {
        self.hardness
    }

    /// When the policy was recorded.
    #[must_use]
    pub fn created_at(&self) -> &str {
        &self.created_at
    }
}

/// How a closed run ended: when, with what verdict, and what it sealed.
///
/// Grouped rather than threaded as five arguments through every row
/// constructor, because the five travel together from every closer and a
/// second order of them would be a second definition of one fact.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunClose {
    /// When the run finished. Private; readers use [`finished_at`](Self::finished_at).
    finished_at: String,
    /// The verdict the run closed with. Private; readers use [`verdict`](Self::verdict).
    verdict: String,
    /// Why the run closed the way it did, if it named one. Private; readers use [`reason`](Self::reason).
    reason: Option<String>,
    /// The sealed record, if the run sealed one. Private; readers use [`record`](Self::record).
    record: Option<String>,
    /// The seal, if the run sealed one. Private; readers use [`seal`](Self::seal).
    seal: Option<String>,
}

impl RunClose {
    /// How a run closed, exactly as the store holds it.
    #[must_use]
    pub fn new(
        finished_at: String,
        verdict: String,
        reason: Option<String>,
        record: Option<String>,
        seal: Option<String>,
    ) -> Self {
        Self {
            finished_at,
            verdict,
            reason,
            record,
            seal,
        }
    }

    /// When the run finished.
    #[must_use]
    pub fn finished_at(&self) -> &str {
        &self.finished_at
    }

    /// The verdict the run closed with.
    #[must_use]
    pub fn verdict(&self) -> &str {
        &self.verdict
    }

    /// Why the run closed the way it did, if it named one.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::tenant_store::RunClose;
    ///
    /// let close = RunClose::new("t1".to_owned(), "fail".to_owned(), Some("lane 2".to_owned()), None, None);
    /// assert_eq!(close.reason(), Some("lane 2"));
    /// ```
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// The sealed record, if the run sealed one.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::tenant_store::RunClose;
    ///
    /// let close = RunClose::new("t1".to_owned(), "pass".to_owned(), None, Some("rec".to_owned()), None);
    /// assert_eq!(close.record(), Some("rec"));
    /// ```
    #[must_use]
    pub fn record(&self) -> Option<&str> {
        self.record.as_deref()
    }

    /// The seal, if the run sealed one.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::tenant_store::RunClose;
    ///
    /// let abandoned = RunClose::new("t1".to_owned(), "abandoned".to_owned(), None, None, None);
    /// assert_eq!(abandoned.seal(), None, "an abandonment seals nothing");
    /// ```
    #[must_use]
    pub fn seal(&self) -> Option<&str> {
        self.seal.as_deref()
    }
}

/// One run as listed or shown.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunRow {
    /// The run id. Private; readers use [`run_id`](Self::run_id).
    run_id: String,
    /// The repository under test. Private; readers use [`repo`](Self::repo).
    repo: String,
    /// When the run started. Private; readers use [`started_at`](Self::started_at).
    started_at: String,
    /// How the run closed, if it did. Private; readers use [`close`](Self::close).
    close: Option<RunClose>,
}

impl RunRow {
    /// One run row exactly as the store holds it.
    #[must_use]
    pub fn new(run_id: String, repo: String, started_at: String, close: Option<RunClose>) -> Self {
        Self {
            run_id,
            repo,
            started_at,
            close,
        }
    }

    /// The identifier the coordinator passed at begin: the row key every lane
    /// and close of this run is folded under, so one run's history is one key.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The repository under test.
    #[must_use]
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// When the run started.
    #[must_use]
    pub fn started_at(&self) -> &str {
        &self.started_at
    }

    /// How the run closed, if it did.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::tenant_store::RunRow;
    ///
    /// let open = RunRow::new("run-1".to_owned(), "repo".to_owned(), "t0".to_owned(), None);
    /// assert!(open.close().is_none(), "an open run has not closed");
    /// ```
    #[must_use]
    pub const fn close(&self) -> Option<&RunClose> {
        self.close.as_ref()
    }

    /// When the run finished, if it did.
    #[must_use]
    pub fn finished_at(&self) -> Option<&str> {
        self.close.as_ref().map(|close| close.finished_at())
    }

    /// The verdict, if the run closed.
    #[must_use]
    pub fn verdict(&self) -> Option<&str> {
        self.close.as_ref().map(|close| close.verdict())
    }

    /// Why the run closed the way it did, if it named one.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.close.as_ref().and_then(RunClose::reason)
    }

    /// The sealed record, if the run sealed one.
    #[must_use]
    pub fn record(&self) -> Option<&str> {
        self.close.as_ref().and_then(RunClose::record)
    }

    /// The closer's sealed record, when the run closed with one: `None` while
    /// the run is open and when the closer sealed nothing (an abandonment).
    #[must_use]
    pub fn seal(&self) -> Option<&str> {
        self.close.as_ref().and_then(RunClose::seal)
    }
}

/// One lane row of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LaneRow {
    /// The lane id from the plan. Private; readers use [`id`](Self::id).
    id: String,
    /// `pending`, `dispatched` or `ended`. Private; readers use [`state`](Self::state).
    state: String,
    /// The outcome, once the lane ended. Private; readers use [`outcome`](Self::outcome).
    outcome: Option<String>,
    /// The detail, once the lane ended. Private; readers use [`detail`](Self::detail).
    detail: Option<String>,
    /// The captured tail, once the lane ended. Private; readers use [`tail`](Self::tail).
    tail: Option<Vec<u8>>,
}

impl LaneRow {
    /// One lane row exactly as the store holds it.
    #[must_use]
    pub fn new(
        id: String,
        state: String,
        outcome: Option<String>,
        detail: Option<String>,
        tail: Option<Vec<u8>>,
    ) -> Self {
        Self {
            id,
            state,
            outcome,
            detail,
            tail,
        }
    }

    /// The lane id from the plan.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// `pending`, `dispatched` or `ended`.
    #[must_use]
    pub fn state(&self) -> &str {
        &self.state
    }

    /// The outcome, once the lane ended.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::tenant_store::LaneRow;
    ///
    /// let lane = LaneRow::new("fmt".to_owned(), "ended".to_owned(), Some("pass".to_owned()), None, None);
    /// assert_eq!(lane.outcome(), Some("pass"));
    /// ```
    #[must_use]
    pub fn outcome(&self) -> Option<&str> {
        self.outcome.as_deref()
    }

    /// The detail, once the lane ended.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::tenant_store::LaneRow;
    ///
    /// let pending = LaneRow::new("fmt".to_owned(), "pending".to_owned(), None, None, None);
    /// assert_eq!(pending.detail(), None, "a pending lane has no detail yet");
    /// ```
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// The captured tail, once the lane ended.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::tenant_store::LaneRow;
    ///
    /// let lane = LaneRow::new("test".to_owned(), "ended".to_owned(), None, None, Some(b"ok\n".to_vec()));
    /// assert_eq!(lane.tail(), Some(&b"ok\n"[..]));
    /// ```
    #[must_use]
    pub fn tail(&self) -> Option<&[u8]> {
        self.tail.as_deref()
    }
}

/// A run whose coordinator may have died.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenRun {
    /// The run id. Private; readers use [`run_id`](Self::run_id).
    run_id: String,
    /// The coordinator's process id, for the liveness check. Private; readers use [`coordinator_pid`](Self::coordinator_pid).
    coordinator_pid: u32,
}

impl OpenRun {
    /// An open run exactly as the store holds it.
    #[must_use]
    pub fn new(run_id: String, coordinator_pid: u32) -> Self {
        Self {
            run_id,
            coordinator_pid,
        }
    }

    /// The run this claim is fenced to: every lane step checks the authority's
    /// epoch for this id before it commits, and a stale one is refused.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The coordinator's process id, for the liveness check.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::tenant_store::OpenRun;
    ///
    /// let open = OpenRun::new("run-1".to_owned(), 4242);
    /// assert_eq!(open.coordinator_pid(), 4242);
    /// ```
    #[must_use]
    pub const fn coordinator_pid(&self) -> u32 {
        self.coordinator_pid
    }
}
