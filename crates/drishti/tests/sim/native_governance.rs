//! Governance-log sims: a random operation stream is run against an in-memory
//! model, tenants reuse session and signal ids on purpose, windows are driven
//! to their edges, and a file-backed log is crashed, torn and filled with
//! garbage; every failure must be typed and no row may be invented.

use std::path::{Path, PathBuf};

use drishti::native_host::governance::{
    AttributionSource, GovernanceError, GovernanceQuery, GovernanceStore, MAX_RECENT_ROWS,
};
use drishti::native_host::protocol::{Attribution, AttributionKind, GovernanceEntry};

use crate::fixtures::{WORK_OBJECT, governance_entry_in};
use crate::motion_kit::{describe, ensure, pick};
use crate::rng::SimRng;

/// Milliseconds in a day.
const DAY_MS: u64 = 86_400_000;

/// ISO stamp of a minute counted from the epoch (three days at most).
pub(crate) fn stamp(minute: u64) -> String {
    format!(
        "1970-01-{:02}T{:02}:{:02}:00Z",
        minute.wrapping_div(1_440).saturating_add(1),
        minute.wrapping_rem(1_440).wrapping_div(60),
        minute.wrapping_rem(60)
    )
}

/// An entry with an explicit attribution kind.
fn attributed(
    tenant: &str,
    session: &str,
    signal: &str,
    kind: &str,
    at: &str,
    source: AttributionKind,
) -> GovernanceEntry {
    let mut entry = GovernanceEntry::new(
        String::from(session),
        format!("entry-{signal}"),
        String::from(WORK_OBJECT),
        String::from(signal),
        String::from(kind),
        String::from("sim"),
        Attribution::new(source, String::from("source"), None, None),
    );
    entry.set_recorded_at(String::from(at));
    entry.set_tenant_id(String::from(tenant));
    entry
}

/// One row the model remembers.
struct Row {
    /// Owning tenant.
    tenant: String,
    /// Session within the tenant.
    session: String,
    /// Signal id, unique within the tenant.
    signal: String,
    /// Event kind.
    kind: &'static str,
    /// Record time in milliseconds.
    at_ms: u64,
    /// Stored attribution.
    source: AttributionKind,
}

/// The documented raw name of an attribution kind.
fn raw_name(source: AttributionKind) -> &'static str {
    match source {
        AttributionKind::User => "user",
        AttributionKind::Widget => "widget",
        AttributionKind::Ml => "ml",
        AttributionKind::System => "system",
        _ => "system",
    }
}

/// The reference kind filter for the four globs the sim uses.
fn glob_says(glob: &str, kind: &str) -> bool {
    match glob.strip_suffix('*') {
        Some(prefix) => kind.starts_with(prefix),
        None => glob == kind,
    }
}

/// The kind globs the read sim draws from.
const GLOBS: &[&str] = &["intent.*", "tick", "*", "forms.*"];

/// Read one seeded window of one tenant session through `query` and `recent`,
/// and compare both with the plain row model.
fn check_read(
    store: &GovernanceStore,
    rows: &[Row],
    random: &mut SimRng,
    scope: (&str, &str),
    at: (u64, usize),
) -> Result<(), String> {
    let (tenant, session) = scope;
    let (seed, step) = at;
    let from = random.below(DAY_MS.saturating_mul(2));
    let width = random.below(DAY_MS);
    let to = from.saturating_add(width);
    let filter = if random.below(3) == 0 {
        Some(match random.below(4) {
            0 => AttributionSource::User,
            1 => AttributionSource::Widget,
            2 => AttributionSource::Ml,
            _ => AttributionSource::System,
        })
    } else {
        None
    };
    let glob = if random.below(3) == 0 {
        Some(String::from(pick(random, GLOBS)?))
    } else {
        None
    };
    let query = GovernanceQuery::new(
        String::from(tenant),
        String::from(session),
        from,
        to,
        filter,
        glob.clone(),
    )
    .map_err(describe)?;
    let mut kept: Vec<&Row> = rows
        .iter()
        .filter(|row| {
            row.tenant == tenant
                && row.session == session
                && row.at_ms >= from
                && row.at_ms <= to
                && filter.is_none_or(|wanted| {
                    AttributionSource::resolve(raw_name(row.source)) == Ok(wanted)
                })
                && glob.as_deref().is_none_or(|text| glob_says(text, row.kind))
        })
        .collect();
    kept.sort_by_key(|row| row.at_ms);
    let all: Vec<String> = kept.iter().map(|row| row.signal.clone()).collect();
    let got: Vec<String> = store
        .query(&query)
        .map_err(describe)?
        .iter()
        .map(|row| String::from(row.signal_id()))
        .collect();
    ensure(
        got == all,
        &format!("seed {seed} step {step}: query read {got:?}, model {all:?}"),
    )?;
    let limit = if random.below(8) == 0 {
        usize::MAX
    } else {
        random.below_len(12)
    };
    let cut = all.len().saturating_sub(limit.min(MAX_RECENT_ROWS));
    let newest: Vec<String> = all.iter().skip(cut).cloned().collect();
    let recent: Vec<String> = store
        .recent(&query, limit)
        .map_err(describe)?
        .iter()
        .map(|row| String::from(row.signal_id()))
        .collect();
    ensure(
        recent == newest,
        &format!("seed {seed} step {step}: recent({limit}) read {recent:?}, model {newest:?}"),
    )
}

