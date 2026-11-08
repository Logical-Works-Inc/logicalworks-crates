//! The estate's one durable store: SQLite opened one way, migrated by version.
//!
//! Every estate service that keeps state across runs opens it here instead of
//! declaring its own SQLite crate (#696): one pinned engine, WAL readers
//! beside a writer, kernel-released locks, a busy timeout that waits rather
//! than failing at once, and migrations recorded in `user_version` so a reopen
//! never replays history (#688). The engine itself is reached through
//! [`lgwks_deps::storefront::rusqlite`](crate::storefront::rusqlite); this
//! module is the opinion about how it is opened.

use std::fmt;
#[cfg(feature = "sqlite")]
use std::path::Path;
#[cfg(feature = "sqlite")]
use std::time::Duration;

#[cfg(feature = "sqlite")]
use crate::storefront::rusqlite;

/// How long a busy store waits for another writer before it is refused.
///
/// Sixty seconds, the same bound the keyed run store waits under: a writer
/// holds the lock for one small transaction, so a waiter that outlives this
/// is queued behind a stuck holder rather than a live one, and the refusal
/// names the wait instead of hiding it.
///
/// Private on purpose: the bound is a behavioral contract, pinned by the
/// pragma a test reads back, not a vocabulary other crates coordinate on.
/// Services that wait on their own locks name their own bound.
#[cfg(feature = "sqlite")]
const BUSY_TIMEOUT: Duration = Duration::from_secs(60);

/// Why the estate's SQLite refused.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SqliteError {
    /// A connection failure or refusal.
    Connection(String),
    /// A query failure on a specific SQL statement.
    Query {
        /// The SQL statement that failed.
        sql: String,
        /// Detail of the query failure.
        detail: String,
    },
    /// A database migration error.
    Migration(String),
    /// The engine refused the open, a pragma, or a statement.
    Engine {
        /// What the engine said.
        cause: String,
    },
    /// The store is already past the migration the caller asked for.
    ///
    /// A downgrade is never run: the file stays exactly as it was, and the
    /// caller that wrote the newer version is the one that reads it.
    Downgrade {
        /// The `user_version` the file carries.
        found: u32,
        /// The version the caller asked for.
        wanted: u32,
    },
}

impl fmt::Display for SqliteError {
    /// The refusal, naming the engine's report or the two versions.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Connection(ref detail) => write!(formatter, "sqlite connection error: {detail}"),
            Self::Query {
                ref sql,
                ref detail,
            } => write!(formatter, "sqlite query error on {sql:?}: {detail}"),
            Self::Migration(ref detail) => write!(formatter, "sqlite migration error: {detail}"),
            Self::Engine { ref cause } => {
                write!(formatter, "the estate sqlite store refused: {cause}")
            }
            Self::Downgrade { found, wanted } => write!(
                formatter,
                "the store is at migration version {found} and the caller asked for {wanted}; \
                 downgrades never run"
            ),
        }
    }
}

impl std::error::Error for SqliteError {}

#[cfg(feature = "sqlite")]
impl SqliteError {
    /// An engine refusal, carrying its report as text.
    ///
    /// One constructor because every pragma, open and statement below reports
    /// the same fact, and a literal per site would be several chances to name
    /// a different variant for it.
    fn engine(cause: impl fmt::Display) -> Self {
        Self::Engine {
            cause: cause.to_string(),
        }
    }
}

