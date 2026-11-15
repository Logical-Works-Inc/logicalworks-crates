//! Append-only governance log over SQLite.
//!
//! Absorbed from `surfaces/native-host/Sources/CanvasStores` (kernel#850 row
//! 15): the Swift store was actor-isolated over SwiftData; this port is a
//! synchronous handle over `rusqlite` with the same contract — empty signal
//! ids are malformed, duplicate signal ids are duplicates, queries are
//! bounded to a 24-hour window, and kind globs filter post-fetch. Attributions
//! resolve through [`AttributionSource::resolve`]; unparseable `recorded_at`
//! values surface as malformed entries rather than silent skew. The immutable
//! read model lives in `snapshot`, split for the size law.

use std::sync::{Arc, Mutex, MutexGuard};

use lgwks_deps::rusqlite::{self, Connection, ErrorCode, params};
use lgwks_deps::sqlite::{SqliteError, migrate};
use lgwks_macros::Error;

use super::ensure;
use super::pattern::glob_match;
use super::protocol::{Attribution, AttributionKind, GovernanceEntry};
use super::time::{MAX_WINDOW_MILLIS, parse_millis};

/// Immutable snapshot of a governance entry (split for the size law).
mod snapshot;

pub use snapshot::GovernanceEntrySnapshot;

/// Most rows one [`GovernanceStore::recent`] read returns, whatever limit the
/// caller asks for.
/// # Examples
/// ```
/// use drishti::native_host::governance::MAX_RECENT_ROWS;
/// assert_eq!(MAX_RECENT_ROWS, 10_000);
/// ```
pub const MAX_RECENT_ROWS: usize = 10_000;

/// Largest instant SQLite stores: `i64::MAX`, as an unsigned bound.
const SIGNED_TOP: u64 = u64::MAX.wrapping_shr(1);

/// Schema version this build writes (`PRAGMA user_version`).
const SCHEMA_VERSION: u32 = 1;

/// Governance failures.
/// # Examples
/// ```
/// use drishti::native_host::governance::{AttributionSource, GovernanceError};
/// assert_eq!(
///     AttributionSource::resolve("robot"),
///     Err(GovernanceError::UnknownAttribution("robot".into()))
/// );
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum GovernanceError {
    /// A record with the same signal id already exists.
    #[error("duplicate governance entry: {0}")]
    Duplicate(String),
    /// The query is unbounded: empty session, inverted window, or a window
    /// wider than 24 hours.
    #[error("unbounded governance query")]
    Unbounded,
    /// The entry is malformed, with the reason attached.
    #[error("malformed governance entry: {0}")]
    MalformedEntry(String),
    /// The store itself failed, with the backend message attached.
    #[error("governance store failed: {0}")]
    Store(String),
    /// A stored attribution name is not one this build issues.
    #[error("unknown attribution source: {0}")]
    UnknownAttribution(String),
}

/// Attribution source for query filters.
/// # Examples
/// ```
/// use drishti::native_host::governance::AttributionSource;
/// assert_eq!(AttributionSource::Ml.raw_value(), "ml");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AttributionSource {
    /// A person.
    User,
    /// A widget.
    Widget,
    /// An ML peer.
    Ml,
    /// The system.
    System,
}

impl AttributionSource {
    /// Resolve a stored attribution kind: `ml` maps to ML, `user` and the
    /// legacy `user_intent` map to the user, `widget` maps to the widget and
    /// `system` maps to the system.
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::UnknownAttribution`] for any other name: an
    /// unknown name is never credited to the system.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::AttributionSource;
    /// assert_eq!(AttributionSource::resolve("user_intent"), Ok(AttributionSource::User));
    /// assert!(AttributionSource::resolve("robot").is_err());
    /// ```
    pub fn resolve(from: &str) -> Result<Self, GovernanceError> {
        match from {
            "ml" => Ok(Self::Ml),
            "user" | "user_intent" => Ok(Self::User),
            "widget" => Ok(Self::Widget),
            "system" => Ok(Self::System),
            other => Err(GovernanceError::UnknownAttribution(String::from(other))),
        }
    }

    /// The lowercase name stored in the log and compared by query filters.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::AttributionSource;
    /// assert_eq!(AttributionSource::Widget.raw_value(), "widget");
    /// ```
    #[must_use]
    pub const fn raw_value(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Widget => "widget",
            Self::Ml => "ml",
            Self::System => "system",
        }
    }
}

