//! The shipped `edge-load --rig`: one row per surface, connection count and
//! round, each naming the guarantees its surface held (#1007).
//!
//! Source: #1007's proof. A short run of the shipped binary drives every
//! surface through its server mode and its separate driver process, and the
//! rows it stored are read back from the SQLite file with the `sqlite3`
//! program, the reader the rig wrote them through. The host label carries a
//! quote and a space, which a row writer that did not quote its SQL would end
//! its string on. The driver is held to the servers it drives: what it sent
//! is what the world settled, a keyed tenant opens only to `hit-keyed`, and
//! the counts its posts leave are what `rt::serve` and axum answer.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};

use crate::scratch::Scratch;
use crate::sim::piped::{Outcome, located};

/// Every surface the rig names, with isolation, log first, idempotency and durability.
const SURFACES: [(&str, [&str; 4]); 5] = [
    ("app", ["1", "0", "0", "0"]),
    ("axum", ["0", "0", "0", "0"]),
    ("serve", ["1", "1", "1", "0"]),
    ("serve-durable", ["1", "1", "1", "1"]),
    ("world", ["1", "0", "0", "0"]),
];

/// The rig binary, as nextest or cargo built it.
fn rig() -> Command {
    Command::new(located(
        ["NEXTEST_BIN_EXE_edge-load", "NEXTEST_BIN_EXE_edge_load"],
        env!("CARGO_BIN_EXE_edge-load"),
    ))
}

/// A short rig with `extra` options into `db`; what it printed, once it succeeded.
fn short_rig(extra: &[&str], db: &Path) -> Outcome<String> {
    let ran = rig()
        .args(["--rig", "--short"])
        .args(extra)
        .arg("--db")
        .arg(db)
        .output()?;
    let printed = String::from_utf8(ran.stdout)?;
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(
        ran.status.success(),
        "the short rig failed: {said}\n{printed}"
    );
    Ok(printed)
}

/// The rows `sql` selects from `db`, read with the `sqlite3` program, cells split on `|`.
fn stored(db: &Path, sql: &str) -> Outcome<Vec<Vec<String>>> {
    let read = Command::new("sqlite3")
        .args(["-separator", "|"])
        .arg(db)
        .arg(sql)
        .output()?;
    let said = String::from_utf8_lossy(&read.stderr);
    assert!(read.status.success(), "sqlite3 refused the read: {said}");
    Ok(String::from_utf8(read.stdout)?
        .lines()
        .map(|line| line.split('|').map(str::to_owned).collect())
        .collect())
}

/// One surface and connection count's stored `rps`: round 0 and the warm rounds.
#[derive(Default)]
struct Rounds {
    /// Round 0's.
    cold: Option<u64>,
    /// Rounds 1 to 3.
    warm: Vec<u64>,
}

/// The printed table against the stored rows: one line per surface and
/// connection count, whose `rps` is the median warm round by `rps` and whose
/// `cold_rps` is round 0's.
fn table_matches_rows(printed: &str, rows: &[Vec<String>]) -> Outcome {
    let mut rounds: HashMap<(&str, &str), Rounds> = HashMap::new();
    for row in rows {
        let cell = |at: usize| row.get(at).map(String::as_str).ok_or("a short row");
        let (surface, conns, round, rps) = (cell(0)?, cell(5)?, cell(6)?, cell(12)?);
        let slot = rounds.entry((surface, conns)).or_default();
        let rps: u64 = rps.parse()?;
        if round == "0" {
            slot.cold = Some(rps);
        } else {
            slot.warm.push(rps);
        }
    }
    let lines: Vec<Vec<&str>> = printed
        .lines()
        .skip(2)
        .map(|line| line.split_whitespace().collect())
        .collect();
    assert_eq!(
        lines.len(),
        5 * 2,
        "one table line per surface and connection count: {printed}"
    );
    for line in &lines {
        let word = |at: usize| line.get(at).copied().ok_or("a short table line");
        let (surface, conns, rps, cold) = (word(0)?, word(1)?, word(2)?, word(3)?);
        let backed = rounds
            .get_mut(&(surface, conns))
            .ok_or("a table line no row backs")?;
        backed.warm.sort_unstable();
        let stored_cold = backed.cold;
        let median = backed
            .warm
            .get(backed.warm.len().div_euclid(2))
            .ok_or("no warm round")?;
        assert_eq!(
            rps.parse::<u64>()?,
            *median,
            "{surface} at {conns}: the warm median"
        );
        assert_eq!(
            Some(cold.parse::<u64>()?),
            stored_cold,
            "{surface} at {conns}: round 0"
        );
    }
    for (surface, _) in SURFACES {
        let lines_for = lines
            .iter()
            .filter(|line| line.first() == Some(&surface))
            .count();
        assert_eq!(
            lines_for, 2,
            "the table names {surface} at both counts: {printed}"
        );
    }
    Ok(())
}