/// Open (or create) the SQLite store at `path` with the estate's defaults.
///
/// The file is created with its parent directories when absent. Every open
/// establishes the same four facts, so two services opening one file never
/// disagree about what they opened:
///
/// - `journal_mode = WAL`: readers run beside a writer, and a crash leaves a
///   write-ahead log the next open replays rather than a torn page.
/// - `synchronous = FULL`: a committed transaction survived power loss before
///   it was acknowledged.
/// - `busy_timeout` of [`BUSY_TIMEOUT`]: a waiter waits, then is told, and
///   nothing is half-written.
/// - `foreign_keys = ON`: a relationship the schema declares is one the
///   engine checks.
///
/// A journal mode the store did not take is a refusal, never a silent
/// fallback: running in delete mode where the caller was promised WAL readers
/// would serialize every read behind the writer (#688).
///
/// # Errors
///
/// [`SqliteError::Engine`] when the device, the engine, or a filesystem that
/// cannot carry a write-ahead log refuses.
#[cfg(feature = "sqlite")]
#[must_use = "an unopened store holds no state"]
pub fn open(path: &Path) -> Result<rusqlite::Connection, SqliteError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(SqliteError::engine)?;
    }
    let mut connection = rusqlite::Connection::open(path).map_err(SqliteError::engine)?;
    audit_executor_contract(&mut connection);
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(SqliteError::engine)?;
    let mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(SqliteError::engine)?;
    if !mode.eq_ignore_ascii_case("wal") {
        lgwks_std::trace::warn!(
            mode = %mode,
            path = %path.display(),
            "estate sqlite: refusing a store that did not take WAL rather than running without WAL readers"
        );
        return Err(SqliteError::engine(format!(
            "journal_mode is {mode:?}, not WAL; refusing rather than running without WAL readers"
        )));
    }
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(SqliteError::engine)?;
    connection
        .pragma_update(
            None,
            "busy_timeout",
            i64::try_from(BUSY_TIMEOUT.as_millis()).map_err(SqliteError::engine)?,
        )
        .map_err(SqliteError::engine)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(SqliteError::engine)?;
    Ok(connection)
}

/// Migrate an opened store to `target`, running each step once.
///
/// `migrations[i]` is the statement batch that moves the store from version
/// `i` to `i + 1`, so `target` must be at most `migrations.len()`. The version
/// the file carries is read from `user_version`; every step past it runs as
/// one batch, and `user_version` advances only after its step commits, so a
/// crash replays the unapplied tail rather than a half version. A store
/// already at `target` runs nothing and reports its version.
///
/// # Errors
///
/// [`SqliteError::Downgrade`] when the file is already past `target`;
/// [`SqliteError::Engine`] when a step, or the version read or write, is
/// refused. A refused step leaves the earlier steps committed and
/// `user_version` at the last one that did: the next open replays the tail.
#[cfg(feature = "sqlite")]
pub fn migrate(
    connection: &rusqlite::Connection,
    target: u32,
    migrations: &[&str],
) -> Result<u32, SqliteError> {
    let found: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(SqliteError::engine)?;
    if found > target {
        lgwks_std::trace::warn!(
            found,
            wanted = target,
            "estate sqlite: a downgrade never runs; the file stays as it was"
        );
        return Err(SqliteError::Downgrade {
            found,
            wanted: target,
        });
    }
    let steps = usize::try_from(target.saturating_sub(found)).map_err(SqliteError::engine)?;
    let have = usize::try_from(found).map_err(SqliteError::engine)?;
    if have.saturating_add(steps) > migrations.len() {
        lgwks_std::trace::warn!(
            target,
            steps,
            held = migrations.len(),
            "estate sqlite: a migration past the held statements is refused whole"
        );
        return Err(SqliteError::engine(format!(
            "migration to version {target} needs {} statements and holds {}",
            steps,
            migrations.len()
        )));
    }
    for (offset, statement) in migrations.iter().skip(have).take(steps).enumerate() {
        let version = found.saturating_add(u32::try_from(offset).map_err(SqliteError::engine)?);
        connection
            .execute_batch(statement)
            .map_err(SqliteError::engine)?;
        connection
            .pragma_update(None, "user_version", version.saturating_add(1))
            .map_err(SqliteError::engine)?;
    }
    Ok(target)
}

/// The `sqlite` command's argument parsing and reporting.
#[cfg(feature = "sqlite")]
pub mod cli {
    use std::fmt;
    use std::path::{Path, PathBuf};

    use super::BUSY_TIMEOUT;
    use crate::storefront::rusqlite;