/// Bug caught: a read that leaks a neighbour tenant's row, mis-orders ties,
/// cuts the limit before the kind filter, or disagrees with a plain list
/// about which rows a window holds.
#[test]
fn sim_governance_matches_the_model_over_random_operations() -> Result<(), String> {
    const TENANTS: &[&str] = &["tenant-a", "tenant-b", "tenant-c"];
    const SESSIONS: &[&str] = &["session-1", "session-2"];
    const KINDS: &[&str] = &["intent.find", "intent.pin", "forms.submitted", "tick"];
    const SOURCES: [AttributionKind; 4] = [
        AttributionKind::User,
        AttributionKind::Widget,
        AttributionKind::Ml,
        AttributionKind::System,
    ];
    for seed in 0..25_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(300_000));
        let store = GovernanceStore::open_in_memory().map_err(describe)?;
        let mut rows: Vec<Row> = Vec::new();
        let mut duplicates = 0_usize;
        let mut reads = 0_usize;
        for step in 0..300_usize {
            let tenant = pick(&mut random, TENANTS)?;
            let session = pick(&mut random, SESSIONS)?;
            if random.below(10) < 6 {
                let signal = format!("signal-{}", random.below(40));
                let kind = pick(&mut random, KINDS)?;
                let minute = random.below(4_320);
                let source = pick(&mut random, &SOURCES)?;
                let entry = attributed(tenant, session, &signal, kind, &stamp(minute), source);
                let taken = rows
                    .iter()
                    .any(|row| row.tenant == tenant && row.signal == signal);
                let outcome = store.append(&entry);
                if taken {
                    duplicates = duplicates.saturating_add(1);
                    ensure(
                        outcome == Err(GovernanceError::Duplicate(signal)),
                        &format!("seed {seed} step {step}: a repeat gave {outcome:?}"),
                    )?;
                } else {
                    ensure(
                        outcome == Ok(()),
                        &format!("seed {seed} step {step}: a fresh row gave {outcome:?}"),
                    )?;
                    rows.push(Row {
                        tenant: String::from(tenant),
                        session: String::from(session),
                        signal,
                        kind,
                        at_ms: minute.saturating_mul(60_000),
                        source,
                    });
                }
                continue;
            }
            reads = reads.saturating_add(1);
            check_read(&store, &rows, &mut random, (tenant, session), (seed, step))?;
        }
        ensure(
            duplicates > 0 && reads > 20,
            &format!("seed {seed}: the run must repeat ids and read often"),
        )?;
    }
    Ok(())
}

/// Bug caught: a signal id that is unique across the whole log instead of
/// within a tenant, so tenant B is refused (and learns that tenant A holds
/// that id) or, worse, a hostile tenant pre-claims a victim's id and the
/// victim's audit append fails.
#[test]
fn sim_governance_signal_ids_are_scoped_to_the_tenant() -> Result<(), String> {
    let store = GovernanceStore::open_in_memory().map_err(describe)?;
    for seed in 0..40_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(310_000));
        let signal = format!("shared-{seed}");
        let attacker = governance_entry_in(
            "tenant-attacker",
            &signal,
            "session",
            "tick",
            &stamp(random.below(100)),
        );
        let victim = governance_entry_in(
            "tenant-victim",
            &signal,
            "session",
            "tick",
            &stamp(random.below(100)),
        );
        ensure(
            store.append(&attacker) == Ok(()),
            "the first writer must be accepted",
        )?;
        ensure(
            store.append(&victim) == Ok(()),
            &format!("seed {seed}: a second tenant's id was refused or leaked as a duplicate"),
        )?;
        ensure(
            store.append(&victim) == Err(GovernanceError::Duplicate(signal)),
            "a repeat inside one tenant is still a duplicate",
        )?;
    }
    for tenant in ["tenant-attacker", "tenant-victim"] {
        let query = GovernanceQuery::new(
            String::from(tenant),
            String::from("session"),
            0,
            DAY_MS,
            None,
            None,
        )
        .map_err(describe)?;
        ensure(
            store.query(&query).map_err(describe)?.len() == 40,
            &format!("{tenant} must read exactly its own 40 rows"),
        )?;
    }
    Ok(())
}

