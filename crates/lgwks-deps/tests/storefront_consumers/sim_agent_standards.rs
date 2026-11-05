//! Seeded adversaries against the agent-standards journeys (ADR 0015).
//!
//! Each runs a storefront-matrix example in its `sim <first> <count>` mode,
//! the same build its journey test made, so a sim costs its run and no
//! compile. The example holds the oracle (a reference model or an invariant,
//! never the crate's own output) and exits non-zero on the first seed that
//! breaks it; these tests add replay (the same seeds print the same trace
//! hash) and coverage (every fool was reached, counted from the trace).

use std::error::Error;

use super::{TestResult, cargo_run, fixtures, output_text};

/// Run `example` with only `feature` on over seeds `first..first+count` and
/// return its trace line.
fn sim_trace(
    feature: &str,
    example: &str,
    first: u64,
    count: u64,
) -> Result<String, Box<dyn Error>> {
    let manifest = fixtures().join("storefront-matrix/Cargo.toml");
    let (first, count) = (first.to_string(), count.to_string());
    let output = cargo_run(
        &manifest,
        &[
            "--no-default-features",
            "--features",
            feature,
            "--example",
            example,
            "--",
            "sim",
            first.as_str(),
            count.as_str(),
        ],
    )?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let trace = stdout.lines().find(|line| line.starts_with("trace "));
        trace
            .map(str::to_owned)
            .ok_or_else(|| format!("{example} printed no trace:\n{}", output_text(&output)).into())
    } else {
        Err(format!(
            "{example} broke an invariant on seeds {first}+{count}:\n{}",
            output_text(&output)
        )
        .into())
    }
}

/// The value of `name=<n>` in a trace line; a counter never recorded is 0.
fn counter(trace: &str, name: &str) -> Result<u64, Box<dyn Error>> {
    let found = trace
        .split(' ')
        .find_map(|word| word.strip_prefix(name)?.strip_prefix('='));
    match found {
        Some(value) => Ok(value.parse()?),
        None => Ok(0),
    }
}

/// The `hash=<hex>` of a trace line.
fn replay_hash(trace: &str) -> Result<&str, Box<dyn Error>> {
    trace
        .split(' ')
        .find_map(|word| word.strip_prefix("hash="))
        .ok_or_else(|| format!("a trace with no hash: {trace}").into())
}

/// Monkey, giant-and-empty and weird-bytes logs (0 to 1,100 entries of empty,
/// NUL, 0xFF, invalid UTF-8 and repeated entries): every root equals the RFC
/// 6962 §2.1 reference hash, every flipped entry is refused, every prefix is
/// proved consistent. Catches a leaf or node domain-separation slip, an
/// off-by-one at a power-of-two split, and a proof that accepts a forgery.
#[test]
fn sim_merkle_log_monkey_roots_match_the_rfc6962_reference_model() -> TestResult {
    let first = sim_trace("merkle-log", "merkle_log", 0, 96)?;
    let again = sim_trace("merkle-log", "merkle_log", 0, 96)?;
    assert_eq!(first, again, "seeds 0..96 replayed a different trace");
    let next = sim_trace("merkle-log", "merkle_log", 96, 96)?;
    assert_ne!(
        replay_hash(&first)?,
        replay_hash(&next)?,
        "two seed ranges drew one trace"
    );
    assert_eq!(
        counter(&first, "roots")?,
        96,
        "a seed's root went unchecked: {first}"
    );
    assert!(
        counter(&first, "forgeries_refused")? > 0,
        "no forgery was tried: {first}"
    );
    assert!(
        counter(&first, "consistent_prefixes")? > 0,
        "no prefix was proved: {first}"
    );
    Ok(())
}

/// Weird-bytes and fat-finger tool calls (NUL, right-to-left and zero-width
/// marks, traversal, multi-byte runs at 255/256/257 code points, wrong types,
/// stowaway keys, a non-object): every verdict equals the reference model's.
/// Catches a byte-length count where JSON Schema counts code points, an enum
/// that admits a near miss, and `additionalProperties` left open.
#[test]
fn sim_json_schema_weird_bytes_verdicts_match_the_reference_model() -> TestResult {
    let trace = sim_trace("json-schema", "json_schema", 0, 256)?;
    let admitted = counter(&trace, "admitted")?;
    let refused = counter(&trace, "refused")?;
    assert_eq!(
        admitted.saturating_add(refused),
        256 * 32,
        "a call went unjudged: {trace}"
    );
    assert!(
        admitted > 0 && refused > 0,
        "one side of the schema was never reached: {trace}"
    );
    let replay = sim_trace("json-schema", "json_schema", 0, 256)?;
    assert_eq!(
        replay_hash(&trace)?,
        replay_hash(&replay)?,
        "seeds 0..256 replayed differently"
    );
    Ok(())
}

/// Saboteur, neighbour and double-clicker receipts (payloads of 0 to 8,192
/// bytes, two tenants per seed): no flipped bit or truncation opens, no tenant
/// opens another's receipt or one bound to another context, and sealing twice
/// is the same bytes. Catches an unchecked signature, a key id trusted in
/// place of the key, and malleable or non-deterministic sealing.
#[test]
fn sim_cose_saboteur_tampered_receipts_never_open() -> TestResult {
    for (first, count) in [(0_u64, 48_u64), (48, 48)] {
        let trace = sim_trace("cose", "cose", first, count)?;
        assert_eq!(counter(&trace, "neighbours_refused")?, count, "{trace}");
        assert_eq!(counter(&trace, "flips_refused")?, count * 16, "{trace}");
        assert_eq!(counter(&trace, "truncations_refused")?, count, "{trace}");
    }
    Ok(())
}

/// The double-clicker at 100 and 1,000 threads: each fresh process races its
/// first `install_ring` from every thread after a seeded spin, every racer is
/// told ring is installed, and the model journey then runs on it. Catches a
/// loser of the install race told another provider won, and a race that
/// installs twice.
#[test]
fn sim_tls_double_clicker_racers_all_agree_on_ring() -> TestResult {
    let mut tiers = Vec::new();
    for seed in 0_u64..6 {
        let trace = sim_trace("model", "model", seed, 1)?;
        let hundred = counter(&trace, "hundred_agreed")?;
        let thousand = counter(&trace, "thousand_agreed")?;
        assert_eq!(
            hundred.saturating_add(thousand),
            1,
            "seed {seed} raced no tier: {trace}"
        );
        tiers.push(thousand);
    }
    assert!(
        tiers.contains(&0) && tiers.contains(&1),
        "seeds 0..6 never reached both tiers: {tiers:?}"
    );
    Ok(())
}
