//! Matched-workload measurement of the owned world (W2 of #985, issue #1002): insert, point read, single write,
//! batch commit and changed-since query at 1,000, 100,000 and 1,000,000 rows.
//!
//! Each line names its row count, its operation, its unit, its sample count and the p50, p95 and p99 of those
//! samples, measured in this process on the wall clock. The picks and values are drawn from one seed and the
//! ids are the process's own version 7 ids, so two runs measure the same shape of workload. The bin reads no
//! process id, so it cannot sample its own resident set. The kernel's peak is read from outside, by running
//! the built bin under `/usr/bin/time -l` (`maximum resident set size`, in bytes on macOS).
//!
//! ```text
//! cargo run --release --locked -p lgwks_bot --bin world-bench
//! /usr/bin/time -l target/release/world-bench
//! ```
//!
//! The bin compiles the world module in by path because the module stays private to the crate until the cell
//! uses it (#1004). Every item of the module is called from here or from the module's own simulations, so the
//! lint that finds dead code stays on.

use std::io::Write;
use std::time::Instant;

use lgwks_std::id::Uuid;

#[path = "../../examples/support/measure.rs"]
mod measure;
#[path = "../world.rs"]
mod world;

use world::component::{Ceilings, Column, Component, Malformed, WorldBuilder};
use world::error::WorldError;
use world::state::World;

/// The row counts the issue names.
const SIZES: [usize; 3] = [1_000, 100_000, 1_000_000];

/// The seed every draw starts from.
const SEED: u64 = 0x2002_5eed_c0de_0001;

/// Entities per insert commit.
const INSERT_BATCH: usize = 100;

/// Writes per batch commit.
const COMMIT_BATCH: usize = 100;

/// Samples of the point read and of the single-write commit.
const POINT_SAMPLES: i64 = 10_000;

/// Samples of the batch commit.
const BATCH_SAMPLES: usize = 1_000;

/// Samples of the changed-since query.
const QUERY_SAMPLES: usize = 100;

/// Why a measurement failed.
#[derive(Debug)]
enum BenchError {
    /// The world refused.
    World(WorldError),
    /// Writing the table failed.
    Io(std::io::Error),
    /// A check of the run failed, in fixed words.
    Check(&'static str),
}

impl std::fmt::Display for BenchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::World(ref cause) => write!(formatter, "the world refused: {cause}"),
            Self::Io(ref cause) => write!(formatter, "writing the table failed: {cause}"),
            Self::Check(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for BenchError {}

impl From<WorldError> for BenchError {
    fn from(cause: WorldError) -> Self {
        Self::World(cause)
    }
}

impl From<std::io::Error> for BenchError {
    fn from(cause: std::io::Error) -> Self {
        Self::Io(cause)
    }
}

/// Log `error` and hand it back, so every refusal leaves a record.
fn refused<T>(error: BenchError) -> Result<T, BenchError> {
    lgwks_std::trace::warn!(%error, "world-bench: refused");
    Err(error)
}

/// A position: sixteen bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pos {
    /// East.
    x: i64,
    /// North.
    y: i64,
}

impl Component for Pos {
    const NAME: &'static str = "pos";

    fn to_wire(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
    }

    fn from_wire(bytes: &[u8]) -> Result<Self, Malformed> {
        match *bytes {
            [
                x0,
                x1,
                x2,
                x3,
                x4,
                x5,
                x6,
                x7,
                y0,
                y1,
                y2,
                y3,
                y4,
                y5,
                y6,
                y7,
            ] => Ok(Self {
                x: i64::from_le_bytes([x0, x1, x2, x3, x4, x5, x6, x7]),
                y: i64::from_le_bytes([y0, y1, y2, y3, y4, y5, y6, y7]),
            }),
            _ => Err(Malformed),
        }
    }
}

/// A tag: eight bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tag(u64);

impl Component for Tag {
    const NAME: &'static str = "tag";

    fn to_wire(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.0.to_le_bytes());
    }

    fn from_wire(bytes: &[u8]) -> Result<Self, Malformed> {
        match *bytes {
            [b0, b1, b2, b3, b4, b5, b6, b7] => {
                Ok(Self(u64::from_le_bytes([b0, b1, b2, b3, b4, b5, b6, b7])))
            }
            _ => Err(Malformed),
        }
    }
}

/// SplitMix64: one word of state, so the seed is the whole workload.
struct Draw(u64);

impl Draw {
    /// The next word.
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ (mixed >> 31)
    }

    /// A drawn index below `bound`; `None` for an empty range.
    fn below(&mut self, bound: usize) -> Option<usize> {
        let word = self.next();
        let span = match u64::try_from(bound) {
            Ok(span) => span,
            Err(error) => {
                lgwks_std::trace::debug!(error = %error, bound, "below: the bound does not fit a word");
                return None;
            }
        };
        match usize::try_from(word.checked_rem(span)?) {
            Ok(index) => Some(index),
            Err(error) => {
                lgwks_std::trace::debug!(error = %error, "below: the draw does not fit an index");
                None
            }
        }
    }
}