/// Bug caught: a window check off by one at 24 hours or at either inclusive
/// edge, a zero-width window mishandled, or an empty scope accepted.
#[test]
fn sim_governance_window_edges_are_inclusive_and_bounded() -> Result<(), String> {
    let store = GovernanceStore::open_in_memory().map_err(describe)?;
    for (index, minute) in [0_u64, 59, 60, 61, 1_439, 1_440].into_iter().enumerate() {
        let entry = governance_entry_in(
            "tenant-w",
            &format!("edge-{index}"),
            "session",
            "tick",
            &stamp(minute),
        );
        store.append(&entry).map_err(describe)?;
    }
    let read = |from: u64, to: u64| -> Result<Vec<String>, String> {
        let query = GovernanceQuery::new(
            String::from("tenant-w"),
            String::from("session"),
            from,
            to,
            None,
            None,
        )
        .map_err(describe)?;
        Ok(store
            .query(&query)
            .map_err(describe)?
            .iter()
            .map(|row| String::from(row.signal_id()))
            .collect())
    };
    ensure(
        read(59 * 60_000, 60 * 60_000)? == ["edge-1", "edge-2"],
        "both edges of a window are inclusive",
    )?;
    ensure(
        read(60 * 60_000, 60 * 60_000)? == ["edge-2"],
        "a zero-width window holds the rows at that instant",
    )?;
    ensure(
        read(62 * 60_000, 1_438 * 60_000)?.is_empty(),
        "a window between rows is empty",
    )?;
    ensure(
        read(0, DAY_MS)?.len() == 6,
        "a 24 hour window holds the whole first day",
    )?;
    for (tenant, session, from, to, bounded) in [
        ("t", "s", 0, DAY_MS, true),
        ("t", "s", 0, DAY_MS.saturating_add(1), false),
        ("t", "s", 1, 0, false),
        ("t", "s", u64::MAX, 0, false),
        ("", "s", 0, 1, false),
        ("t", "", 0, 1, false),
        ("", "", 0, 0, false),
    ] {
        let built = GovernanceQuery::new(
            String::from(tenant),
            String::from(session),
            from,
            to,
            None,
            None,
        );
        ensure(
            built.is_ok() == bounded,
            &format!(
                "({tenant:?}, {session:?}, {from}, {to}): verdict {:?}",
                built.is_ok()
            ),
        )?;
        if !bounded {
            ensure(
                built == Err(GovernanceError::Unbounded),
                "an unbounded query is the typed refusal",
            )?;
        }
    }
    let wild = GovernanceQuery::new(
        String::from("tenant-w"),
        String::from("session"),
        u64::MAX,
        u64::MAX,
        None,
        None,
    )
    .map_err(describe)?;
    ensure(
        store.query(&wild) == Ok(Vec::new()),
        "an absurd instant holds no rows and is not a failure",
    )?;
    Ok(())
}

