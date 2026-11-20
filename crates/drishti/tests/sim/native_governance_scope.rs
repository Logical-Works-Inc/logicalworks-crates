//! Governance-log scope and schema sims: counts stay inside one tenant, a
//! database written by the first release (signal id alone as the key) is
//! migrated in place without losing a row, stored attribution names are
//! resolved strictly, and instants beyond the signed range read as empty
//! windows instead of store failures.

use std::collections::BTreeMap;
use std::path::Path;

use drishti::native_host::governance::{
    AttributionSource, GovernanceError, GovernanceQuery, GovernanceStore,
};
use lgwks_deps::rusqlite::{self, Connection, params};

use crate::fixtures::governance_entry_in;
use crate::motion_kit::{describe, ensure, pick};
use crate::native_governance::{cleanup, scratch, stamp};
use crate::rng::SimRng;

/// Milliseconds in a day.
const DAY_MS: u64 = 86_400_000;
/// The two tenants of the legacy file.
const LEGACY_TENANTS: [&str; 2] = ["tenant-a", "tenant-b"];
/// The two sessions of the legacy file.
const LEGACY_SESSIONS: [&str; 2] = ["s1", "s2"];

/// Entries of one session of one tenant, through the call under test.
fn count_in(store: &GovernanceStore, tenant: &str, session: &str) -> Result<u64, GovernanceError> {
    store.count(tenant, session)
}

/// Bug caught: a per-session count that adds up every tenant's rows under a
/// shared session id, so one tenant learns how busy another tenant's
/// session is (and a quota or audit total is wrong for both).
#[test]
fn sim_governance_count_is_scoped_to_the_tenant() -> Result<(), String> {
    const TENANTS: &[&str] = &["tenant-a", "tenant-b", "tenant-c"];
    const SESSIONS: &[&str] = &["shared", "other"];
    for seed in 0..20_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(340_000));
        let store = GovernanceStore::open_in_memory().map_err(describe)?;
        let mut held: BTreeMap<(&str, &str), u64> = BTreeMap::new();
        for step in 0..150_u64 {
            let tenant = pick(&mut random, TENANTS)?;
            let session = pick(&mut random, SESSIONS)?;
            let entry = governance_entry_in(
                tenant,
                &format!("signal-{step}"),
                session,
                "tick",
                &stamp(random.below(1_000)),
            );
            store.append(&entry).map_err(describe)?;
            let slot = held.entry((tenant, session)).or_insert(0);
            *slot = slot.saturating_add(1);
            let probe_tenant = pick(&mut random, TENANTS)?;
            let probe_session = pick(&mut random, SESSIONS)?;
            let want: u64 = held
                .get(&(probe_tenant, probe_session))
                .into_iter()
                .copied()
                .sum();
            let got = count_in(&store, probe_tenant, probe_session).map_err(describe)?;
            ensure(
                got == want,
                &format!(
                    "seed {seed} step {step}: {probe_tenant}/{probe_session} counted {got}, model holds {want}"
                ),
            )?;
        }
        ensure(
            count_in(&store, "tenant-none", "shared").map_err(describe)? == 0,
            "a tenant with no rows must count zero, whatever others hold",
        )?;
    }
    Ok(())
}

/// The table as the first release created it: the signal id alone was the key.
const LEGACY_SCHEMA: &str = "CREATE TABLE governance_entries (
    signal_id TEXT PRIMARY KEY,
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
CREATE INDEX idx_governance_scope_time
    ON governance_entries (tenant_id, session_id, timestamp_ms);";

/// One row of the legacy file.
struct Legacy {
    /// Owning tenant.
    tenant: &'static str,
    /// Session within the tenant.
    session: &'static str,
    /// Signal id, globally unique in the legacy file.
    signal: String,
    /// Record minute of the first day.
    minute: u64,
}

/// Create a database file with the legacy schema and the given rows.
fn write_legacy(path: &Path, rows: &[Legacy]) -> Result<(), String> {
    let connection = Connection::open(path).map_err(describe)?;
    connection.execute_batch(LEGACY_SCHEMA).map_err(describe)?;
    for row in rows {
        let timestamp = i64::try_from(row.minute.saturating_mul(60_000)).map_err(describe)?;
        connection
            .execute(
                "INSERT INTO governance_entries
                    (signal_id, tenant_id, session_id, source_kind, kind, attribution,
                     timestamp_ms, recorded_at, policy_bundle_id, work_object_id)
                 VALUES (?1, ?2, ?3, 'user', 'tick', 'user', ?4, ?5, 'policy-legacy', 'work-legacy')",
                params![
                    row.signal,
                    row.tenant,
                    row.session,
                    timestamp,
                    stamp(row.minute)
                ],
            )
            .map_err(describe)?;
    }
    Ok(())
}