/// Bounded query parameters. Construction validates the bound: empty
/// sessions, inverted windows, and windows wider than 24 hours are
/// [`GovernanceError::Unbounded`].
///
/// Fields are private: construction runs through [`GovernanceQuery::new`] and
/// reads go through the accessors.
/// # Examples
/// ```
/// use drishti::native_host::governance::{GovernanceError, GovernanceQuery};
/// // A window wider than 24 hours is refused.
/// let wide = GovernanceQuery::new("t".into(), "s".into(), 0, 86_400_001, None, None);
/// assert_eq!(wide, Err(GovernanceError::Unbounded));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GovernanceQuery {
    /// Tenant to scope the query to.
    tenant_id: String,
    /// Session to scope the query to (within the tenant).
    session_id: String,
    /// Window start in millisecond time.
    from_ms: u64,
    /// Window end in millisecond time.
    to_ms: u64,
    /// Optional attribution filter; none means all sources.
    attribution_filter: Option<AttributionSource>,
    /// Optional kind glob, matched post-fetch; none means all kinds.
    kind_glob: Option<String>,
}

impl GovernanceQuery {
    /// Build a bounded query for one session of one tenant.
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::Unbounded`] for an empty tenant, an empty
    /// session, an inverted window, or a window wider than 24 hours.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::{GovernanceError, GovernanceQuery};
    /// assert!(GovernanceQuery::new("t".into(), "s".into(), 10, 20, None, None).is_ok());
    /// assert_eq!(
    ///     GovernanceQuery::new("t".into(), "s".into(), 20, 10, None, None),
    ///     Err(GovernanceError::Unbounded)
    /// );
    /// ```
    pub fn new(
        tenant_id: String,
        session_id: String,
        from_ms: u64,
        to_ms: u64,
        attribution_filter: Option<AttributionSource>,
        kind_glob: Option<String>,
    ) -> Result<Self, GovernanceError> {
        ensure(!tenant_id.is_empty() && !session_id.is_empty(), || {
            GovernanceError::Unbounded
        })?;
        let window = to_ms
            .checked_sub(from_ms)
            .ok_or(GovernanceError::Unbounded)?;
        ensure(window <= MAX_WINDOW_MILLIS, || GovernanceError::Unbounded)?;
        Ok(Self {
            tenant_id,
            session_id,
            from_ms,
            to_ms,
            attribution_filter,
            kind_glob,
        })
    }

    /// Tenant to scope the query to.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceQuery;
    /// let q = GovernanceQuery::new("tenant-a".into(), "s".into(), 0, 1, None, None)?;
    /// assert_eq!(q.tenant_id(), "tenant-a");
    /// # Ok::<(), drishti::native_host::governance::GovernanceError>(())
    /// ```
    #[must_use]
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// Session to scope the query to.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceQuery;
    /// let q = GovernanceQuery::new("t".into(), "sess".into(), 0, 1, None, None)?;
    /// assert_eq!(q.session_id(), "sess");
    /// # Ok::<(), drishti::native_host::governance::GovernanceError>(())
    /// ```
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Window start in millisecond time.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceQuery;
    /// let q = GovernanceQuery::new("t".into(), "s".into(), 5, 9, None, None)?;
    /// assert_eq!(q.from_ms(), 5);
    /// # Ok::<(), drishti::native_host::governance::GovernanceError>(())
    /// ```
    #[must_use]
    pub const fn from_ms(&self) -> u64 {
        self.from_ms
    }

    /// Window end in millisecond time.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceQuery;
    /// let q = GovernanceQuery::new("t".into(), "s".into(), 5, 9, None, None)?;
    /// assert_eq!(q.to_ms(), 9);
    /// # Ok::<(), drishti::native_host::governance::GovernanceError>(())
    /// ```
    #[must_use]
    pub const fn to_ms(&self) -> u64 {
        self.to_ms
    }

    /// Optional attribution filter; none means all sources.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::{AttributionSource, GovernanceQuery};
    /// let q = GovernanceQuery::new("t".into(), "s".into(), 0, 1, Some(AttributionSource::Ml), None)?;
    /// assert_eq!(q.attribution_filter(), Some(AttributionSource::Ml));
    /// # Ok::<(), drishti::native_host::governance::GovernanceError>(())
    /// ```
    #[must_use]
    pub const fn attribution_filter(&self) -> Option<AttributionSource> {
        self.attribution_filter
    }