/// Bug caught: an append that stores an entry the log cannot place (no
/// signal, no tenant, a stamp it cannot read), or that blames the wrong
/// field when several are wrong.
#[test]
fn sim_governance_append_refusals_are_typed_and_ordered() -> Result<(), String> {
    let store = GovernanceStore::open_in_memory().map_err(describe)?;
    let stamps = [
        "",
        "not a stamp",
        "2026-13-01T00:00:00Z",
        "2026-02-30T00:00:00Z",
        "1969-12-31T23:59:59Z",
        "2026-11-03T00:00:00",
        "2026-11-03T00:00:00\u{200b}Z",
    ];
    for (index, at) in stamps.into_iter().enumerate() {
        let bad_time = governance_entry_in("tenant-g", &format!("s-{index}"), "session", "k", at);
        ensure(
            matches!(
                store.append(&bad_time),
                Err(GovernanceError::MalformedEntry(_))
            ),
            &format!("{at:?}: an unreadable stamp must be malformed"),
        )?;
        let no_tenant = governance_entry_in("", &format!("t-{index}"), "session", "k", at);
        ensure(
            matches!(
                store.append(&no_tenant),
                Err(GovernanceError::MalformedEntry(_))
            ),
            "no tenant must be malformed whatever the stamp",
        )?;
        let no_signal = governance_entry_in("tenant-g", "", "session", "k", at);
        ensure(
            matches!(
                store.append(&no_signal),
                Err(GovernanceError::MalformedEntry(_))
            ),
            "no signal id must be malformed whatever the rest",
        )?;
    }
    let first = store.append(&governance_entry_in("", "", "session", "k", "bad"));
    let second = store.append(&governance_entry_in("", "x", "session", "k", "bad"));
    let third = store.append(&governance_entry_in("tenant-g", "x", "session", "k", "bad"));
    let reasons = [first, second, third].map(|outcome| match outcome {
        Err(GovernanceError::MalformedEntry(reason)) => reason,
        other => format!("{other:?}"),
    });
    let [by_signal, by_tenant, by_stamp] = reasons.each_ref();
    ensure(
        by_signal.contains("signalId")
            && by_tenant.contains("tenantId")
            && by_stamp.contains("recordedAt"),
        &format!("the first defect must be named: {reasons:?}"),
    )?;
    ensure(
        store.count("tenant-g", "session").map_err(describe)? == 0,
        "no refused entry may be stored",
    )?;
    for good in [
        "2026-11-03T00:00:00Z",
        "2026-11-03T05:00:00+05:00",
        "2026-11-03T00:00:00.999Z",
    ] {
        store
            .append(&governance_entry_in("tenant-g", good, "session", "k", good))
            .map_err(describe)?;
    }
    Ok(())
}

/// Bug caught: a filter that returns another source's rows (name resolution
/// itself is judged in `native_governance_scope`).
#[test]
fn sim_governance_attribution_filter_returns_only_its_source() -> Result<(), String> {
    let store = GovernanceStore::open_in_memory().map_err(describe)?;
    let kinds = [
        AttributionKind::User,
        AttributionKind::Widget,
        AttributionKind::Ml,
        AttributionKind::System,
    ];
    for (index, kind) in kinds.into_iter().enumerate() {
        for copy in 0..3_usize {
            let signal = format!("a-{index}-{copy}");
            store
                .append(&attributed(
                    "tenant-x",
                    "session",
                    &signal,
                    "tick",
                    &stamp(1),
                    kind,
                ))
                .map_err(describe)?;
        }
    }
    for source in [
        AttributionSource::User,
        AttributionSource::Widget,
        AttributionSource::Ml,
        AttributionSource::System,
    ] {
        let query = GovernanceQuery::new(
            String::from("tenant-x"),
            String::from("session"),
            0,
            DAY_MS,
            Some(source),
            None,
        )
        .map_err(describe)?;
        let rows = store.query(&query).map_err(describe)?;
        ensure(
            rows.len() == 3
                && rows
                    .iter()
                    .all(|row| row.attribution() == source.raw_value()),
            &format!("{source:?}: the filter must return exactly its own three rows"),
        )?;
    }
    Ok(())
}

/// A unique scratch database path for one sim step.
pub(crate) fn scratch(label: &str, seed: u64) -> PathBuf {
    std::env::temp_dir().join(format!(
        "drishti-sim-gov-{label}-{seed}-{}.db",
        lgwks_std::id::Uuid::now_v7()
    ))
}

/// Remove a database and its journal files.
pub(crate) fn cleanup(base: &Path) -> Result<(), String> {
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let mut name = base.as_os_str().to_os_string();
        name.push(suffix);
        let path = PathBuf::from(name);
        if path.exists() {
            std::fs::remove_file(&path).map_err(describe)?;
        }
    }
    Ok(())
}

/// Every row of the log, oldest first.
fn dump(store: &GovernanceStore) -> Result<Vec<String>, GovernanceError> {
    let query = GovernanceQuery::new(
        String::from("tenant-f"),
        String::from("session"),
        0,
        DAY_MS,
        None,
        None,
    )?;
    Ok(store
        .query(&query)?
        .iter()
        .map(|row| {
            format!(
                "{}|{}|{}|{}|{}",
                row.signal_id(),
                row.kind(),
                row.timestamp_ms(),
                row.recorded_at(),
                row.attribution()
            )
        })
        .collect())
}

