//! `edge-load --rig`: every surface on one workload, one table, one row per
//! round (#1007).
//!
//! For each surface the rig starts this binary in that server mode. For each
//! connection count it runs a fixed warm-up, stored as round 0 (the cold
//! numbers), and then three warm rounds, each a separate `drive` process, so
//! the client's CPU never counts against the server's. The table prints the
//! warm median by `rps` and the cold round's `rps` beside it. Every round sends the same workload: 80% `GET`, 20% `POST`,
//! round-robin over 100 tenants, its reads drawn from the round's seed. The
//! surfaces are plain axum (the rival), the world's edge, the app SDK,
//! `rt::serve` over a scratch store, and `rt::serve` over a durable store (the
//! durable-write route). A full rig refuses a debug build; `--short` is a
//! check run with small rounds that any build may make.
//!
//! # The columns of `rig_row`
//!
//! | Column | Unit | What it measures |
//! |---|---|---|
//! | `rps` | requests / s | answers `200` over the driver's wall time, connects included |
//! | `p50_us`, `p95_us`, `p99_us`, `max_us` | µs | nearest-rank latency of every request in the round, first byte written to last byte read |
//! | `errors`, `sent` | requests | answers other than `200`, lost connections, and all sent; the error rate is their ratio |
//! | `rss_bytes` | bytes | the server's peak resident set by the round's end; `rss_method` says how it was read |
//! | `cpu_us` | µs | server user plus system time spent during the round |
//! | `load_avg` | load | the host's one-minute load average as the round starts |
//! | `isolation`, `log_first`, `idempotency`, `durability` | 0 or 1 | the guarantees the surface held |
//!
//! `host` is `--host` (default `uname -n`), `commit` is `--commit` (default
//! `git rev-parse`), and `cpus` and `memory_bytes` are what this process may
//! use, so a run under a cgroup limit names the limit. Rows go to `--db`
//! (default `edge-rig.sqlite`) through the `sqlite3` program in one
//! transaction; with no `sqlite3` the rig refuses before it starts a server.
//!
//! The driver is closed loop, so the tail hides queueing that a constant-rate
//! client would see; read `rps` beside `p99_us`.

use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Stdio};

use lgwks_std::process::{Child, Command};

use crate::usage::{self, Host, RSS_METHOD, Usage};
use crate::{LoadError, refused};

/// One surface and the guarantees it holds.
struct Surface {
    /// The server mode, and the row's surface name.
    mode: &'static str,
    /// Each tenant's state is its own, unreachable from another tenant.
    isolation: bool,
    /// The request is recorded before its handler runs.
    log_first: bool,
    /// A repeated key is answered from the first run, never run twice.
    idempotency: bool,
    /// The record survives a restart of the server.
    durability: bool,
}

/// `mode` holding the guarantees `[isolation, log_first, idempotency, durability]`.
const fn surface(mode: &'static str, held: [bool; 4]) -> Surface {
    let [isolation, log_first, idempotency, durability] = held;
    Surface {
        mode,
        isolation,
        log_first,
        idempotency,
        durability,
    }
}

/// Every surface, the rival first.
const SURFACES: [Surface; 5] = [
    surface("axum", [false, false, false, false]),
    surface("world", [true, false, false, false]),
    surface("app", [true, false, false, false]),
    surface("serve", [true, true, true, false]),
    surface("serve-durable", [true, true, true, true]),
];

/// The size of one rig run.
struct Shape {
    /// The connection counts each surface is driven at.
    conns: [usize; 2],
    /// Requests in the warm-up round.
    warm: usize,
    /// Requests in each measured round.
    round: usize,
}

/// The issue's workload: 64 and 1,024 connections.
const FULL: Shape = Shape {
    conns: [64, 1_024],
    warm: 4_096,
    round: 32_768,
};

/// A check run: every surface and column, in seconds.
const SHORT: Shape = Shape {
    conns: [16, 64],
    warm: 64,
    round: 512,
};

/// Measured rounds per connection count.
const ROUNDS: u64 = 3;

/// Tenants every round goes over.
const TENANTS: usize = 100;

/// Reads per thousand requests.
const READ_PERMILLE: u64 = 800;

/// The first round's seed; round `n` draws from `SEED + n`, round 0 is the warm-up.
const SEED: u64 = 1_007;