/// One measured line of the table.
fn report(
    out: &mut impl Write,
    rows: usize,
    operation: &str,
    unit: &str,
    samples: &mut [u128],
) -> Result<(), BenchError> {
    let count = samples.len();
    let summary = measure::Summary::of(samples);
    let line = if unit == "us" {
        summary.line(operation)
    } else {
        summary.line_in(operation, unit)
    };
    writeln!(out, "rows={rows} samples={count} {line}")?;
    Ok(())
}

/// An empty world of this bench's tenant and its two columns. A row costs 24 value bytes and a cell in each of
/// the two columns, 48 bytes of table, so 128 bytes a row leaves room for the rows and the spare slots.
fn build(rows: usize) -> Result<(World, Column<Pos>, Column<Tag>), BenchError> {
    let ceilings = Ceilings::new(4, rows.saturating_add(1_024), rows.saturating_mul(128))?;
    let mut builder = WorldBuilder::new("bench", ceilings)?;
    let pos = builder.column(&Pos { x: 0, y: 0 })?;
    let tag = builder.column(&Tag(0))?;
    Ok((builder.build(), pos, tag))
}

/// The ids of a loaded world, in spawn order.
type Ids = Vec<Uuid>;

/// Fill the world to `rows` entities in commits of [`INSERT_BATCH`], timing each commit.
fn insert(
    world: &mut World,
    columns: (Column<Pos>, Column<Tag>),
    rows: usize,
) -> Result<(Ids, Vec<u128>), BenchError> {
    let (pos, tag) = columns;
    let mut ids = Vec::with_capacity(rows);
    let mut samples = Vec::new();
    let mut seq = 0_u64;
    while ids.len() < rows {
        let started = Instant::now();
        let mut tx = world.begin();
        let mut batch = 0;
        while batch < INSERT_BATCH && ids.len() < rows {
            seq = seq.saturating_add(1);
            let id = Uuid::now_v7();
            tx.spawn(id)?;
            tx.set(pos, id, &Pos { x: 0, y: 0 })?;
            tx.set(tag, id, &Tag(seq))?;
            ids.push(id);
            batch = batch.saturating_add(1);
        }
        tx.commit()?;
        samples.push(started.elapsed().as_micros());
    }
    Ok((ids, samples))
}

/// A drawn id of the loaded world.
fn pick(ids: &[Uuid], draw: &mut Draw) -> Result<Uuid, BenchError> {
    let at = draw
        .below(ids.len())
        .ok_or(BenchError::Check("a draw from an empty world"))?;
    ids.get(at)
        .copied()
        .ok_or(BenchError::Check("a drawn index is past the ids"))
}

/// Time point reads, single-write commits, batch commits and the changed-since query.
fn operate(
    out: &mut impl Write,
    world: &mut World,
    columns: (Column<Pos>, Column<Tag>),
    ids: &[Uuid],
    draw: &mut Draw,
) -> Result<(), BenchError> {
    let (pos, tag) = columns;
    let rows = ids.len();
    let mut reads = Vec::new();
    for _ in 0..POINT_SAMPLES {
        let id = pick(ids, draw)?;
        let started = Instant::now();
        let found = world.get(pos, &id)?;
        reads.push(started.elapsed().as_nanos());
        if found.is_none() {
            return refused(BenchError::Check("a loaded row has a position"));
        }
    }
    report(out, rows, "get", "ns", &mut reads)?;

    let mut writes = Vec::new();
    for round in 0..POINT_SAMPLES {
        let id = pick(ids, draw)?;
        let started = Instant::now();
        let mut tx = world.begin();
        tx.set(pos, id, &Pos { x: round, y: 0 })?;
        tx.commit()?;
        writes.push(started.elapsed().as_nanos());
    }
    report(out, rows, "write-commit(1 op)", "ns", &mut writes)?;

    let mut batches = Vec::new();
    for _ in 0..BATCH_SAMPLES {
        let started = Instant::now();
        let mut tx = world.begin();
        for _ in 0..COMMIT_BATCH {
            let id = pick(ids, draw)?;
            tx.set(tag, id, &Tag(draw.next()))?;
        }
        tx.commit()?;
        batches.push(started.elapsed().as_micros());
    }
    report(out, rows, "batch-commit(100 ops)", "us", &mut batches)?;

    let since = world.tick().saturating_sub(1);
    let mut queries = Vec::new();
    let mut moved = 0_usize;
    for _ in 0..QUERY_SAMPLES {
        let started = Instant::now();
        moved = world
            .changed_since(since)
            .filter(|changed| changed.alive && changed.touches(tag))
            .count();
        queries.push(started.elapsed().as_micros());
    }
    report(out, rows, "changed-since(last tick)", "us", &mut queries)?;
    writeln!(out, "rows={rows} the last tick moved {moved} rows")?;
    Ok(())
}

