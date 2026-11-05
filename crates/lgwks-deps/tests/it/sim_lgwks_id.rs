//! `lgwks id` driven through the shipped binary under seeded inputs (#851).
//!
//! An operator issues identifiers, packs them into FoundationDB key bytes and
//! reads a key back, each through its own invocation of the built `lgwks`.
//! The tuple layer's specification (`design/tuple.md` in apple/foundationdb)
//! is the model: a UUID element is `30` followed by the 16 bytes as printed.
//! RFC 9562 Appendix A.6's version 7 example pins the bytes. Refusals carry
//! the stable kinds an agent branches on. One seed drives every draw, and the
//! same seed must replay the same trace while distinct seeds diverge.

use std::error::Error;
use std::path::Path;

use lgwks_std::json::Value;
use lgwks_std::seeded::Seeded;

use crate::lgwks_scratch::{Scratch, run_lgwks};

/// What this test returns.
type TestResult = Result<(), Box<dyn Error>>;

/// The seeds every sweep replays, named so a failure names its seed.
const SEEDS: [u64; 4] = [
    0x5EED_0000_0000_0001,
    0x5EED_0000_0000_0002,
    0x5EED_0000_FFFF_FFFF,
    0xDEAD_BEEF_CAFE_0001,
];

/// Identifiers drawn per seed, packed in one `key` invocation.
const PER_SEED: usize = 64;

/// RFC 9562 Appendix A.6, the version 7 example.
const RFC_V7: &str = "017f22e2-79b0-7cc3-98c4-dc0c0c07398f";

/// The FNV-1a offset basis a trace starts from.
const FNV_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// The FNV-1a prime a trace folds with.
const FNV_PRIME: u64 = 0x0100_0000_01b3;