/// What the log reads for one tenant session, one text line per row.
fn rows_of(store: &GovernanceStore, tenant: &str, session: &str) -> Result<Vec<String>, String> {
    let query = GovernanceQuery::new(
        String::from(tenant),
        String::from(session),
        0,
        DAY_MS,
        None,
        None,
    )
    .map_err(describe)?;
    Ok(store
        .query(&query)
        .map_err(describe)?
        .iter()
        .map(|row| {
            format!(
                "{}|{}|{}|{}|{}",
                row.signal_id(),
                row.timestamp_ms(),
                row.recorded_at(),
                row.policy_bundle_id(),
                row.work_object_id()
            )
        })
        .collect())
}

/// Every tenant session of the legacy file as read from the store.
fn everything(store: &GovernanceStore) -> Result<Vec<Vec<String>>, String> {
    let mut all = Vec::new();
    for tenant in LEGACY_TENANTS {
        for session in LEGACY_SESSIONS {
            all.push(rows_of(store, tenant, session)?);
        }
    }
    Ok(all)
}

/// The reference read of the legacy rows: filter, then oldest first with ties
/// in insertion order.
fn legacy_model(rows: &[Legacy]) -> Vec<Vec<String>> {
    let mut all = Vec::new();
    for tenant in LEGACY_TENANTS {
        for session in LEGACY_SESSIONS {
            let mut kept: Vec<&Legacy> = rows
                .iter()
                .filter(|row| row.tenant == tenant && row.session == session)
                .collect();
            kept.sort_by_key(|row| row.minute);
            all.push(
                kept.iter()
                    .map(|row| {
                        format!(
                            "{}|{}|{}|policy-legacy|work-legacy",
                            row.signal,
                            row.minute.saturating_mul(60_000),
                            stamp(row.minute)
                        )
                    })
                    .collect(),
            );
        }
    }
    all
}

/// The schema version, the count of tables other than the log, and the
/// primary-key columns in key order, read straight from the file.
fn schema_facts(path: &Path) -> Result<(i64, i64, Vec<String>), String> {
    let connection = Connection::open(path).map_err(describe)?;
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(describe)?;
    let strays: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name != 'governance_entries'",
            [],
            |row| row.get(0),
        )
        .map_err(describe)?;
    let mut statement = connection
        .prepare(
            "SELECT name FROM pragma_table_info('governance_entries')
             WHERE pk > 0 ORDER BY pk",
        )
        .map_err(describe)?;
    let keys = statement
        .query_map([], |row| row.get(0))
        .map_err(describe)?
        .collect::<Result<Vec<String>, rusqlite::Error>>()
        .map_err(describe)?;
    Ok((version, strays, keys))
}

/// Bug caught: a database written with the signal id as the only key keeps
/// that key after an upgrade (tenant B is refused an id tenant A holds, and
/// learns that it exists), or the upgrade loses, reorders or duplicates a
/// row, leaves a stray table, or runs again on the next open; a file from a
/// newer build must be refused rather than half read.
#[test]
fn sim_governance_legacy_database_migrates_in_place() -> Result<(), String> {
    for seed in 0..4_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(350_000));
        let path = scratch("legacy", seed);
        cleanup(&path)?;
        let mut rows = Vec::new();
        for index in 0..40_u64 {
            rows.push(Legacy {
                tenant: pick(&mut random, &LEGACY_TENANTS)?,
                session: pick(&mut random, &LEGACY_SESSIONS)?,
                signal: format!("legacy-{index}"),
                minute: random.below(1_400),
            });
        }
        write_legacy(&path, &rows)?;
        let store = GovernanceStore::open(&path).map_err(describe)?;
        ensure(
            everything(&store)? == legacy_model(&rows),
            &format!("seed {seed}: the migrated log differs from the legacy rows"),
        )?;
        let anchor = rows.get(3).ok_or("the legacy file holds a fourth row")?;
        let owner = anchor.tenant;
        let other = if owner == "tenant-a" {
            "tenant-b"
        } else {
            "tenant-a"
        };
        let taken = anchor.signal.clone();
        ensure(
            store.append(&governance_entry_in(other, &taken, "s1", "tick", &stamp(5))) == Ok(()),
            &format!("seed {seed}: the other tenant was refused an id the owner holds"),
        )?;
        ensure(
            store.append(&governance_entry_in(owner, &taken, "s1", "tick", &stamp(5)))
                == Err(GovernanceError::Duplicate(taken.clone())),
            &format!("seed {seed}: a repeat inside the owning tenant was accepted"),
        )?;
        let before = everything(&store)?;
        drop(store);
        let facts = schema_facts(&path)?;
        ensure(
            facts.0 == 1 && facts.1 == 0 && facts.2 == ["tenant_id", "signal_id"],
            &format!("seed {seed}: schema facts after the upgrade were {facts:?}"),
        )?;
        let again = GovernanceStore::open(&path).map_err(describe)?;
        ensure(
            everything(&again)? == before,
            &format!("seed {seed}: reopening an upgraded file changed the rows"),
        )?;
        drop(again);
        {
            let connection = Connection::open(&path).map_err(describe)?;
            connection
                .pragma_update(None, "user_version", 9)
                .map_err(describe)?;
        }
        ensure(
            matches!(GovernanceStore::open(&path), Err(GovernanceError::Store(_))),
            &format!("seed {seed}: a file from a newer build was opened"),
        )?;
        cleanup(&path)?;
    }
    Ok(())
}