    /// Optional kind glob, matched post-fetch; none means all kinds.
    /// # Examples
    /// ```
    /// # use drishti::native_host::governance::GovernanceQuery;
    /// let q = GovernanceQuery::new("t".into(), "s".into(), 0, 1, None, Some("forms.*".into()))?;
    /// assert_eq!(q.kind_glob().map(String::as_str), Some("forms.*"));
    /// # Ok::<(), drishti::native_host::governance::GovernanceError>(())
    /// ```
    #[must_use]
    pub const fn kind_glob(&self) -> Option<&String> {
        self.kind_glob.as_ref()
    }
}

/// Append-only governance log. Clone shares the same SQLite backing: an
/// in-memory database for tests, a file for durable hosts.
/// # Examples
/// ```
/// use drishti::native_host::governance::GovernanceStore;
/// let store = GovernanceStore::open_in_memory()?;
/// assert_eq!(store.count("tenant-a", "sess")?, 0);
/// # Ok::<(), drishti::native_host::governance::GovernanceError>(())
/// ```
#[derive(Debug, Clone)]
pub struct GovernanceStore {
    /// Shared connection. `rusqlite::Connection` is `Send` but not `Sync`;
    /// the mutex makes the handle `Sync`.
    inner: Arc<Mutex<Connection>>,
}

impl GovernanceStore {
    /// Open an in-memory store (tests and ephemeral hosts).
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::Store`] when the schema cannot install.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::GovernanceStore;
    /// let store = GovernanceStore::open_in_memory()?;
    /// assert_eq!(store.count("t", "s")?, 0);
    /// # Ok::<(), drishti::native_host::governance::GovernanceError>(())
    /// ```
    pub fn open_in_memory() -> Result<Self, GovernanceError> {
        let connection = Connection::open_in_memory()
            .map_err(|error| GovernanceError::Store(format!("{error}")))?;
        Self::with_connection(connection)
    }

    /// Open a file-backed store (durable hosts).
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::Store`] when the file cannot open or the
    /// schema cannot install.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::GovernanceStore;
    /// // A path under a missing directory cannot open.
    /// let missing = std::env::temp_dir().join("drishti-no-such-dir").join("log.db");
    /// assert!(GovernanceStore::open(&missing).is_err());
    /// ```
    pub fn open(path: &std::path::Path) -> Result<Self, GovernanceError> {
        let connection =
            Connection::open(path).map_err(|error| GovernanceError::Store(format!("{error}")))?;
        Self::with_connection(connection)
    }

    /// Wrap a connection and install the schema.
    fn with_connection(connection: Connection) -> Result<Self, GovernanceError> {
        let store = Self {
            inner: Arc::new(Mutex::new(connection)),
        };
        store.install_schema()?;
        Ok(store)
    }

    /// Lock the connection, recovering from poisoning by keeping the
    /// connection (a poisoned mutex still yields its data).
    fn lock(&self) -> MutexGuard<'_, Connection> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Bring the file to [`SCHEMA_VERSION`] through the estate's one migration
    /// mechanism, `lgwks_deps::sqlite::migrate` (#696). The step from version
    /// 0 creates the entries table and its tenant/session/time index, first
    /// upgrading a first-release file (signal id alone as the primary key)
    /// to the `(tenant_id, signal_id)` key. Each step and its `user_version`
    /// commit in one immediate transaction: a failure rolls back and leaves
    /// the old file intact, and a file written by a newer build is refused
    /// rather than half read.
    fn install_schema(&self) -> Result<(), GovernanceError> {
        let connection = self.lock();
        migrate(&connection, SCHEMA_VERSION, &MIGRATIONS)
            .map(|_version| ())
            .map_err(migration_error)
    }