/// Raise the open-file limit for a 1,024-connection round, then run the binary.
const RAISE: &str =
    "ulimit -n 8192 2>/dev/null || ulimit -n \"$(ulimit -Hn)\" || exit 97; exec \"$0\" \"$@\"";

/// What the rig was asked.
struct Options {
    /// The check-run shape instead of the full one.
    short: bool,
    /// Where the rows go.
    db: PathBuf,
    /// The host label on every row.
    host: String,
    /// The commit on every row.
    commit: String,
    /// Run only this surface.
    only: Option<String>,
}

/// The options `args` names.
fn options(args: &[&str]) -> Result<Options, LoadError> {
    let mut chosen = Options {
        short: false,
        db: PathBuf::from("edge-rig.sqlite"),
        host: String::new(),
        commit: String::new(),
        only: None,
    };
    let mut words = args.iter();
    while let Some(&word) = words.next() {
        match word {
            "--short" => chosen.short = true,
            "--db" | "--host" | "--commit" | "--only" => {
                let Some(&value) = words.next() else {
                    return refused(LoadError::Usage);
                };
                match word {
                    "--db" => chosen.db = PathBuf::from(value),
                    "--host" => chosen.host = label(value)?,
                    "--commit" => chosen.commit = label(value)?,
                    _ => chosen.only = Some(label(value)?),
                }
            }
            _ => return refused(LoadError::Usage),
        }
    }
    if chosen.host.is_empty() {
        chosen.host = label(&usage::node_name()?)?;
    }
    if chosen.commit.is_empty() {
        chosen.commit = label(&usage::commit())?;
    }
    Ok(chosen)
}

/// `text` as a row label: 1 to 64 printable ASCII bytes. Quotes are allowed;
/// [`quoted`] doubles them, so no label can end its SQL string.
fn label(text: &str) -> Result<String, LoadError> {
    if text.is_empty()
        || text.len() > 64
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
    {
        return refused(LoadError::Usage);
    }
    Ok(text.to_owned())
}

/// `text` as one SQL string literal.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// One measured round, as the driver printed it.
#[derive(Clone, Copy, Debug, Default)]
struct Round {
    /// Requests sent.
    sent: u64,
    /// Answers `200`.
    ok: u64,
    /// Everything else.
    errors: u64,
    /// Answers `200` per second.
    rps: u64,
    /// Median latency, µs.
    p50: u64,
    /// 95th percentile latency, µs.
    p95: u64,
    /// 99th percentile latency, µs.
    p99: u64,
    /// Slowest request, µs.
    max: u64,
    /// The driver's wall time, ms.
    wall: u64,
}

/// The round the driver's `line` reports.
fn round_of(line: &str) -> Result<Round, LoadError> {
    let mut round = Round::default();
    let mut seen = 0_u32;
    for word in line.split_whitespace() {
        let Some((key, value)) = word.split_once('=') else {
            continue;
        };
        let slot = match key {
            "sent" => &mut round.sent,
            "ok" => &mut round.ok,
            "errors" => &mut round.errors,
            "rps" => &mut round.rps,
            "p50" => &mut round.p50,
            "p95" => &mut round.p95,
            "p99" => &mut round.p99,
            "max" => &mut round.max,
            "wall" => &mut round.wall,
            _ => continue,
        };
        let digits = value.trim_end_matches(|ch: char| ch.is_ascii_alphabetic());
        let Ok(number) = digits.parse::<u64>() else {
            return refused(LoadError::Malformed("a driver field is not a number"));
        };
        *slot = number;
        seen = seen.saturating_add(1);
    }
    if seen == 9 {
        Ok(round)
    } else {
        refused(LoadError::Malformed("the driver line lacks a field"))
    }
}

/// One row of `rig_row`.
struct Row {
    /// The surface measured.
    surface: &'static Surface,
    /// Connections in the round.
    conns: usize,
    /// 1 to [`ROUNDS`].
    round: u64,
    /// The seed the round's reads were drawn from.
    seed: u64,
    /// The host's load as the round started.
    load_avg: String,
    /// What the driver printed.
    numbers: Round,
    /// The server's CPU and resident set across the round.
    spent: Usage,
}