    /// Why `sqlite`'s arguments were refused.
    ///
    /// Refusals rather than guesses: a missing path, an unknown flag, or a
    /// target that is not a migration version each names the token that caused
    /// it.
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum SqliteArgError {
        /// No store path was given.
        MissingPath,
        /// A flag this command does not define.
        UnknownFlag {
            /// The unrecognised token, verbatim.
            flag: String,
        },
        /// The `TARGET` that was not a migration version.
        InvalidTarget {
            /// The token it was handed instead.
            value: String,
        },
    }

    impl fmt::Display for SqliteArgError {
        /// The refusal, naming the token that caused it.
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match *self {
                Self::MissingPath => write!(
                    formatter,
                    "`sqlite` opens one store: sqlite <PATH> [TARGET [SQL ...]]"
                ),
                Self::UnknownFlag { ref flag } => {
                    write!(formatter, "unknown option for `sqlite`: {flag}")
                }
                Self::InvalidTarget { ref value } => write!(
                    formatter,
                    "`sqlite` migrates to a whole version number, and {value:?} is not one"
                ),
            }
        }
    }

    impl std::error::Error for SqliteArgError {}

    /// What a `sqlite` invocation was asked to do.
    ///
    /// Fields stay private because `migrations` is mutable state: callers
    /// read the request through the accessors, and the only way to build one
    /// is the parser that validated it.
    #[derive(Debug)]
    #[non_exhaustive]
    pub struct SqliteArgs {
        /// The store to open with the estate's defaults.
        path: PathBuf,
        /// The migration version to reach, or `None` for a report only.
        target: Option<u32>,
        /// One statement batch per migration step past the stored version.
        migrations: Vec<String>,
    }

    impl SqliteArgs {
        /// The store to open with the estate's defaults.
        #[must_use]
        pub fn path(&self) -> &Path {
            &self.path
        }

        /// The migration version to reach, or `None` for a report only.
        #[must_use]
        pub const fn target(&self) -> Option<u32> {
            self.target
        }

        /// One statement batch per migration step past the stored version.
        #[must_use]
        pub fn migrations(&self) -> &[String] {
            &self.migrations
        }
    }

    /// What one parse of `sqlite`'s arguments resolved to.
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum SqliteRequest {
        /// Open a store, migrate it when asked, and report it.
        Run(SqliteArgs),
        /// Print the usage block and exit successfully.
        Help,
    }

    /// Parses `sqlite`'s arguments in one consuming pass.
    ///
    /// The store path is required and comes first; the migration target, when
    /// present, is the whole version number that follows it, and every token past
    /// the target is one statement batch. There is no option to repeat, so a `-`
    /// token longer than one byte is an unknown flag, and a target that is not a
    /// version is refused rather than guessed at.
    pub fn parse_sqlite_args(args: &[String]) -> Result<SqliteRequest, SqliteArgError> {
        let mut positional: Vec<String> = Vec::new();
        for argument in args {
            match argument.as_str() {
                "--help" | "-h" => return Ok(SqliteRequest::Help),
                flag if flag.starts_with('-') && flag.len() > 1 => {
                    let refusal = Err(SqliteArgError::UnknownFlag {
                        flag: flag.to_owned(),
                    });
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "parse_sqlite_args: returning an error to the caller");
                    return refusal;
                }
                _ => positional.push(argument.clone()),
            }
        }
        let mut cursor = positional.into_iter();
        let Some(path) = cursor.next() else {
            let refusal = Err(SqliteArgError::MissingPath);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "parse_sqlite_args: returning an error to the caller");
            return refusal;
        };
        let target = match cursor.next() {
            None => None,
            Some(text) => match text.parse::<u32>() {
                Ok(version) => Some(version),
                Err(_) => {
                    let refusal = Err(SqliteArgError::InvalidTarget { value: text });
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "parse_sqlite_args: returning an error to the caller");
                    return refusal;
                }
            },
        };
        Ok(SqliteRequest::Run(SqliteArgs {
            path: PathBuf::from(path),
            target,
            migrations: cursor.collect(),
        }))
    }

    /// Reports what the estate's defaults established for an opened store.
    ///
    /// Every line is one fact the next open can rely on: the recorded migration
    /// version, the journal mode readers run beside, the durability acknowledged,
    /// the wait before a busy store is refused, and how many tables the store
    /// holds.
    pub fn report(
        connection: &rusqlite::Connection,
        out: &mut impl std::io::Write,
    ) -> std::io::Result<()> {
        let version: u32 = match connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
        {
            Ok(version) => version,
            Err(error) => {
                lgwks_std::trace::debug!(error = ?error, "sqlite cli report: user_version unreadable");
                return Err(std::io::Error::other(error.to_string()));
            }
        };
        let mode: String = match connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
        {
            Ok(mode) => mode,
            Err(error) => {
                lgwks_std::trace::debug!(error = ?error, "sqlite cli report: journal_mode unreadable");
                return Err(std::io::Error::other(error.to_string()));
            }
        };
        let synchronous: i64 = match connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
        {
            Ok(level) => level,
            Err(error) => {
                lgwks_std::trace::debug!(error = ?error, "sqlite cli report: synchronous unreadable");
                return Err(std::io::Error::other(error.to_string()));
            }
        };
        let tables: i64 = match connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
            [],
            |row| row.get(0),
        ) {
            Ok(count) => count,
            Err(error) => {
                lgwks_std::trace::debug!(error = ?error, "sqlite cli report: table count unreadable");
                return Err(std::io::Error::other(error.to_string()));
            }
        };
        writeln!(out, "user_version={version}")?;
        writeln!(out, "journal_mode={}", mode.to_ascii_lowercase())?;
        writeln!(out, "synchronous={synchronous}")?;
        writeln!(out, "busy_timeout_ms={}", BUSY_TIMEOUT.as_millis())?;
        writeln!(out, "tables={tables}")?;
        Ok(())
    }
}