/// The names a stored attribution may carry.
const KNOWN_NAMES: [&str; 5] = ["ml", "user", "user_intent", "widget", "system"];

/// The source a stored name resolves to, or the typed refusal.
fn resolved(raw: &str) -> Result<AttributionSource, GovernanceError> {
    AttributionSource::resolve(raw)
}

/// A seeded near miss of a known name.
fn near_miss(random: &mut SimRng) -> Result<String, String> {
    let base = pick(random, &KNOWN_NAMES)?;
    Ok(match random.below(6) {
        0 => base.to_uppercase(),
        1 => format!("{base} "),
        2 => format!(" {base}"),
        3 => format!("{base}\0"),
        4 => base.chars().take(base.len().saturating_sub(1)).collect(),
        _ => format!("{base}{}", random.text(2)),
    })
}

/// Bug caught: an attribution name nobody issued is read as the system, so a
/// corrupted or forged row is credited to the most trusted source instead of
/// being refused.
#[test]
fn sim_governance_unknown_attribution_is_refused_not_system() -> Result<(), String> {
    for (raw, want) in [
        ("ml", AttributionSource::Ml),
        ("user", AttributionSource::User),
        ("user_intent", AttributionSource::User),
        ("widget", AttributionSource::Widget),
        ("system", AttributionSource::System),
    ] {
        ensure(
            resolved(raw) == Ok(want),
            &format!("{raw:?} is a known name and must resolve to {want:?}"),
        )?;
    }
    for source in [
        AttributionSource::User,
        AttributionSource::Widget,
        AttributionSource::Ml,
        AttributionSource::System,
    ] {
        ensure(
            resolved(source.raw_value()) == Ok(source),
            "the stored name of every source must resolve back to it",
        )?;
    }
    for seed in 0..400_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(360_000));
        let candidate = near_miss(&mut random)?;
        let known = KNOWN_NAMES.contains(&candidate.as_str());
        ensure(
            resolved(&candidate).is_ok() == known,
            &format!("seed {seed}: {candidate:?} resolved against the rule (known: {known})"),
        )?;
    }
    for raw in ["", "root", "admin", "ML", "User", "systém", "unknown"] {
        ensure(
            resolved(raw) == Err(GovernanceError::UnknownAttribution(String::from(raw))),
            &format!("{raw:?} must be the typed refusal naming it"),
        )?;
    }
    Ok(())
}

/// Bug caught: a query window beyond the signed 64-bit range surfaces as a
/// store failure (a host error for a question with a provable answer), or is
/// clamped into a window that returns rows from the wrong instant.
#[test]
fn sim_governance_absurd_instants_read_as_empty_windows() -> Result<(), String> {
    let store = GovernanceStore::open_in_memory().map_err(describe)?;
    for minute in [0_u64, 10, 600, 1_439] {
        store
            .append(&governance_entry_in(
                "tenant-w",
                &format!("sane-{minute}"),
                "session",
                "tick",
                &stamp(minute),
            ))
            .map_err(describe)?;
    }
    let edge = u64::try_from(i64::MAX).map_err(describe)?;
    for seed in 0..80_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(370_000));
        let from = match random.below(4) {
            0 => u64::MAX.saturating_sub(random.below(DAY_MS)),
            1 => edge.saturating_sub(random.below(1_000)),
            2 => edge.saturating_add(random.below(1_000)),
            _ => edge,
        };
        let to = from.saturating_add(random.below(DAY_MS));
        let glob = if random.below(2) == 0 {
            Some(String::from("*"))
        } else {
            None
        };
        let query = GovernanceQuery::new(
            String::from("tenant-w"),
            String::from("session"),
            from,
            to,
            None,
            glob,
        )
        .map_err(describe)?;
        ensure(
            store.query(&query) == Ok(Vec::new()),
            &format!("seed {seed}: query [{from}, {to}] was not an empty read"),
        )?;
        ensure(
            store.recent(&query, 5) == Ok(Vec::new()),
            &format!("seed {seed}: recent [{from}, {to}] was not an empty read"),
        )?;
    }
    let sane = GovernanceQuery::new(
        String::from("tenant-w"),
        String::from("session"),
        0,
        DAY_MS,
        None,
        None,
    )
    .map_err(describe)?;
    ensure(
        store.query(&sane).map_err(describe)?.len() == 4,
        "an ordinary window must still read its four rows",
    )
}
