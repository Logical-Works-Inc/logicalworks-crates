//! The concurrency mode of `parse_budget`: one tier of checked parses split
//! across scoped workers at admission.

use std::time::Instant;

use lgwks_ast::{Language, try_parse};

use super::{column, megabytes_per_second_milli, spread, tile, width_of};

/// One worker's share of a concurrency tier.
struct TierWorker {
    /// How many parses this worker was given.
    assigned: u64,
    /// Per-parse nanoseconds, in the order the worker took them.
    samples: Vec<u128>,
    /// How many of the answers were typed refusals.
    refused: u64,
}

/// What one concurrency tier reports.
pub(super) struct TierRow {
    /// Parses admitted.
    level: u64,
    /// Bytes per parse.
    ///
    /// A `u128` for the same reason as `Measured::bytes`: the throughput
    /// column divides bytes by a nanosecond count, and widening here keeps the
    /// arithmetic below single-width.
    bytes: u128,
    /// Workers admitted.
    workers: usize,
    /// Median parse latency.
    p50: u128,
    /// 99th-percentile parse latency.
    p99: u128,
    /// The slowest parse observed.
    max: u128,
    /// Parses answered with a typed refusal rather than a tree.
    refused: u64,
    /// Whole-run wall time.
    wall_ns: u128,
    /// The grammar this tier measured.
    grammar: &'static str,
}

/// Run `level` checked parses of `source` across at most `threads` workers.
///
/// The index range is split at admission, so a worker owns its slice outright:
/// no queue, no lock, and a worker's retained state is its own slice. Every
/// worker is joined by the scope, so no handle outlives the run.
pub(super) fn measure_tier(
    language: Language,
    representative: &str,
    level: u64,
    bytes: usize,
    threads: usize,
) -> Result<TierRow, String> {
    let workers = threads.max(1);
    // The work count is at most sixty-four bits wide on every target, so this
    // conversion refuses rather than assuming a worker count it cannot hold.
    let workers_u64 = u64::try_from(workers)
        .map_err(|error| format!("{workers} workers do not fit the tier's arithmetic: {error}"))?;
    // `integer_division` is forbidden, so the split of the level across the
    // workers is the checked one. `workers` is at least one by construction, so
    // the refusal below is unreachable and states the invariant it rests on.
    let share = level
        .checked_div(workers_u64)
        .ok_or("a tier with no workers cannot be split")?;
    let tiled = tile(representative, bytes);
    let sized = non_empty_tier(&tiled, bytes);
    sized?;
    let source = std::sync::Arc::new(tiled);
    let mut reports: Vec<TierWorker> = Vec::with_capacity(workers);
    let started = Instant::now();
    let admitted = std::thread::scope(|scope| -> Result<(), String> {
        let mut handles = Vec::with_capacity(workers);
        for worker in 0..workers {
            let owned = std::sync::Arc::clone(&source);
            let at = u64::try_from(worker)
                .map_err(|error| format!("worker {worker} does not fit its own index: {error}"))?;
            let first = share.saturating_mul(at);
            let count = if worker.saturating_add(1) == workers {
                level.saturating_sub(first)
            } else {
                share
            };
            let handle = std::thread::Builder::new()
                .name(format!("ast-tier-{worker}"))
                .spawn_scoped(scope, move || -> Result<TierWorker, String> {
                    let room = usize::try_from(count).map_err(|error| {
                        format!("{count} parses do not fit this target's address space: {error}")
                    })?;
                    let mut samples = Vec::with_capacity(room);
                    let mut refused = 0_u64;
                    for _ in 0..count {
                        let at = Instant::now();
                        let answer = try_parse(&owned, language);
                        samples.push(at.elapsed().as_nanos());
                        if answer.is_err() {
                            refused = refused.saturating_add(1);
                        }
                    }
                    Ok(TierWorker {
                        assigned: count,
                        samples,
                        refused,
                    })
                })
                .map_err(|error| format!("a tier worker did not start: {error}"))?;
            handles.push(handle);
        }
        for handle in handles {
            match handle.join() {
                Ok(Ok(report)) => reports.push(report),
                // A worker's own refusal names the shortfall it hit rather than
                // reporting a tier row over fewer parses than it admitted.
                Ok(Err(shortfall)) => {
                    let refusal = Err(shortfall);
                    tracing::debug!(error = ?refusal.as_ref().err(), "tier: returning a worker's refusal to the caller");
                    return refusal;
                }
                // A panicking worker reports zero, and the count check below
                // turns that into a refusal naming the shortfall rather than a
                // tier that silently measured fewer parses than it admitted.
                Err(panicked) => {
                    tracing::debug!(payload = ?panicked, "tier: a worker panicked; its parses count as unadmitted");
                    reports.push(TierWorker {
                        assigned: 0,
                        samples: Vec::new(),
                        refused: 0,
                    });
                }
            }
        }
        Ok(())
    });
    admitted?;
    let wall_ns = started.elapsed().as_nanos();

    let assigned = reports
        .iter()
        .fold(0_u64, |total, row| total.saturating_add(row.assigned));
    let complete = all_admitted(assigned, level);
    complete?;
    let refused = reports
        .iter()
        .fold(0_u64, |total, row| total.saturating_add(row.refused));
    let mut samples: Vec<u128> = reports.iter().flat_map(|row| row.samples.clone()).collect();
    let (p50, p99) = spread(&mut samples)?;
    // The slowest parse observed is read off the sorted list rather than
    // defaulted: a tier whose workers admitted nothing is already refused by
    // `all_admitted`, so the list is never empty here.
    let max = samples
        .last()
        .copied()
        .ok_or("every admitted parse reported no elapsed time")?;
    let bytes = width_of(source.len())?;
    Ok(TierRow {
        level: assigned,
        bytes,
        workers,
        p50,
        p99,
        max,
        refused,
        wall_ns,
        grammar: language.name(),
    })
}

/// Refuse a tier source that holds nothing.
///
/// `tile` takes whole copies only, so a fragment longer than the tier size tiles
/// to the empty string, and a tier of empty parses reports throughput nobody
/// measured.
fn non_empty_tier(source: &str, bytes: usize) -> Result<(), String> {
    if source.is_empty() {
        Err(format!(
            "no whole copy of the fragment fits in {bytes} bytes, so the tier would parse nothing"
        ))
    } else {
        Ok(())
    }
}

/// Refuse a tier whose workers admitted fewer parses than the level asked for.
///
/// A worker that panicked reports zero admitted, so a short count is the one
/// place a lost worker becomes visible; a tier row over fewer parses than its
/// level names would be a concurrency number nobody ran.
fn all_admitted(assigned: u64, level: u64) -> Result<(), String> {
    if assigned == level {
        Ok(())
    } else {
        Err(format!("{assigned} of {level} parses were admitted"))
    }
}

/// The line one tier row prints.
pub(super) fn tier_line(row: &TierRow) -> String {
    // The tier's throughput is the bytes it parsed over the wall time: the level
    // is a `u64` and the bytes a `u128`, so the widening is total and needs no
    // stand-in for a conversion.
    let parsed = u128::from(row.level).saturating_mul(row.bytes);
    format!(
        "tier\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        row.level,
        row.bytes,
        row.workers,
        row.p50,
        row.p99,
        row.max,
        row.wall_ns,
        column(megabytes_per_second_milli(parsed, row.wall_ns)),
        row.refused,
        row.grammar,
    )
}