/// Folds `bytes` into `trace`, FNV-1a.
fn fold(trace: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(trace, |acc, byte| {
        (acc ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

/// Runs `lgwks id <args> --json` in `dir`: the exit code and the verdict.
fn invoke(dir: &Path, args: &[&str]) -> Result<(Option<i32>, Value), Box<dyn Error>> {
    let mut argv = vec!["id"];
    argv.extend_from_slice(args);
    argv.push("--json");
    let outcome = run_lgwks(dir, &argv)?;
    let verdict: Value = lgwks_std::json::from_slice(&outcome.stdout).map_err(|error| {
        format!(
            "lgwks {} printed no JSON ({error}): {}",
            argv.join(" "),
            outcome.stderr
        )
    })?;
    Ok((outcome.code, verdict))
}

/// The verdict of a call that must succeed.
fn ok(dir: &Path, args: &[&str]) -> Result<Value, Box<dyn Error>> {
    let (code, verdict) = invoke(dir, args)?;
    assert!(
        code == Some(0) && verdict.get("ok") == Some(&Value::Bool(true)),
        "lgwks id {} failed: {verdict}",
        args.join(" ")
    );
    Ok(verdict)
}

/// The refusal kind of a call that must be refused with exit 2.
fn refused(dir: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let (code, verdict) = invoke(dir, args)?;
    assert_eq!(code, Some(2), "lgwks id {} exit", args.join(" "));
    let kind = verdict
        .get("refusals")
        .and_then(Value::as_array)
        .and_then(|refusals| refusals.first())
        .and_then(|first| first.get("kind"))
        .and_then(Value::as_str)
        .ok_or("a refusal with no kind")?;
    Ok(kind.to_owned())
}

/// One identifier of 128 seeded bits, as canonical text.
fn draw_id(rng: &mut Seeded) -> String {
    let high = lgwks_std::hex::encode(rng.next_u64().to_be_bytes());
    let low = lgwks_std::hex::encode(rng.next_u64().to_be_bytes());
    let (Some(time_low), Some(time_mid), Some(time_high), Some(clock), Some(node)) = (
        high.get(..8),
        high.get(8..12),
        high.get(12..16),
        low.get(..4),
        low.get(4..16),
    ) else {
        return String::new();
    };
    format!("{time_low}-{time_mid}-{time_high}-{clock}-{node}")
}

/// Packs `PER_SEED` seeded identifiers in one call, checks each key against
/// the specification, reads each back with a seeded tail, and returns the
/// trace of every key printed.
fn replay(dir: &Path, seed: u64) -> Result<u64, Box<dyn Error>> {
    let mut rng = Seeded::from_seed(seed);
    let ids: Vec<String> = (0..PER_SEED).map(|_| draw_id(&mut rng)).collect();
    let words: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut args = vec!["key"];
    args.extend_from_slice(&words);
    let verdict = ok(dir, &args)?;
    let rows = verdict
        .get("keys")
        .and_then(Value::as_array)
        .ok_or("no keys array")?;
    assert_eq!(rows.len(), PER_SEED, "seed {seed}");
    let mut trace = FNV_BASIS;
    for (row, id) in rows.iter().zip(&ids) {
        let key = row.get("key").and_then(Value::as_str).ok_or("no key")?;
        assert_eq!(key, format!("30{}", id.replace('-', "")), "seed {seed}");
        trace = fold(trace, key.as_bytes());
    }
    let tail = lgwks_std::hex::encode(rng.next_u64().to_le_bytes());
    let first = rows
        .first()
        .and_then(|row| row.get("key"))
        .and_then(Value::as_str)
        .ok_or("no first key")?;
    let read = ok(dir, &["read", &format!("{first}{tail}")])?;
    assert_eq!(
        read.get("id").and_then(Value::as_str),
        ids.first().map(String::as_str),
        "seed {seed}"
    );
    assert_eq!(
        read.get("rest").and_then(Value::as_str),
        Some(tail.as_str()),
        "seed {seed}"
    );
    Ok(trace)
}

#[test]
fn the_rfc_vector_packs_to_the_specification_bytes_and_reads_back() -> TestResult {
    let scratch = Scratch::fresh("id-rfc")?;
    let verdict = ok(scratch.dir(), &["key", RFC_V7])?;
    let key = verdict
        .get("keys")
        .and_then(Value::as_array)
        .and_then(|keys| keys.first())
        .and_then(|row| row.get("key"))
        .and_then(Value::as_str);
    assert_eq!(key, Some("30017f22e279b07cc398c4dc0c0c07398f"));
    let read = ok(
        scratch.dir(),
        &["read", "30017f22e279b07cc398c4dc0c0c07398f"],
    )?;
    assert_eq!(read.get("id").and_then(Value::as_str), Some(RFC_V7));
    assert_eq!(read.get("rest").and_then(Value::as_str), Some(""));
    Ok(())
}

#[test]
fn every_seed_packs_to_the_model_and_reads_back_with_its_tail() -> TestResult {
    let scratch = Scratch::fresh("id-seeds")?;
    for seed in SEEDS {
        replay(scratch.dir(), seed)?;
    }
    Ok(())
}

#[test]
fn the_same_seed_replays_to_the_same_trace_and_seeds_diverge() -> TestResult {
    let scratch = Scratch::fresh("id-replay")?;
    let [first, second, ..] = SEEDS;
    assert_eq!(replay(scratch.dir(), first)?, replay(scratch.dir(), first)?);
    assert_ne!(
        replay(scratch.dir(), first)?,
        replay(scratch.dir(), second)?
    );
    Ok(())
}

#[test]
fn new_issues_strictly_increasing_version_seven_ids() -> TestResult {
    let scratch = Scratch::fresh("id-new")?;
    let verdict = ok(scratch.dir(), &["new", "--count=500"])?;
    let ids: Vec<&str> = verdict
        .get("ids")
        .and_then(Value::as_array)
        .ok_or("no ids array")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(ids.len(), 500);
    assert!(
        ids.iter().all(|id| id.get(14..15) == Some("7")),
        "version 7"
    );
    assert!(
        ids.windows(2)
            .all(|pair| matches!(pair, [earlier, later] if later > earlier)),
        "issue order"
    );
    Ok(())
}

#[test]
fn a_wrong_code_a_short_key_bad_hex_and_bad_counts_are_refused() -> TestResult {
    let scratch = Scratch::fresh("id-refuse")?;
    let dir = scratch.dir();
    assert_eq!(
        refused(dir, &["read", "33017f22e279b07cc398c4dc0c0c07398f"])?,
        "malformed"
    );
    assert_eq!(refused(dir, &["read", "30017f22"])?, "malformed");
    assert_eq!(refused(dir, &["read", "3g"])?, "malformed");
    assert_eq!(refused(dir, &["key", "not-a-uuid"])?, "malformed");
    assert_eq!(refused(dir, &["new", "--count=0"])?, "bound-exceeded");
    assert_eq!(refused(dir, &["new", "--count=10001"])?, "bound-exceeded");
    assert_eq!(refused(dir, &["new", "--count=many"])?, "malformed");
    assert_eq!(refused(dir, &["mint"])?, "malformed");
    Ok(())
}