    /// Append an entry with an empty policy bundle id.
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::MalformedEntry`] for an empty signal id or
    /// an unparseable `recorded_at`, [`GovernanceError::Duplicate`] for a
    /// repeated signal id, and [`GovernanceError::Store`] for backend
    /// failures.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::{GovernanceError, GovernanceStore};
    /// use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// let store = GovernanceStore::open_in_memory()?;
    /// let by = Attribution::new(AttributionKind::Widget, "w".into(), None, None);
    /// let mut entry = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig-1".into(), "forms.submitted".into(), "d".into(), by);
    /// entry.set_recorded_at("2026-11-03T00:00:00Z".into());
    /// entry.set_tenant_id("tenant-a".into());
    /// store.append(&entry)?;
    /// assert_eq!(store.append(&entry), Err(GovernanceError::Duplicate("sig-1".into())));
    /// assert_eq!(store.count("tenant-a", "sess")?, 1);
    /// # Ok::<(), GovernanceError>(())
    /// ```
    pub fn append(&self, entry: &GovernanceEntry) -> Result<(), GovernanceError> {
        self.append_with_policy(entry, String::new())
    }

    /// Append an entry carrying the session's policy bundle id.
    ///
    /// # Errors
    ///
    /// Same as [`GovernanceStore::append`].
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::{GovernanceError, GovernanceQuery, GovernanceStore};
    /// use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};
    /// use drishti::native_host::time::parse_millis;
    /// let store = GovernanceStore::open_in_memory()?;
    /// let by = Attribution::new(AttributionKind::Widget, "w".into(), None, None);
    /// let mut entry = GovernanceEntry::new("sess".into(), "e1".into(), "wo".into(), "sig-1".into(), "k".into(), "d".into(), by);
    /// entry.set_recorded_at("2026-11-03T00:00:00Z".into());
    /// entry.set_tenant_id("tenant-a".into());
    /// store.append_with_policy(&entry, "policy-1".into())?;
    /// let at = parse_millis("2026-11-03T00:00:00Z").map_err(|e| GovernanceError::Store(e.to_string()))?;
    /// let query = GovernanceQuery::new("tenant-a".into(), "sess".into(), at, at, None, None)?;
    /// assert_eq!(store.query(&query)?[0].policy_bundle_id(), "policy-1");
    /// # Ok::<(), GovernanceError>(())
    /// ```
    pub fn append_with_policy(
        &self,
        entry: &GovernanceEntry,
        policy_bundle_id: String,
    ) -> Result<(), GovernanceError> {
        ensure(!entry.signal_id().is_empty(), || {
            GovernanceError::MalformedEntry(String::from("signalId must not be empty"))
        })?;
        ensure(!entry.tenant_id().is_empty(), || {
            GovernanceError::MalformedEntry(String::from("tenantId must not be empty"))
        })?;
        let timestamp_ms = parse_millis(entry.recorded_at()).map_err(|error| {
            GovernanceError::MalformedEntry(format!(
                "unparseable recordedAt {}: {error}",
                entry.recorded_at()
            ))
        })?;
        let attribution = AttributionSource::resolve(attribution_kind_raw(entry.attribution()))?;
        let timestamp = i64::try_from(timestamp_ms).map_err(|error| {
            GovernanceError::MalformedEntry(format!(
                "recordedAt out of range {}: {error}",
                entry.recorded_at()
            ))
        })?;
        let result = self.lock().execute(
            "INSERT INTO governance_entries
                (signal_id, session_id, source_kind, kind, attribution,
                 timestamp_ms, recorded_at, policy_bundle_id, work_object_id, tenant_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                entry.signal_id(),
                entry.session_id(),
                attribution_kind_raw(entry.attribution()),
                entry.event_type(),
                attribution.raw_value(),
                timestamp,
                entry.recorded_at(),
                policy_bundle_id,
                entry.work_object_id(),
                entry.tenant_id(),
            ],
        );
        match result {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == ErrorCode::ConstraintViolation =>
            {
                Err(GovernanceError::Duplicate(entry.signal_id().to_owned()))
            }
            Err(error) => Err(GovernanceError::Store(format!("{error}"))),
        }
    }

