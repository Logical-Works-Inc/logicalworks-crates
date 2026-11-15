//! Seeded tape campaigns for audit fixes 2 and 7: two tenants that share a
//! session id never read each other's tape, and the newest-`limit` read runs
//! inside the store.
//!
//! The model is a plain list of appended rows per tenant, sorted and cut by
//! hand. Every run is replayed: the same seed gives the same trace hash and a
//! distinct seed diverges.

use drishti::native_host::broker::Scope;
use drishti::native_host::governance::{
    GovernanceError, GovernanceQuery, GovernanceStore, MAX_RECENT_ROWS,
};
use drishti::native_host::telemetry::TapeReader;

use crate::broker_scope::replays;
use crate::fixtures::{ensure, governance_entry_in};
use crate::rng::{SimRng, TraceHash};

/// Milliseconds in the first day; the tape clock reads its end.
const DAY_MS: u64 = 86_400_000;
/// The one session id every tenant of the campaign reuses.
const SESSION: &str = "session-shared";

/// One row the model remembers.
struct Row {
    /// Signal id (unique, and it names the tenant that wrote it).
    signal: String,
    /// Record time in milliseconds.
    at_ms: u64,
    /// Signal kind.
    kind: &'static str,
}

/// ISO-8601 stamp of a minute of the first day.
fn stamp(minute: u64) -> String {
    format!(
        "1970-01-01T{:02}:{:02}:00Z",
        minute.wrapping_div(60),
        minute.wrapping_rem(60)
    )
}

/// The signals the model says a read returns: rows at or after `from_ms`
/// (and of `kind` when given), oldest first with ties in append order, cut to
/// the newest `limit`.
fn newest(rows: &[Row], from_ms: u64, kind: Option<&str>, limit: usize) -> Vec<String> {
    let mut kept: Vec<&Row> = rows
        .iter()
        .filter(|row| row.at_ms >= from_ms && kind.is_none_or(|wanted| row.kind == wanted))
        .collect();
    kept.sort_by_key(|row| row.at_ms);
    let skip = kept.len().saturating_sub(limit);
    kept.into_iter()
        .skip(skip)
        .map(|row| row.signal.clone())
        .collect()
}