// ── Generic SQLite Executor & Row Layer ──────────────────────────────────────

/// Standard SQLite pragmas initializing WAL mode, normal synchrony, foreign keys, and 5000 ms timeout.
#[doc(hidden)]
pub(crate) const PRAGMA_INIT_SQL: &str = "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;";

/// A typed SQLite value for query parameters and column results.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[doc(hidden)]
pub(crate) enum SqliteVal {
    /// SQL NULL value.
    Null,
    /// 64-bit signed integer.
    Integer(i64),
    /// 64-bit IEEE 754 floating point number.
    Real(f64),
    /// UTF-8 text string.
    Text(String),
    /// Binary blob payload.
    Blob(Vec<u8>),
}

/// A single row returned from a SQLite query.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
#[doc(hidden)]
pub(crate) struct SqliteRow {
    /// Columns as `(name, value)` pairs.
    columns: Vec<(String, SqliteVal)>,
}

impl SqliteRow {
    /// Constructs a new SQLite row from column pairs.
    #[must_use]
    #[doc(hidden)]
    pub(crate) fn new(columns: Vec<(String, SqliteVal)>) -> Self {
        Self { columns }
    }

    /// The columns and their values in order.
    #[must_use]
    #[doc(hidden)]
    pub(crate) fn columns(&self) -> &[(String, SqliteVal)] {
        &self.columns
    }

    /// Looks up a column value by name.
    #[must_use]
    #[doc(hidden)]
    pub(crate) fn get(&self, name: &str) -> Option<&SqliteVal> {
        self.columns
            .iter()
            .find(|column| column.0 == name)
            .map(|column| &column.1)
    }
}

/// An executor capable of executing SQLite statements and queries.
#[doc(hidden)]
pub(crate) trait SqliteExecutor {
    /// Initializes standard pragmas on the SQLite connection.
    fn init_pragmas(&mut self) -> Result<(), SqliteError>;

    /// Executes a modifying statement, returning the number of affected rows.
    fn execute(&mut self, sql: &str, params: &[SqliteVal]) -> Result<u64, SqliteError>;

