//! Governance store and tape reader contract.
//!
//! Counterpart of `CanvasStoresTests` (Governance, Abuse, Persistence) and
//! `CanvasTelemetryTests/TapeReaderTests` (kernel#850 row 15): append and
//! duplicate rules, malformed entries, bounded queries with glob and
//! attribution filters, a 1,000-entry round trip, and newest-last tape reads
//! with a clamped window.

use drishti::native_host::broker::Scope;
use drishti::native_host::governance::{
    AttributionSource, GovernanceEntrySnapshot, GovernanceError, GovernanceQuery, GovernanceStore,
};
use drishti::native_host::telemetry::TapeReader;

#[path = "support/fixtures.rs"]
mod fixtures;

use fixtures::TENANT;
use fixtures::governance_entry as entry;

/// Query fixture over the first hour of the epoch day.
fn query(session_id: &str) -> Result<GovernanceQuery, String> {
    GovernanceQuery::new(
        String::from(TENANT),
        String::from(session_id),
        0,
        3_600_000,
        None,
        None,
    )
    .map_err(|error| format!("{error}"))
}

/// Scope fixture for one session of the fixture tenant.
fn scope(session_id: &str) -> Scope {
    Scope::new(TENANT, session_id)
}

/// Append counts, duplicates fail, empty signals are malformed.
#[test]
fn append_counts_and_rejects_duplicates() -> Result<(), String> {
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    store
        .append(&entry(
            "signal-1",
            "session",
            "forms.submitted",
            "1970-01-01T00:00:00Z",
        ))
        .map_err(|error| format!("{error}"))?;
    if store
        .count(TENANT, "session")
        .map_err(|error| format!("{error}"))?
        != 1
    {
        return Err(String::from("count must be one"));
    }
    let duplicated = store.append(&entry(
        "signal-1",
        "session",
        "forms.submitted",
        "1970-01-01T00:00:00Z",
    ));
    if duplicated != Err(GovernanceError::Duplicate(String::from("signal-1"))) {
        return Err(String::from("duplicate signal must fail"));
    }
    let malformed = store.append(&entry(
        "",
        "session",
        "forms.submitted",
        "1970-01-01T00:00:00Z",
    ));
    if !matches!(malformed, Err(GovernanceError::MalformedEntry(_))) {
        return Err(String::from("empty signal must be malformed"));
    }
    let corrupt = store.append(&entry(
        "signal-2",
        "session",
        "forms.submitted",
        "not a timestamp",
    ));
    if !matches!(corrupt, Err(GovernanceError::MalformedEntry(_))) {
        return Err(String::from("bad recorded_at must be malformed"));
    }
    Ok(())
}

/// Queries filter by session, window, attribution, and kind glob.
#[test]
fn queries_filter_session_window_attribution_and_glob() -> Result<(), String> {
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    store
        .append(&entry(
            "signal-1",
            "session-a",
            "intent.find",
            "1970-01-01T00:10:00Z",
        ))
        .map_err(|error| format!("{error}"))?;
    store
        .append(&entry(
            "signal-2",
            "session-a",
            "intent.pin",
            "1970-01-01T00:20:00Z",
        ))
        .map_err(|error| format!("{error}"))?;
    store
        .append(&entry(
            "signal-3",
            "session-b",
            "intent.find",
            "1970-01-01T00:10:00Z",
        ))
        .map_err(|error| format!("{error}"))?;
    let all = store
        .query(&query("session-a")?)
        .map_err(|error| format!("{error}"))?;
    if all.len() != 2 {
        return Err(String::from("session filter must return two"));
    }
    let window = GovernanceQuery::new(
        String::from(TENANT),
        String::from("session-a"),
        0,
        600_000,
        None,
        None,
    )
    .map_err(|error| format!("{error}"))?;
    if store
        .query(&window)
        .map_err(|error| format!("{error}"))?
        .len()
        != 1
    {
        return Err(String::from("window must narrow to one"));
    }
    let globbed = GovernanceQuery::new(
        String::from(TENANT),
        String::from("session-a"),
        0,
        3_600_000,
        None,
        Some(String::from("intent.*")),
    )
    .map_err(|error| format!("{error}"))?;
    if store
        .query(&globbed)
        .map_err(|error| format!("{error}"))?
        .len()
        != 2
    {
        return Err(String::from("glob must match both"));
    }
    let ml_only = GovernanceQuery::new(
        String::from(TENANT),
        String::from("session-a"),
        0,
        3_600_000,
        Some(AttributionSource::Ml),
        None,
    )
    .map_err(|error| format!("{error}"))?;
    if !store
        .query(&ml_only)
        .map_err(|error| format!("{error}"))?
        .is_empty()
    {
        return Err(String::from("ML filter must exclude widget entries"));
    }
    Ok(())
}

