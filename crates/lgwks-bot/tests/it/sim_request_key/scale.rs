//! The scale measurement: concurrent request-keyed submissions across tiers, two tenants over one store file.

use super::*;
use crate::resume_fixtures::{Summary, peak_rss_mib};
use std::io::Write as _;
use std::time::Instant;

/// The environment variable that extends the submission measurement past its
/// first tier.
const SCALE_ENV: &str = "LGWKS_REQUEST_SCALE";

/// The submission tiers the measurement drives: the first on every run, all of
/// them when [`SCALE_ENV`] is set.
///
/// The list stops at ten thousand because that is the largest tier this file
/// actually measured; a tier nobody ran is not a tier this file claims.
const SCALE_TIERS: [usize; 3] = [100, 1_000, 10_000];

/// Concurrent request-keyed submissions across every tier, two tenants
/// interleaved over one store file.
///
/// The hyperscale row for durable submission: what has to hold at ten thousand
/// is that every distinct key derives its own run, nothing is lost, and the two
/// tenants never read each other's receipt. Every submission commits through the
/// store's one owner thread, so the question is whether the derived identities
/// stay distinct under load, not whether the disk can take it.
///
/// Every run drives the first tier with every assertion, so the property is
/// checked on each suite rather than only when someone asks. The two larger
/// tiers are minutes of real `fsync`s, so they run when [`SCALE_ENV`] is set.
/// Each tier prints what it measured, so the output shows which tiers ran.
///
/// ```text
/// LGWKS_REQUEST_SCALE=1 cargo nextest run --locked -p lgwks_bot --features script,ephemeral \
///     --test it --no-capture -E 'test(concurrent_submissions_across_tiers)'
/// ```
#[test]
fn concurrent_submissions_across_tiers() -> TestResult {
    let tiers = match std::env::var_os(SCALE_ENV) {
        Some(_) => &SCALE_TIERS[..],
        None => &SCALE_TIERS[..1],
    };
    for &tier in tiers {
        measure_tier(tier)?;
    }
    Ok(())
}

/// One tier: `tier` distinct keys, two tenants interleaved, over one store.
fn measure_tier(tier: usize) -> TestResult {
    let scratch = Scratch::new("req-scale")?;
    let store = RunStore::open(scratch.path().join("shared.runstore"))?;
    let counter = Rc::new(Cell::new(0));
    let work = counting_task(Rc::clone(&counter))?;
    let hosts = hosts_over(&TENANTS, &store)?;

    let started = Instant::now();
    let mut samples: Vec<u128> = Vec::with_capacity(tier);
    let mut runs: Vec<String> = Vec::with_capacity(tier);
    let mut parity = false;
    let submissions = u32::try_from(tier)?;
    for index in 0..submissions {
        let (host, key) = alternating_target(&hosts, parity, &format!("scale-{index}"))?;
        let at = Instant::now();
        let submission = lgwks_bot::script::block_on(host.submit(&key, &work, index))?;
        samples.push(at.elapsed().as_micros());
        let run = submission.run_id().ok_or("a submission names a run")?;
        runs.push(run.id().to_hex());
        parity = !parity;
    }
    let elapsed = started.elapsed();

    assert_eq!(
        distinct_runs(&runs),
        runs.len(),
        "every key at tier {tier} derived a distinct run"
    );
    assert_eq!(
        counter.get(),
        submissions,
        "every key at tier {tier} ran its body once"
    );

    let summary = Summary::of(&mut samples).ok_or("the tier measured no samples")?;
    let rss = peak_rss_mib();
    let mut line = std::io::stdout().lock();
    let _written = writeln!(
        line,
        "tier {tier}: requested {tier} reached {} ceiling {} in {:.2?}  |  {}  |  peak RSS {rss}",
        runs.len(),
        DECLARED_TIER_CEILING,
        elapsed,
        summary.line("per-submit"),
    );
    Ok(())
}