/// Catches a label check that lets an empty, overlong or unprintable label
/// reach a row.
#[test]
fn rig_labels_outside_printable_ascii_are_refused() -> Outcome {
    let scratch = Scratch::new("edge-rig-labels")?;
    let db = scratch.path().join("rig.sqlite");
    for host in [String::new(), "h".repeat(65), String::from("a\tb")] {
        let ran = rig()
            .args(["--rig", "--short", "--host", &host, "--db"])
            .arg(&db)
            .output()?;
        let said = String::from_utf8_lossy(&ran.stderr);
        assert!(
            !ran.status.success() && said.contains("usage"),
            "host {host:?} is refused: {said}"
        );
        assert!(!db.exists(), "a refused label stored nothing");
    }
    Ok(())
}

/// Catches a surface, a guarantee, a round or a measured column missing from
/// the stored rows, or a label that breaks the row writer's SQL.
#[test]
fn the_rig_table_names_every_surface_and_guarantee() -> Outcome {
    let scratch = Scratch::new("edge-rig")?;
    let db = scratch.path().join("rig.sqlite");
    // The longest label a row takes, 64 bytes.
    let commit = "c".repeat(64);
    let printed = short_rig(&["--host", "it's ci", "--commit", &commit], &db)?;
    let rows = stored(
        &db,
        "SELECT surface, isolation, log_first, idempotency, durability, conns, round, host, \
         profile, sent, ok, errors, rps, p50_us <= p99_us, p99_us <= max_us, rss_bytes > 0, \
         cpu_us >= 0, tenants, read_permille, commit_sha FROM rig_row \
         ORDER BY surface, conns, round",
    )?;
    table_matches_rows(&printed, &rows)?;
    assert_eq!(
        rows.len(),
        5 * 2 * 4,
        "five surfaces, two connection counts, a cold round and three warm ones"
    );
    let mut at = rows.iter();
    for (surface, held) in SURFACES {
        for conns in ["16", "64"] {
            for round in ["0", "1", "2", "3"] {
                let row = at.next().ok_or("a row is missing")?;
                let cells: Vec<&str> = row.iter().map(String::as_str).collect();
                let [
                    name,
                    isolation,
                    log_first,
                    idempotency,
                    durability,
                    stored_conns,
                    stored_round,
                    host,
                    profile,
                    sent,
                    ok,
                    errors,
                    rps,
                    ordered,
                    capped,
                    resident,
                    spent,
                    tenants,
                    share,
                    stored_commit,
                ] = *cells.as_slice()
                else {
                    return Err(format!("a row has the wrong width: {row:?}").into());
                };
                assert_eq!((name, stored_conns, stored_round), (surface, conns, round));
                // The warm-up is 64 requests, a warm round 512, cut evenly over the connections.
                let planned = if round == "0" { "64" } else { "512" };
                assert_eq!(sent, planned, "{surface} round {round} sent its share");
                assert_eq!(stored_commit, commit, "a 64-byte label is stored whole");
                assert_eq!(
                    [isolation, log_first, idempotency, durability],
                    held,
                    "{surface}'s guarantees"
                );
                assert_eq!(
                    (host, profile),
                    ("it's ci", "debug"),
                    "the label survived its quote"
                );
                assert_eq!(
                    (ok, errors),
                    (sent, "0"),
                    "{surface} answered every request"
                );
                assert_ne!(rps, "0", "{surface} at {conns} measured a rate");
                assert_eq!(
                    [ordered, capped, resident, spent],
                    ["1"; 4],
                    "{surface}'s columns hold"
                );
                assert_eq!((tenants, share), ("100", "800"));
            }
        }
    }
    Ok(())
}