/// Unbounded queries fail at construction: empty tenant, empty session,
/// inverted and over-wide windows.
#[test]
fn unbounded_queries_fail_at_construction() -> Result<(), String> {
    let bounded = |tenant: &str, session: &str, from: u64, to: u64| {
        GovernanceQuery::new(
            String::from(tenant),
            String::from(session),
            from,
            to,
            None,
            None,
        )
    };
    if bounded("tenant", "", 0, 1_000) != Err(GovernanceError::Unbounded) {
        return Err(String::from("empty session must be unbounded"));
    }
    if bounded("", "session", 0, 1_000) != Err(GovernanceError::Unbounded) {
        return Err(String::from("empty tenant must be unbounded"));
    }
    if bounded("tenant", "session", 2_000, 1_000) != Err(GovernanceError::Unbounded) {
        return Err(String::from("inverted window must be unbounded"));
    }
    if bounded("tenant", "session", 0, 90_000_000) != Err(GovernanceError::Unbounded) {
        return Err(String::from("25-hour window must be unbounded"));
    }
    // Exactly 24 hours is bounded.
    bounded("tenant", "session", 0, 86_400_000)
        .map_err(|error| format!("24 hours must be bounded: {error}"))?;
    Ok(())
}

/// An entry with no tenant is malformed, so an unscoped row is never stored.
#[test]
fn append_refuses_an_entry_without_a_tenant() -> Result<(), String> {
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    let unscoped =
        fixtures::governance_entry_in("", "signal-1", "session", "k", "1970-01-01T00:00:00Z");
    if !matches!(
        store.append(&unscoped),
        Err(GovernanceError::MalformedEntry(_))
    ) {
        return Err(String::from("an entry without a tenant must be malformed"));
    }
    if store
        .count(TENANT, "session")
        .map_err(|error| format!("{error}"))?
        != 0
    {
        return Err(String::from("a refused entry must not be stored"));
    }
    Ok(())
}

/// Attribution resolution follows the kernel mapping.
#[test]
fn attribution_resolves() -> Result<(), String> {
    if AttributionSource::resolve("ml") != Ok(AttributionSource::Ml) {
        return Err(String::from("ml must resolve"));
    }
    if AttributionSource::resolve("user") != Ok(AttributionSource::User)
        || AttributionSource::resolve("user_intent") != Ok(AttributionSource::User)
    {
        return Err(String::from("user forms must resolve"));
    }
    if AttributionSource::resolve("widget") != Ok(AttributionSource::Widget) {
        return Err(String::from("widget must resolve"));
    }
    if AttributionSource::resolve("system") != Ok(AttributionSource::System) {
        return Err(String::from("system must resolve"));
    }
    if AttributionSource::resolve("unknown")
        != Err(GovernanceError::UnknownAttribution(String::from("unknown")))
    {
        return Err(String::from("an unknown name must be refused, not system"));
    }
    Ok(())
}

/// A thousand entries round-trip with identical counts and queries.
#[test]
fn thousand_entry_round_trip() -> Result<(), String> {
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    for index in 0..1000 {
        let minute = u64::try_from(index)
            .map_err(|error| format!("index overflow: {error}"))?
            .wrapping_mul(60_000);
        let stamp = format!(
            "1970-01-01T{:02}:{:02}:00Z",
            minute.wrapping_div(3_600_000),
            minute.wrapping_rem(3_600_000).wrapping_div(60_000)
        );
        store
            .append(&entry(
                &format!("signal-{index}"),
                "session",
                "tick",
                &stamp,
            ))
            .map_err(|error| format!("append {index} failed: {error}"))?;
    }
    if store
        .count(TENANT, "session")
        .map_err(|error| format!("{error}"))?
        != 1000
    {
        return Err(String::from("count must be 1000"));
    }
    let found = store
        .query(
            &GovernanceQuery::new(
                String::from(TENANT),
                String::from("session"),
                0,
                86_400_000,
                None,
                None,
            )
            .map_err(|error| format!("{error}"))?,
        )
        .map_err(|error| format!("{error}"))?;
    if found.len() != 1000 {
        return Err(String::from("query must return 1000"));
    }
    Ok(())
}

/// Tape reads scope by session, take the newest suffix, and clamp windows.
#[test]
fn tape_reads_scope_limit_and_clamp() -> Result<(), String> {
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    let reader = TapeReader::new(store.clone());
    let empty: Vec<GovernanceEntrySnapshot> = reader
        .recent_entries(&scope("missing"), 10, 1.0, 3_600_000)
        .map_err(|error| format!("{error}"))?;
    if !empty.is_empty() {
        return Err(String::from("missing session must read empty"));
    }
    for index in 0..5_i32 {
        let stamp = format!("1970-01-01T00:{:02}:00Z", index.wrapping_mul(10));
        store
            .append(&entry(
                &format!("signal-{index}"),
                "session",
                "tick",
                &stamp,
            ))
            .map_err(|error| format!("{error}"))?;
    }
    store
        .append(&entry("signal-x", "other", "tick", "1970-01-01T00:10:00Z"))
        .map_err(|error| format!("{error}"))?;
    let newest: Vec<GovernanceEntrySnapshot> = reader
        .recent_entries(&scope("session"), 3, 1.0, 3_600_000)
        .map_err(|error| format!("{error}"))?;
    if newest.len() != 3 || newest[2].signal_id() != "signal-4" {
        return Err(String::from("limit must take the newest suffix"));
    }
    let all: Vec<GovernanceEntrySnapshot> = reader
        .recent_entries(&scope("session"), 100, 48.0, 3_600_000)
        .map_err(|error| format!("{error}"))?;
    if all.len() != 5 {
        return Err(String::from("48 hours must clamp, not fail"));
    }
    Ok(())
}