/// `binary` with `args`, under the raised open-file limit.
fn command(binary: &Path, args: &[String]) -> Command {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(RAISE).arg(binary).args(args);
    command
}

/// A started server: the child, its stdout, and the address it announced.
struct Server {
    /// The server process; its stdin stays open until the surface is done.
    child: Child,
    /// Read past the address, and kept open so a closing record has a reader.
    stdout: BufReader<ChildStdout>,
    /// Where it listens.
    addr: String,
}

/// Start `binary` in `mode`, with `extra` arguments.
fn start(binary: &Path, mode: &str, extra: Option<&Path>) -> Result<Server, LoadError> {
    let mut args = vec![String::from(mode)];
    if let Some(dir) = extra {
        args.push(dir.display().to_string());
    }
    let mut child = command(binary, &args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let Some(out) = child.take_stdout() else {
        return refused(LoadError::Malformed("the server has no stdout"));
    };
    let mut stdout = BufReader::new(out);
    let mut first = String::new();
    stdout.read_line(&mut first)?;
    let Some(addr) = first.trim().strip_prefix("listening ") else {
        return refused(LoadError::Malformed("the server announced no address"));
    };
    let addr = addr.to_owned();
    Ok(Server {
        child,
        stdout,
        addr,
    })
}

/// Run one driver round at `server`.
fn drive(
    binary: &Path,
    addr: &str,
    conns: usize,
    requests: usize,
    seed: u64,
) -> Result<Round, LoadError> {
    let per = requests.div_euclid(conns).max(1);
    let args = [
        String::from("drive"),
        addr.to_owned(),
        conns.to_string(),
        per.to_string(),
        TENANTS.to_string(),
        READ_PERMILLE.to_string(),
        seed.to_string(),
    ];
    let done = command(binary, &args).stdin(Stdio::null()).output()?;
    if !done.status.success() {
        return refused(LoadError::World(format!(
            "the driver exited {}",
            done.status
        )));
    }
    let text = String::from_utf8_lossy(&done.stdout);
    let line = text
        .lines()
        .last()
        .ok_or(LoadError::Malformed("the driver printed nothing"))?;
    round_of(line)
}

/// Every row of one surface: a warm-up and [`ROUNDS`] rounds at each connection count.
fn surface_rows(
    binary: &Path,
    surface: &'static Surface,
    shape: &Shape,
    run: &str,
) -> Result<Vec<Row>, LoadError> {
    let store = surface
        .durability
        .then(|| std::env::temp_dir().join(format!("lgwks-edge-rig-{run}-{}", surface.mode)));
    let mut server = start(binary, surface.mode, store.as_deref())?;
    let measured = rounds(binary, surface, shape, &server);
    // `wait` closes the server's input first, which says the surface is done,
    // and then kills what is left of its group.
    let exited = server.child.wait();
    let mut tail = String::new();
    let drained = std::io::Read::read_to_string(&mut server.stdout, &mut tail);
    if let Some(dir) = store
        && let Err(error) = std::fs::remove_dir_all(&dir)
    {
        lgwks_std::trace::warn!(%error, dir = %dir.display(), "edge-load: the rig's durable store was not removed");
    }
    let rows = measured?;
    drained?;
    let status = exited?;
    if !status.success() {
        return refused(LoadError::World(format!(
            "{} exited {status}",
            surface.mode
        )));
    }
    Ok(rows)
}

/// The rounds against a started `server`: round 0 is the fixed warm-up,
/// stored too, so the cold numbers stand beside the warm ones.
fn rounds(
    binary: &Path,
    surface: &'static Surface,
    shape: &Shape,
    server: &Server,
) -> Result<Vec<Row>, LoadError> {
    let pid = server.child.id();
    let mut rows = Vec::new();
    let mut peak = 0_u64;
    for conns in shape.conns {
        for round in 0..=ROUNDS {
            let seed = SEED.saturating_add(round);
            let requests = if round == 0 { shape.warm } else { shape.round };
            let load_avg = usage::load_average()?;
            let before = usage::of(pid)?;
            let numbers = drive(binary, &server.addr, conns, requests, seed)?;
            let after = usage::of(pid)?;
            peak = peak.max(after.rss_bytes);
            rows.push(Row {
                surface,
                conns,
                round,
                seed,
                load_avg,
                numbers,
                spent: Usage {
                    cpu_micros: after.cpu_micros.saturating_sub(before.cpu_micros),
                    rss_bytes: peak,
                },
            });
        }
    }
    Ok(rows)
}

/// The facts every row of one run shares.
struct RunFacts {
    /// This run's id.
    run: String,
    /// What was asked.
    options: Options,
    /// What the host gives this process.
    host: Host,
    /// `release` or `debug`.
    profile: &'static str,
}

/// The table, created when absent; one row per surface, connection count and round.
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS rig_row (
    run TEXT NOT NULL, host TEXT NOT NULL, cpus INTEGER NOT NULL, memory_bytes INTEGER NOT NULL,
    commit_sha TEXT NOT NULL, profile TEXT NOT NULL, surface TEXT NOT NULL,
    isolation INTEGER NOT NULL, log_first INTEGER NOT NULL, idempotency INTEGER NOT NULL,
    durability INTEGER NOT NULL, conns INTEGER NOT NULL, round INTEGER NOT NULL,
    seed INTEGER NOT NULL, tenants INTEGER NOT NULL, read_permille INTEGER NOT NULL,
    load_avg TEXT NOT NULL, sent INTEGER NOT NULL, ok INTEGER NOT NULL, errors INTEGER NOT NULL,
    rps INTEGER NOT NULL, p50_us INTEGER NOT NULL, p95_us INTEGER NOT NULL,
    p99_us INTEGER NOT NULL, max_us INTEGER NOT NULL, wall_ms INTEGER NOT NULL,
    rss_bytes INTEGER NOT NULL, rss_method TEXT NOT NULL, cpu_us INTEGER NOT NULL,
    PRIMARY KEY (run, surface, conns, round)
);
";

/// The statements that store `rows`, in one transaction.
fn statements(facts: &RunFacts, rows: &[Row]) -> Result<String, std::fmt::Error> {
    let mut sql = String::from(SCHEMA);
    sql.push_str("BEGIN;\n");
    for row in rows {
        let (surface, numbers) = (row.surface, row.numbers);
        writeln!(
            sql,
            "INSERT INTO rig_row VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {});",
            quoted(&facts.run),
            quoted(&facts.options.host),
            facts.host.cpus,
            facts.host.memory_bytes,
            quoted(&facts.options.commit),
            quoted(facts.profile),
            quoted(surface.mode),
            u8::from(surface.isolation),
            u8::from(surface.log_first),
            u8::from(surface.idempotency),
            u8::from(surface.durability),
            row.conns,
            row.round,
            row.seed,
            TENANTS,
            READ_PERMILLE,
            quoted(&row.load_avg),
            numbers.sent,
            numbers.ok,
            numbers.errors,
            numbers.rps,
            numbers.p50,
            numbers.p95,
            numbers.p99,
            numbers.max,
            numbers.wall,
            row.spent.rss_bytes,
            quoted(RSS_METHOD),
            row.spent.cpu_micros,
        )?;
    }
    sql.push_str("COMMIT;\n");
    Ok(sql)
}

/// Pipe `sql` to `sqlite3 -bail db`.
fn store(db: &Path, sql: &str) -> Result<(), LoadError> {
    let mut child = Command::new("sqlite3")
        .arg("-bail")
        .arg(db)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| LoadError::Store(format!("sqlite3 did not start: {error}")))?;
    // The input closes at the end of this arm, so sqlite3 reads to its end.
    match child.take_stdin() {
        Some(mut input) => input.write_all(sql.as_bytes())?,
        None => return refused(LoadError::Store(String::from("sqlite3 has no stdin"))),
    }
    let done = child.wait_with_output()?;
    if done.status.success() {
        return Ok(());
    }
    refused(LoadError::Store(
        String::from_utf8_lossy(&done.stderr).trim().to_owned(),
    ))
}