/// The `key=value` numbers of one driver or world line, units stripped.
fn numbers(line: &str) -> HashMap<String, u64> {
    line.split_whitespace()
        .filter_map(|word| {
            let (key, value) = word.split_once('=')?;
            let digits = value.trim_end_matches(|ch: char| ch.is_ascii_alphabetic());
            Some((key.to_owned(), digits.parse().ok()?))
        })
        .collect()
}

/// The number `key` names in `line`.
fn at(line: &HashMap<String, u64>, key: &str) -> Outcome<u64> {
    Ok(*line.get(key).ok_or(format!("no {key} in {line:?}"))?)
}

/// A server mode of the shipped binary, its stdout past the address, and the address.
fn serving(mode: &str) -> Outcome<(Child, BufReader<ChildStdout>, String)> {
    let mut server = rig()
        .arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut lines = BufReader::new(server.stdout.take().ok_or("the server has no stdout")?);
    let mut first = String::new();
    lines.read_line(&mut first)?;
    let addr = first
        .trim()
        .strip_prefix("listening ")
        .ok_or("no address")?;
    let addr = addr.to_owned();
    Ok((server, lines, addr))
}

/// One client mode (`drive`, `hit`, `hit-keyed`) at `addr`; its numbers, once it succeeded.
fn client(mode: &str, addr: &str, words: &[&str]) -> Outcome<HashMap<String, u64>> {
    let ran = rig().arg(mode).arg(addr).args(words).output()?;
    let line = String::from_utf8(ran.stdout)?;
    assert!(ran.status.success(), "{mode} {words:?} failed: {line}");
    Ok(numbers(&line))
}

/// The body a plain `GET` of `tenant`'s count answers, under its own key.
fn count_at(addr: &str, tenant: &str, key: &str) -> Outcome<String> {
    let mut wire = TcpStream::connect(addr)?;
    let request = format!(
        "GET /t/{tenant}/count HTTP/1.1\r\nhost: rig\r\nidempotency-key: {key}\r\nconnection: close\r\ncontent-length: 0\r\n\r\n"
    );
    wire.write_all(request.as_bytes())?;
    let mut answer = Vec::new();
    wire.read_to_end(&mut answer)?;
    let text = String::from_utf8(answer)?;
    let (_, body) = text
        .split_once("\r\n\r\n")
        .ok_or("an answer with no head")?;
    Ok(body.to_owned())
}

/// Catches a client that shows a key it should not, or hides one it should:
/// a keyed tenant answers only `hit-keyed`, and `hit` and `drive` are refused.
#[test]
fn drive_hit_and_hit_keyed_meet_a_keyed_tenant() -> Outcome {
    let (mut server, _lines, addr) = serving("app-keyed")?;
    let met = (|| -> Outcome<[HashMap<String, u64>; 3]> {
        Ok([
            client("hit", &addr, &["2", "5", "3"])?,
            client("hit-keyed", &addr, &["2", "5", "3"])?,
            client("drive", &addr, &["2", "5", "3", "0", "7"])?,
        ])
    })();
    drop(server.stdin.take());
    server.wait()?;
    let [plain, keyed, driven] = met?;
    assert_eq!(
        (at(&plain, "ok")?, at(&plain, "errors")?),
        (0, 10),
        "hit shows no key"
    );
    assert_eq!(
        (at(&keyed, "ok")?, at(&keyed, "errors")?),
        (10, 0),
        "hit-keyed opens each tenant"
    );
    assert_eq!(
        (at(&driven, "ok")?, at(&driven, "errors")?),
        (0, 10),
        "drive shows no key"
    );
    Ok(())
}