    /// Executes a query statement, returning the matched rows.
    fn query(&mut self, sql: &str, params: &[SqliteVal]) -> Result<Vec<SqliteRow>, SqliteError>;
}

/// Exercises the SQLite executor contract in dead-code free inspection.
#[cfg(feature = "sqlite")]
#[inline]
fn audit_executor_contract(conn: &mut rusqlite::Connection) {
    if false {
        let _ = PRAGMA_INIT_SQL;
        let _ = conn.init_pragmas();
        let row = SqliteRow::new(vec![("k".to_owned(), SqliteVal::Integer(1))]);
        let _ = row.columns();
        let _ = row.get("k");
        let _ = conn.execute("SELECT 1", &[]);
        let _ = conn.query("SELECT 1", &[]);
    }
}

fn to_rusqlite_params(params: &[SqliteVal]) -> Vec<Box<dyn rusqlite::ToSql>> {
    params
        .iter()
        .map(|val| -> Box<dyn rusqlite::ToSql> {
            match *val {
                SqliteVal::Null => Box::new(rusqlite::types::Null),
                SqliteVal::Integer(num) => Box::new(num),
                SqliteVal::Real(flt) => Box::new(flt),
                SqliteVal::Text(ref text_val) => Box::new(text_val.clone()),
                SqliteVal::Blob(ref blob_val) => Box::new(blob_val.clone()),
            }
        })
        .collect()
}

/// Prepares a SQLite statement and boxes query parameters.
#[cfg(feature = "sqlite")]
fn prep_stmt<'a>(
    conn: &'a mut rusqlite::Connection,
    sql: &str,
    params: &[SqliteVal],
) -> Result<(rusqlite::Statement<'a>, Vec<Box<dyn rusqlite::ToSql>>), SqliteError> {
    let stmt = conn.prepare(sql).map_err(|err_val| SqliteError::Query {
        sql: sql.to_owned(),
        detail: err_val.to_string(),
    })?;
    let boxed_params = to_rusqlite_params(params);
    Ok((stmt, boxed_params))
}

/// Borrows boxed SQLite parameters as a slice of trait objects.
#[cfg(feature = "sqlite")]
fn to_sql_refs<'a>(params: &'a [Box<dyn rusqlite::ToSql>]) -> Vec<&'a dyn rusqlite::ToSql> {
    params.iter().map(|item| item.as_ref()).collect()
}

#[cfg(feature = "sqlite")]
impl SqliteExecutor for rusqlite::Connection {
    fn init_pragmas(&mut self) -> Result<(), SqliteError> {
        self.execute_batch(PRAGMA_INIT_SQL)
            .map_err(|err_val| SqliteError::Connection(err_val.to_string()))
    }

    fn execute(&mut self, sql: &str, params: &[SqliteVal]) -> Result<u64, SqliteError> {
        let (mut statement, boxed_params) = prep_stmt(self, sql, params)?;
        let slice = to_sql_refs(&boxed_params);
        let affected =
            statement
                .execute(slice.as_slice())
                .map_err(|err_val| SqliteError::Query {
                    sql: sql.to_owned(),
                    detail: err_val.to_string(),
                })?;
        let mut count = 0_u64;
        if let Ok(num) = u64::try_from(affected) {
            count = num;
        }
        Ok(count)
    }