/// The guarantees `surface` holds, as words.
fn guarantees(surface: &Surface) -> String {
    let named = [
        (surface.isolation, "isolation"),
        (surface.log_first, "log-first"),
        (surface.idempotency, "idempotency"),
        (surface.durability, "durability"),
    ];
    let held: Vec<&str> = named
        .iter()
        .filter(|&&(on, _)| on)
        .map(|&(_, word)| word)
        .collect();
    if held.is_empty() {
        String::from("none")
    } else {
        held.join(",")
    }
}

/// The table: per surface and connection count, the median round by `rps`.
fn table(facts: &RunFacts, rows: &[Row]) -> Result<String, std::fmt::Error> {
    let mut out = String::new();
    writeln!(
        out,
        "rig {} host={} cpus={} memory={}MiB commit={} profile={} db={}",
        facts.run,
        facts.options.host,
        facts.host.cpus,
        facts.host.memory_bytes.div_euclid(1_048_576),
        facts.options.commit,
        facts.profile,
        facts.options.db.display(),
    )?;
    writeln!(
        out,
        "{:<14} {:>5} {:>8} {:>9} {:>8} {:>8} {:>8} {:>6} {:>8} {:>8} {:>5}  guarantees",
        "surface",
        "conns",
        "rps",
        "cold_rps",
        "p50_us",
        "p95_us",
        "p99_us",
        "err‰",
        "rss_MiB",
        "cpu_ms",
        "load"
    )?;
    for group in rows.chunk_by(|left, right| {
        left.surface.mode == right.surface.mode && left.conns == right.conns
    }) {
        let cold = group
            .iter()
            .find(|row| row.round == 0)
            .map(|row| row.numbers.rps);
        let mut ranked: Vec<&Row> = group.iter().filter(|row| row.round > 0).collect();
        ranked.sort_by_key(|row| row.numbers.rps);
        // Every group holds round 0 and the warm rounds, as `rounds` stores them.
        let (Some(row), Some(cold)) = (ranked.get(ranked.len().div_euclid(2)), cold) else {
            lgwks_std::trace::warn!(
                rows = group.len(),
                "edge-load: a table group lacks its rounds"
            );
            return Err(std::fmt::Error);
        };
        let numbers = row.numbers;
        writeln!(
            out,
            "{:<14} {:>5} {:>8} {:>9} {:>8} {:>8} {:>8} {:>6} {:>8} {:>8} {:>5}  {}",
            row.surface.mode,
            row.conns,
            numbers.rps,
            cold,
            numbers.p50,
            numbers.p95,
            numbers.p99,
            numbers
                .errors
                .saturating_mul(1_000)
                .div_euclid(numbers.sent.max(1)),
            row.spent.rss_bytes.div_euclid(1_048_576),
            row.spent.cpu_micros.div_euclid(1_000),
            row.load_avg,
            guarantees(row.surface),
        )?;
    }
    Ok(out)
}