/// Hash, snapshot and restore once each, and prove the restored world is the loaded one.
fn checkpoint(out: &mut impl Write, world: &World, rows: usize) -> Result<(), BenchError> {
    let started = Instant::now();
    let before = world.hash()?;
    let hash_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let bytes = world.snapshot_since(0)?;
    let snapshot_ms = started.elapsed().as_millis();
    let mut other = build(rows)?.0;
    let started = Instant::now();
    other.restore(&[bytes.as_slice()])?;
    let restore_ms = started.elapsed().as_millis();
    if other.hash()? != before {
        return refused(BenchError::Check(
            "a restored world hashes as the loaded one",
        ));
    }
    writeln!(
        out,
        "rows={rows} samples=1 hash={hash_ms}ms snapshot={snapshot_ms}ms restore={restore_ms}ms snapshot_bytes={}",
        bytes.len()
    )?;
    Ok(())
}

/// Walk the operations the timed loops do not reach, untimed, so this bin is a real caller of the module.
fn self_check(out: &mut impl Write) -> Result<(), BenchError> {
    let (mut world, pos, tag) = build(8)?;
    let mut live = Vec::new();
    let mut tx = world.begin();
    for seq in 1..=4_u64 {
        let id = Uuid::now_v7();
        tx.spawn(id)?;
        tx.set(pos, id, &Pos { x: 1, y: 2 })?;
        tx.set(tag, id, &Tag(seq))?;
        live.push(id);
    }
    if tx.pending() != 12 {
        return refused(BenchError::Check("a transaction holds what was written"));
    }
    let log = tx.commit()?;
    let Some(&first) = live.first() else {
        return refused(BenchError::Check("the self check spawned an entity"));
    };
    let mut tx = world.begin();
    if tx.get(tag, &first)? != Some(Tag(1)) {
        return refused(BenchError::Check("a transaction reads the world"));
    }
    if !tx.remove(tag, first)? {
        return refused(BenchError::Check("a held cell is removed"));
    }
    tx.despawn(first)?;
    drop(tx);
    let mut twin = build(8)?.0;
    twin.replay(&log)?;
    if twin.hash()? != world.hash()? {
        return refused(BenchError::Check("a replayed log builds the live world"));
    }
    let (alive, bytes) = world.recount();
    if alive != world.live() || bytes != world.bytes() {
        return refused(BenchError::Check("the running totals match a recount"));
    }
    let Some(entity) = world.entity(&first) else {
        return refused(BenchError::Check("a live id has a slot"));
    };
    if entity.index != 0 || entity.generation != 0 || world.raw(pos.index(), &first)?.is_none() {
        return refused(BenchError::Check("the first slot is the first entity"));
    }
    if world.tenant() != "bench" || world.tick() != 1 {
        return refused(BenchError::Check("the world names its tenant and its tick"));
    }
    let mut tight = WorldBuilder::new("tight", Ceilings::new(1, 1, 1)?)?;
    tight.column(&Tag(0))?;
    if let Err(refusal) = tight.column(&Pos { x: 0, y: 0 }) {
        writeln!(out, "self check: past the columns ceiling: {refusal}")?;
    }
    Ok(())
}

fn main() -> Result<(), BenchError> {
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "world-bench seed={SEED:#x} command=\"cargo run --release --locked -p lgwks_bot --bin world-bench\""
    )?;
    self_check(&mut out)?;
    for rows in SIZES {
        let mut draw = Draw(SEED);
        let (mut world, pos, tag) = build(rows)?;
        let (ids, mut inserts) = insert(&mut world, (pos, tag), rows)?;
        report(
            &mut out,
            rows,
            "insert-commit(100 rows)",
            "us",
            &mut inserts,
        )?;
        if world.live() != rows {
            return refused(BenchError::Check("the world holds every inserted row"));
        }
        operate(&mut out, &mut world, (pos, tag), &ids, &mut draw)?;
        checkpoint(&mut out, &world, rows)?;
    }
    Ok(())
}

/// The table line's shape, held apart from the timings.
#[cfg(test)]
mod tests {
    use super::{BenchError, report, self_check};

    /// Catches a self check that passes a broken world (the saboteur inside the bench): the check runs on
    /// its own small world and every claim it makes holds, so a flipped comparison in it refuses (#1002).
    #[test]
    fn world_bench_self_check_holds() -> Result<(), BenchError> {
        let mut out = Vec::new();
        self_check(&mut out)?;
        assert!(
            String::from_utf8_lossy(&out).contains("past the columns ceiling"),
            "the self check names the refused column"
        );
        Ok(())
    }

    /// Catches a table line that loses its sample count, its row count or a percentile: 100 known samples in
    /// each unit give nearest-rank p50, p95 and p99 of 50, 95 and 99 on a line that names all three (#1002).
    #[test]
    fn bench_report_names_samples_and_percentiles() -> Result<(), BenchError> {
        for (unit, label) in [("ns", "get"), ("us", "commit")] {
            let mut out = Vec::new();
            let mut samples: Vec<u128> = (1..=100).rev().collect();
            report(&mut out, 7, label, unit, &mut samples)?;
            assert_eq!(
                String::from_utf8_lossy(&out),
                format!("rows=7 samples=100 {label}: p50=50{unit} p95=95{unit} p99=99{unit}\n"),
                "the line for unit {unit}"
            );
        }
        Ok(())
    }
}