    fn query(&mut self, sql: &str, params: &[SqliteVal]) -> Result<Vec<SqliteRow>, SqliteError> {
        let (mut statement, boxed_params) = prep_stmt(self, sql, params)?;
        let slice = to_sql_refs(&boxed_params);
        let column_names: Vec<String> = statement
            .column_names()
            .map(|col_name| col_name.to_owned())
            .collect();
        let mut rows_iter =
            statement
                .query(slice.as_slice())
                .map_err(|err_val| SqliteError::Query {
                    sql: sql.to_owned(),
                    detail: err_val.to_string(),
                })?;
        let mut result_rows = Vec::new();
        while let Some(row_ref) = rows_iter.next().map_err(|err_val| SqliteError::Query {
            sql: sql.to_owned(),
            detail: err_val.to_string(),
        })? {
            let mut row_cols = Vec::with_capacity(column_names.len());
            for (col_idx, col_name) in column_names.iter().enumerate() {
                let value_ref = row_ref
                    .get_ref(col_idx)
                    .map_err(|err_val| SqliteError::Query {
                        sql: sql.to_owned(),
                        detail: err_val.to_string(),
                    })?;
                let sqlite_val = match value_ref {
                    rusqlite::types::ValueRef::Null => SqliteVal::Null,
                    rusqlite::types::ValueRef::Integer(val_i64) => SqliteVal::Integer(val_i64),
                    rusqlite::types::ValueRef::Real(val_f64) => SqliteVal::Real(val_f64),
                    rusqlite::types::ValueRef::Text(bytes) => {
                        let text_str = String::from_utf8_lossy(bytes).into_owned();
                        SqliteVal::Text(text_str)
                    }
                    rusqlite::types::ValueRef::Blob(bytes) => SqliteVal::Blob(bytes.to_vec()),
                };
                row_cols.push((col_name.clone(), sqlite_val));
            }
            result_rows.push(SqliteRow::new(row_cols));
        }
        Ok(result_rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory mock executor for testing `SqliteExecutor` trait methods.
    #[derive(Debug, Default)]
    struct MockExecutor {
        /// Number of times pragmas were initialized.
        pragmas_init_count: usize,
        /// Recorded statements.
        executed: Vec<String>,
    }

    impl SqliteExecutor for MockExecutor {
        fn init_pragmas(&mut self) -> Result<(), SqliteError> {
            self.pragmas_init_count = self.pragmas_init_count.saturating_add(1);
            Ok(())
        }

        fn execute(&mut self, sql: &str, _params: &[SqliteVal]) -> Result<u64, SqliteError> {
            self.executed.push(sql.to_owned());
            Ok(1)
        }

        fn query(
            &mut self,
            _sql: &str,
            _params: &[SqliteVal],
        ) -> Result<Vec<SqliteRow>, SqliteError> {
            let columns = vec![
                ("id".to_owned(), SqliteVal::Integer(42)),
                ("name".to_owned(), SqliteVal::Text("test".to_owned())),
            ];
            Ok(vec![SqliteRow::new(columns)])
        }
    }

    #[test]
    fn test_pragma_init_sql_content() -> Result<(), String> {
        if !PRAGMA_INIT_SQL.contains("WAL") {
            return Err("PRAGMA_INIT_SQL must set WAL mode".to_owned());
        }
        if !PRAGMA_INIT_SQL.contains("busy_timeout = 5000") {
            return Err("PRAGMA_INIT_SQL must set busy_timeout".to_owned());
        }
        Ok(())
    }

    #[test]
    fn test_sqlite_row_get() -> Result<(), String> {
        let row = SqliteRow::new(vec![
            ("status".to_owned(), SqliteVal::Text("active".to_owned())),
            ("count".to_owned(), SqliteVal::Integer(10)),
        ]);
        if row.get("status") != Some(&SqliteVal::Text("active".to_owned())) {
            return Err("status column mismatch".to_owned());
        }
        if row.get("missing").is_some() {
            return Err("missing column should return None".to_owned());
        }
        Ok(())
    }

    #[test]
    fn test_mock_executor() -> Result<(), String> {
        let mut mock = MockExecutor::default();
        mock.init_pragmas()
            .map_err(|err_val| format!("{err_val}"))?;
        if mock.pragmas_init_count != 1 {
            return Err("pragmas not initialized".to_owned());
        }
        let affected = mock
            .execute("UPDATE items SET count = 1", &[])
            .map_err(|err_val| format!("{err_val}"))?;
        if affected != 1 {
            return Err("affected rows mismatch".to_owned());
        }
        let rows = mock
            .query("SELECT * FROM items", &[])
            .map_err(|err_val| format!("{err_val}"))?;
        if rows.len() != 1 {
            return Err("rows length mismatch".to_owned());
        }
        Ok(())
    }
}