/// Bug caught: a log that loses or reorders committed rows across a restart,
/// or that accepts a replay of an entry it already holds as new.
#[test]
fn sim_governance_file_store_replays_to_the_same_state() -> Result<(), String> {
    for seed in 0..4_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(320_000));
        let path = scratch("replay", seed);
        cleanup(&path)?;
        let before = {
            let store = GovernanceStore::open(&path).map_err(describe)?;
            for index in 0..12_u64 {
                let entry = governance_entry_in(
                    "tenant-f",
                    &format!("s-{index}"),
                    "session",
                    "tick",
                    &stamp(random.below(1_000)),
                );
                store.append(&entry).map_err(describe)?;
            }
            dump(&store).map_err(describe)?
        };
        let reopened = GovernanceStore::open(&path).map_err(describe)?;
        let after = dump(&reopened).map_err(describe)?;
        ensure(
            before.len() == 12 && before == after,
            &format!("seed {seed}: the reopened log differs from the committed one"),
        )?;
        let replay = governance_entry_in(
            "tenant-f",
            "s-3",
            "session",
            "tick",
            &stamp(random.below(1_000)),
        );
        ensure(
            reopened.append(&replay) == Err(GovernanceError::Duplicate(String::from("s-3"))),
            &format!("seed {seed}: a replayed entry was taken as new"),
        )?;
        ensure(
            reopened.count("tenant-f", "session").map_err(describe)? == 12,
            "a refused replay must not grow the log",
        )?;
        cleanup(&path)?;
    }
    Ok(())
}

/// Bug caught: a damaged file that panics the host, reads back a row that was
/// never written, or loses its typed error (the pulled-plug and torn-write
/// cases, and a file that is not a database at all).
#[test]
fn sim_governance_torn_and_garbage_files_fail_typed() -> Result<(), String> {
    for seed in 0..6_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(330_000));
        let path = scratch("torn", seed);
        cleanup(&path)?;
        let original = {
            let store = GovernanceStore::open(&path).map_err(describe)?;
            for index in 0..30_u64 {
                let entry = governance_entry_in(
                    "tenant-f",
                    &format!("s-{index}"),
                    "session",
                    "tick",
                    &stamp(index),
                );
                store.append(&entry).map_err(describe)?;
            }
            dump(&store).map_err(describe)?
        };
        let bytes = std::fs::read(&path).map_err(describe)?;
        ensure(bytes.len() > 4_096, "the file must span several pages")?;
        let cut = random.below_len(bytes.len());
        let kept = bytes.get(..cut).ok_or("the cut is past the file")?;
        std::fs::write(&path, kept).map_err(describe)?;
        match GovernanceStore::open(&path) {
            Err(error) => ensure(
                matches!(error, GovernanceError::Store(_)),
                &format!("seed {seed}: a torn file gave {error:?}"),
            )?,
            Ok(store) => {
                match dump(&store) {
                    Ok(rows) => ensure(
                        rows.iter().all(|row| original.contains(row)),
                        &format!("seed {seed}: a torn file read back a row never written"),
                    )?,
                    Err(error) => ensure(
                        matches!(error, GovernanceError::Store(_)),
                        &format!("seed {seed}: a torn read gave {error:?}"),
                    )?,
                }
                let again =
                    governance_entry_in("tenant-f", "after-tear", "session", "k", &stamp(9));
                match store.append(&again) {
                    Ok(()) => {}
                    Err(error) => ensure(
                        matches!(
                            error,
                            GovernanceError::Store(_) | GovernanceError::Duplicate(_)
                        ),
                        &format!("seed {seed}: an append to a torn file gave {error:?}"),
                    )?,
                }
            }
        }
        cleanup(&path)?;
        let junk_length = random.below_len(6_000).saturating_add(1_024);
        let junk: Vec<u8> = (0..junk_length)
            .map(|_| random.next_u64().to_le_bytes()[0])
            .collect();
        std::fs::write(&path, &junk).map_err(describe)?;
        let outcome = GovernanceStore::open(&path);
        ensure(
            matches!(outcome, Err(GovernanceError::Store(_))),
            &format!("seed {seed}: {junk_length} bytes of junk opened as a log"),
        )?;
        cleanup(&path)?;
    }
    Ok(())
}