    /// Query snapshots for a bounded query, oldest first. The kind glob, if
    /// any, filters post-fetch.
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::Store`] for backend failures.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::{GovernanceError, GovernanceQuery, GovernanceStore};
    /// let store = GovernanceStore::open_in_memory()?;
    /// let query = GovernanceQuery::new("tenant-a".into(), "sess".into(), 0, 1_000, None, None)?;
    /// assert!(store.query(&query)?.is_empty());
    /// # Ok::<(), GovernanceError>(())
    /// ```
    pub fn query(
        &self,
        query: &GovernanceQuery,
    ) -> Result<Vec<GovernanceEntrySnapshot>, GovernanceError> {
        // SQLite reads a negative `LIMIT` as no limit; the 24-hour window
        // bounds the rows.
        let rows = self.fetch(query, false, -1)?;
        match query.kind_glob.as_ref() {
            None => Ok(rows),
            Some(glob) => Ok(rows
                .into_iter()
                .filter(|snapshot| glob_match(glob, snapshot.kind()))
                .collect()),
        }
    }

    /// The newest `limit` snapshots (at most [`MAX_RECENT_ROWS`]) of a
    /// bounded query, oldest first. The
    /// limit runs inside SQL (newest first, then reversed), so the store
    /// never builds the rest of the window. A kind glob filters after the
    /// fetch, so a query with one reads its window and keeps the newest
    /// matches instead.
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::Store`] for backend failures.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::{GovernanceError, GovernanceQuery, GovernanceStore};
    /// let store = GovernanceStore::open_in_memory()?;
    /// let query = GovernanceQuery::new("tenant-a".into(), "sess".into(), 0, 1_000, None, None)?;
    /// assert!(store.recent(&query, 5)?.is_empty());
    /// # Ok::<(), GovernanceError>(())
    /// ```
    pub fn recent(
        &self,
        query: &GovernanceQuery,
        limit: usize,
    ) -> Result<Vec<GovernanceEntrySnapshot>, GovernanceError> {
        let limit = limit.min(MAX_RECENT_ROWS);
        if query.kind_glob.is_some() {
            let matched = self.query(query)?;
            let skip = matched.len().saturating_sub(limit);
            return Ok(matched.into_iter().skip(skip).collect());
        }
        let bound =
            i64::try_from(limit).map_err(|error| GovernanceError::Store(format!("{error}")))?;
        let mut newest = self.fetch(query, true, bound)?;
        newest.reverse();
        Ok(newest)
    }

    /// Run the tenant-scoped select of `query` in one direction, up to
    /// `limit` rows.
    fn fetch(
        &self,
        query: &GovernanceQuery,
        newest_first: bool,
        limit: i64,
    ) -> Result<Vec<GovernanceEntrySnapshot>, GovernanceError> {
        // Stored times are signed 64-bit and never negative. A window that
        // starts beyond the signed range holds no row; one that ends beyond it
        // is cut at the top of the range. Neither is a store failure.
        let Ok(from) = i64::try_from(query.from_ms) else {
            return Ok(Vec::new());
        };
        let to = i64::try_from(query.to_ms.min(SIGNED_TOP))
            .map_err(|error| GovernanceError::Store(format!("{error}")))?;
        let direction = if newest_first { "DESC" } else { "ASC" };
        let sql = format!(
            "SELECT signal_id, session_id, source_kind, kind, attribution,
                    timestamp_ms, recorded_at, policy_bundle_id, work_object_id
             FROM governance_entries
             WHERE tenant_id = ?1 AND session_id = ?2
               AND timestamp_ms >= ?3 AND timestamp_ms <= ?4
               AND (?5 IS NULL OR attribution = ?5)
             ORDER BY timestamp_ms {direction}, rowid {direction}
             LIMIT ?6"
        );
        let filter = query.attribution_filter.map(AttributionSource::raw_value);
        self.select(
            &sql,
            params![query.tenant_id, query.session_id, from, to, filter, limit],
        )
    }

    /// Run a select and map rows to snapshots.
    fn select(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<GovernanceEntrySnapshot>, GovernanceError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare(sql)
            .map_err(|error| GovernanceError::Store(format!("{error}")))?;
        let rows = statement
            .query_map(params, |row| {
                let timestamp: i64 = row.get(5)?;
                let timestamp_ms = u64::try_from(timestamp).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?;
                let mut snapshot = GovernanceEntrySnapshot::new(
                    row.get(1)?,
                    row.get(0)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    timestamp_ms,
                    row.get(6)?,
                );
                snapshot.set_policy_bundle_id(row.get(7)?);
                snapshot.set_work_object_id(row.get(8)?);
                Ok(snapshot)
            })
            .map_err(|error| GovernanceError::Store(format!("{error}")))?;
        let mut snapshots = Vec::new();
        for row in rows {
            snapshots.push(row.map_err(|error| GovernanceError::Store(format!("{error}")))?);
        }
        Ok(snapshots)
    }