/// Catches a storage owner that holds a blocking-pool thread for its store's
/// life (#1041). `rt::serve` opens one store per tenant and hops every request
/// to that pool, whose ceiling is 512, so 600 tenants left every pool thread an
/// owner and the next request queued for ever. Source: the 600-tenant drive in
/// #1041, which answered nothing in 25 s.
#[test]
fn one_store_per_tenant_past_the_pool_ceiling_still_answers_each_batch() -> Outcome {
    let (mut server, _lines, addr) = serving("serve")?;
    let driven = client("drive", &addr, &["1", "600", "600", "0", "7"]);
    drop(server.stdin.take());
    server.wait()?;
    let driven = driven?;
    assert_eq!(
        (at(&driven, "ok")?, at(&driven, "errors")?),
        (600, 0),
        "every one of 600 tenants answered its first write"
    );
    Ok(())
}

/// Catches a counter that counts a read or drops a write, on `rt::serve` and
/// on axum: three posts and two reads leave the count at three.
#[test]
fn drive_counts_land_on_serve_and_axum() -> Outcome {
    for mode in ["serve", "axum"] {
        let (mut server, _lines, addr) = serving(mode)?;
        let counted = (|| -> Outcome<String> {
            client("drive", &addr, &["1", "3", "1", "0", "11"])?;
            client("drive", &addr, &["1", "2", "1", "1000", "12"])?;
            count_at(&addr, "tenant-0", "rig-count-check")
        })();
        drop(server.stdin.take());
        server.wait()?;
        assert_eq!(counted?, "3", "{mode}: three posts, two reads");
    }
    Ok(())
}

/// Catches a driver whose seeded mix drifts between runs, whose read share or
/// tenant spread is off, whose counts disagree with what the server settled,
/// or whose usage check lets a bad word through.
#[test]
fn drive_replays_its_seeded_mix_against_the_worlds_edge() -> Outcome {
    let (mut server, mut lines, addr) = serving("world")?;
    let driven = (|| -> Outcome<Vec<HashMap<String, u64>>> {
        let mut seen = Vec::new();
        // 5,000 posts at share 0: a draw of 0 read as `<=` would turn some into reads.
        let mixes: [&[&str]; 4] = [
            &["4", "50", "10", "800", "7"],
            &["4", "50", "10", "800", "7"],
            &["2", "2500", "10", "0", "7"],
            &["2", "25", "10", "1000", "7"],
        ];
        for words in mixes {
            seen.push(client("drive", &addr, words)?);
        }
        for refused in [
            ["0", "50", "10", "800", "7"],
            ["4", "50", "10", "1001", "7"],
            ["4", "50", "0", "800", "7"],
            ["4", "50", "10", "800", "seven"],
        ] {
            let ran = rig().arg("drive").arg(&addr).args(refused).output()?;
            let said = String::from_utf8_lossy(&ran.stderr);
            assert!(
                !ran.status.success() && said.contains("usage"),
                "drive {refused:?} is refused: {said}"
            );
        }
        Ok(seen)
    })();
    drop(server.stdin.take());
    let mut record = String::new();
    lines.read_line(&mut record)?;
    server.wait()?;
    let seen = driven?;
    let (Some(mixed), Some(again), Some(posts), Some(gets)) =
        (seen.first(), seen.get(1), seen.get(2), seen.get(3))
    else {
        return Err("four driver runs".into());
    };
    for (key, sent) in [("sent", 200), ("ok", 200), ("errors", 0)] {
        assert_eq!(at(mixed, key)?, sent, "the mixed run's {key}");
    }
    let reads = at(mixed, "reads")?;
    assert_eq!(
        reads.saturating_add(at(mixed, "writes")?),
        200,
        "every request is a read or a write"
    );
    assert!(
        (120..=190).contains(&reads),
        "800 per mille of 200 reads: {reads}"
    );
    assert_eq!(
        (at(again, "reads")?, at(again, "writes")?),
        (reads, 200 - reads),
        "one seed, one mix"
    );
    assert_eq!(
        (at(posts, "reads")?, at(posts, "writes")?),
        (0, 5_000),
        "share 0 posts only"
    );
    assert_eq!(
        (at(gets, "reads")?, at(gets, "writes")?),
        (50, 0),
        "share 1000 reads only"
    );
    let tail = [
        at(mixed, "p50")?,
        at(mixed, "p95")?,
        at(mixed, "p99")?,
        at(mixed, "max")?,
    ];
    assert!(tail.is_sorted(), "p50 <= p95 <= p99 <= max: {tail:?}");
    let world = numbers(&record);
    assert_eq!(
        at(&world, "settled")?,
        5_450,
        "the world settled every request sent: {record}"
    );
    assert_eq!(
        (at(&world, "refused")?, at(&world, "tenants")?),
        (0, 10),
        "{record}"
    );
    Ok(())
}