/// `edge-load --rig ARGS`: measure every surface, store every round, print the table.
pub(super) fn run(args: &[&str]) -> Result<(), LoadError> {
    let options = options(args)?;
    if cfg!(debug_assertions) && !options.short {
        return refused(LoadError::DebugBuild);
    }
    let found = Command::new("sqlite3").arg("-version").output();
    if !matches!(found, Ok(ref done) if done.status.success()) {
        return refused(LoadError::Store(String::from(
            "the rig stores rows through sqlite3, and it is not on PATH",
        )));
    }
    let shape = if options.short { SHORT } else { FULL };
    let binary = std::env::current_exe()?;
    let unique = lgwks_std::random::bytes::<6>()
        .map_err(|error| LoadError::Store(format!("the run id's entropy is refused: {error}")))?;
    let run: String = unique.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut rows = Vec::new();
    for surface in &SURFACES {
        if options
            .only
            .as_deref()
            .is_some_and(|only| only != surface.mode)
        {
            continue;
        }
        rows.extend(surface_rows(&binary, surface, &shape, &run)?);
    }
    if rows.is_empty() {
        return refused(LoadError::Usage);
    }
    let facts = RunFacts {
        run,
        options,
        host: usage::host()?,
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    };
    let to_text =
        |error: std::fmt::Error| LoadError::Store(format!("a row did not render: {error}"));
    store(
        &facts.options.db,
        &statements(&facts, &rows).map_err(to_text)?,
    )?;
    let printed = table(&facts, &rows).map_err(to_text)?;
    let mut out = std::io::stdout().lock();
    out.write_all(printed.as_bytes())?;
    out.flush()?;
    Ok(())
}