/// Three tenants append to one session id; seeded reads of every tenant are
/// compared with the model, and no read returns a neighbour's signal.
fn run_tape_tenancy(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    let reader = TapeReader::new(store.clone());
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let tenants = ["tenant-a", "tenant-b", "tenant-c"];
    let mut model: Vec<Vec<Row>> = tenants.iter().map(|_| Vec::new()).collect();
    for step in 0..300_u64 {
        let side = random.below_len(tenants.len());
        let tenant = *tenants
            .get(side)
            .ok_or_else(|| String::from("tenant out of range"))?;
        let rows = model
            .get_mut(side)
            .ok_or_else(|| String::from("tenant out of range"))?;
        if random.below(10) < 7 {
            let minute = random.below(1_440);
            let kind = if random.below(2) == 0 {
                "intent.find"
            } else {
                "tick"
            };
            let signal = format!("{tenant}-{step}");
            let entry = governance_entry_in(tenant, &signal, SESSION, kind, &stamp(minute));
            store
                .append(&entry)
                .map_err(|error| format!("append failed: {error}"))?;
            rows.push(Row {
                signal,
                at_ms: minute.saturating_mul(60_000),
                kind,
            });
            continue;
        }
        let limit = if random.below(10) == 0 {
            usize::MAX
        } else {
            random.below_len(12)
        };
        let (window, from_ms) = if random.below(2) == 0 {
            (24.0, 0)
        } else {
            (1.0, 82_800_000)
        };
        let scope = Scope::new(tenant, SESSION);
        let read = reader
            .recent_entries(&scope, limit, window, DAY_MS)
            .map_err(|error| format!("tape read failed: {error}"))?;
        let got: Vec<String> = read
            .iter()
            .map(|row| String::from(row.signal_id()))
            .collect();
        ensure(got == newest(rows, from_ms, None, limit), || {
            format!("{tenant} read {got:?}, not the newest of its own rows")
        })?;
        ensure(got.iter().all(|signal| signal.starts_with(tenant)), || {
            String::from("a tape read returned another tenant's row")
        })?;
        // A kind glob filters after the fetch, so its limit must not cut first.
        let glob = Some(String::from("intent.*"));
        let query = GovernanceQuery::new(
            String::from(tenant),
            String::from(SESSION),
            0,
            DAY_MS,
            None,
            glob,
        )
        .map_err(|error| format!("{error}"))?;
        let matched = store
            .recent(&query, limit)
            .map_err(|error| format!("{error}"))?;
        let matched: Vec<String> = matched
            .iter()
            .map(|row| String::from(row.signal_id()))
            .collect();
        ensure(
            matched == newest(rows, 0, Some("intent.find"), limit),
            || String::from("a globbed read must keep the newest matches"),
        )?;
        trace.mix_len(got.len());
        trace.mix_len(matched.len());
    }
    // Fools: no tenant is unbounded, an unknown tenant reads nothing.
    let nobody = reader.recent_entries(&Scope::new("", SESSION), 5, 24.0, DAY_MS);
    ensure(nobody == Err(GovernanceError::Unbounded), || {
        String::from("a tapeless tenant name must be unbounded")
    })?;
    let stranger = reader
        .recent_entries(&Scope::new("tenant-z", SESSION), 5, 24.0, DAY_MS)
        .map_err(|error| format!("{error}"))?;
    ensure(stranger.is_empty(), || {
        String::from("an unknown tenant must read an empty tape")
    })?;
    Ok(trace.finish())
}

/// One tenant overfills the tape; a read that asks for everything gets the
/// newest `MAX_RECENT_ROWS` and no more.
fn run_row_bound(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let store = GovernanceStore::open_in_memory().map_err(|error| format!("{error}"))?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let total = MAX_RECENT_ROWS
        .saturating_add(random.below_len(40))
        .saturating_add(1);
    let mut rows = Vec::new();
    for index in 0..total {
        let minute = random.below(1_440);
        let signal = format!("giant-{index}");
        let entry = governance_entry_in("tenant-a", &signal, SESSION, "tick", &stamp(minute));
        store
            .append(&entry)
            .map_err(|error| format!("append failed: {error}"))?;
        rows.push(Row {
            signal,
            at_ms: minute.saturating_mul(60_000),
            kind: "tick",
        });
    }
    let scope = Scope::new("tenant-a", SESSION);
    let read = TapeReader::new(store)
        .recent_entries(&scope, usize::MAX, 24.0, DAY_MS)
        .map_err(|error| format!("{error}"))?;
    let got: Vec<String> = read
        .iter()
        .map(|row| String::from(row.signal_id()))
        .collect();
    ensure(got == newest(&rows, 0, None, MAX_RECENT_ROWS), || {
        String::from("a read for everything must return the newest bounded rows")
    })?;
    trace.mix_len(got.len());
    Ok(trace.finish())
}

/// Giant-and-empty. Bug caught: a tape read that builds every row of the
/// window in memory (or returns it all) however large the tape grows.
#[test]
fn sim_tape_giant_read_is_capped_at_the_row_bound() -> Result<(), String> {
    replays(run_row_bound, [1, 9, 101], 2)
}

/// Neighbour. Bug caught: a tape read keyed on the session id alone (issue
/// drishti#8), so tenant B reads tenant A's tape when both reuse a session id; and a
/// limit applied after the whole window was fetched.
#[test]
fn sim_tape_neighbour_same_session_id_never_crosses_tenants() -> Result<(), String> {
    replays(run_tape_tenancy, [4, 23, 877], 5)
}