    /// Count entries for a session of one tenant. Another tenant's entries
    /// under the same session id are never counted.
    ///
    /// # Errors
    ///
    /// Returns [`GovernanceError::Store`] for backend failures.
    /// # Examples
    /// ```
    /// use drishti::native_host::governance::GovernanceStore;
    /// let store = GovernanceStore::open_in_memory()?;
    /// assert_eq!(store.count("tenant-b", "sess")?, 0);
    /// # Ok::<(), drishti::native_host::governance::GovernanceError>(())
    /// ```
    pub fn count(&self, tenant_id: &str, session_id: &str) -> Result<u64, GovernanceError> {
        let count: i64 = self
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM governance_entries
                 WHERE tenant_id = ?1 AND session_id = ?2",
                params![tenant_id, session_id],
                |row| row.get(0),
            )
            .map_err(|error| GovernanceError::Store(format!("{error}")))?;
        u64::try_from(count).map_err(|error| GovernanceError::Store(format!("{error}")))
    }
}

/// The one migration, from version 0 to [`SCHEMA_VERSION`].
///
/// A statement batch cannot branch, so the step makes the old shape true first:
/// `CREATE TABLE IF NOT EXISTS` leaves a first-release table (signal id alone
/// as the primary key) as it is and gives a fresh file an empty one. Either
/// way the table is then renamed, the `(tenant_id, signal_id)` table and its
/// tenant/session/time index are created, rows are copied across in insertion
/// order and the old table is dropped.
const MIGRATION_V1: &str = "DROP INDEX IF EXISTS idx_governance_scope_time;
    CREATE TABLE IF NOT EXISTS governance_entries (
        signal_id TEXT NOT NULL PRIMARY KEY,
        tenant_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        source_kind TEXT NOT NULL,
        kind TEXT NOT NULL,
        attribution TEXT NOT NULL,
        timestamp_ms INTEGER NOT NULL,
        recorded_at TEXT NOT NULL,
        policy_bundle_id TEXT NOT NULL,
        work_object_id TEXT NOT NULL
    );
    ALTER TABLE governance_entries RENAME TO governance_entries_legacy;
    CREATE TABLE governance_entries (
        signal_id TEXT NOT NULL,
        tenant_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        source_kind TEXT NOT NULL,
        kind TEXT NOT NULL,
        attribution TEXT NOT NULL,
        timestamp_ms INTEGER NOT NULL,
        recorded_at TEXT NOT NULL,
        policy_bundle_id TEXT NOT NULL,
        work_object_id TEXT NOT NULL,
        PRIMARY KEY (tenant_id, signal_id)
    );
    CREATE INDEX idx_governance_scope_time
        ON governance_entries (tenant_id, session_id, timestamp_ms);
    INSERT INTO governance_entries
        (signal_id, tenant_id, session_id, source_kind, kind, attribution,
         timestamp_ms, recorded_at, policy_bundle_id, work_object_id)
    SELECT signal_id, tenant_id, session_id, source_kind, kind, attribution,
           timestamp_ms, recorded_at, policy_bundle_id, work_object_id
    FROM governance_entries_legacy ORDER BY rowid;
    DROP TABLE governance_entries_legacy;";

/// `migrations[i]` moves the file from version `i` to `i + 1`.
const MIGRATIONS: [&str; 1] = [MIGRATION_V1];

/// Render a migration failure as the typed store error, keeping the wording
/// a file from a newer build has always been refused with.
fn migration_error(error: SqliteError) -> GovernanceError {
    match error {
        SqliteError::Downgrade { found, wanted } => GovernanceError::Store(format!(
            "governance schema version {found} is newer than this build's {wanted}"
        )),
        other => GovernanceError::Store(format!("{other}")),
    }
}

/// Raw attribution kind string for storage.
fn attribution_kind_raw(attribution: &Attribution) -> &'static str {
    match attribution.kind() {
        AttributionKind::User => "user",
        AttributionKind::Widget => "widget",
        AttributionKind::Ml => "ml",
        AttributionKind::System => "system",
    }
}