/// Catches a usage reading, a host default, or a durable scratch store left
/// behind, on a rig of one surface.
#[test]
fn usage_of_one_surface_names_this_host_and_removes_its_store() -> Outcome {
    let scratch = Scratch::new("edge-rig-usage")?;
    let db = scratch.path().join("rig.sqlite");
    let printed = short_rig(&["--only", "serve-durable"], &db)?;
    assert!(
        !printed.contains("axum"),
        "--only ran one surface: {printed}"
    );
    let node = Command::new("uname").arg("-n").output()?;
    let host = String::from_utf8(node.stdout)?.trim().to_owned();
    let method = if cfg!(target_os = "linux") {
        "proc-vmhwm"
    } else {
        "ps-sampled"
    };
    let rows = stored(
        &db,
        "SELECT run, host, cpus > 0, memory_bytes > 0, rss_bytes > 0, commit_sha != '', rss_method \
         FROM rig_row ORDER BY conns, round",
    )?;
    assert_eq!(
        rows.len(),
        2 * 4,
        "one surface, two connection counts, four rounds"
    );
    for row in &rows {
        let cells: Vec<&str> = row.iter().map(String::as_str).collect();
        let &[
            run,
            stored_host,
            cpus,
            memory,
            resident,
            commit,
            stored_method,
        ] = cells.as_slice()
        else {
            return Err(format!("a row has the wrong width: {row:?}").into());
        };
        assert_eq!(
            (stored_host, stored_method),
            (host.as_str(), method),
            "the host and how its RSS was read"
        );
        assert_eq!(
            [cpus, memory, resident, commit],
            ["1"; 4],
            "the host's limits and the server's usage: {row:?}"
        );
        let store = std::env::temp_dir().join(format!("lgwks-edge-rig-{run}-serve-durable"));
        assert!(
            !store.exists(),
            "the durable scratch store is removed: {}",
            store.display()
        );
    }
    let spent = stored(
        &db,
        "SELECT SUM(cpu_us) > 0, COUNT(DISTINCT run) FROM rig_row",
    )?;
    assert_eq!(spent, [["1", "1"]], "the server spent CPU across one run");
    Ok(())
}

/// Catches a full rig that measures a debug build, whose numbers the issue refuses.
#[cfg(debug_assertions)]
#[test]
fn a_full_rig_from_a_debug_build_is_refused() -> Outcome {
    let scratch = Scratch::new("edge-rig-debug")?;
    let db = scratch.path().join("rig.sqlite");
    let ran = rig().args(["--rig", "--db"]).arg(&db).output()?;
    assert!(!ran.status.success(), "a debug build ran a full rig");
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(
        said.contains("release build"),
        "the refusal names the build: {said}"
    );
    assert!(!db.exists(), "a refused rig stored nothing");
    Ok(())
}
