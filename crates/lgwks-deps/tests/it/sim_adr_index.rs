//! Seeded fools against the ADR index (`docs/adr/README.md`). The reference
//! model is the record directory itself: every `NNNN-*.md` record is linked
//! exactly once and no numbered link names a record that is gone. The saboteur
//! drops rows, the double-clicker repeats rows, and the fat-finger adds rows for
//! records that do not exist; the model names each fault and nothing else.
//!
//! Source: the merge of main (#925, ADR 0031) into #923 (ADRs 0029 and 0030),
//! where both sides appended rows to the same table end and taking either side
//! alone would have dropped the other's records from the index.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use crate::seed::{Rng, Trace};

/// A test's result type.
type TestResult = Result<(), Box<dyn Error>>;

/// How many mutated indexes each run judges; each is one string, no disk.
const SEEDS: u64 = 2_048;

/// The record directory of this repository.
fn adr_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/adr")
}

/// Whether `name` is a numbered record: four digits, a dash, and `.md`.
fn is_record(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() > 8
        && bytes.iter().take(4).all(u8::is_ascii_digit)
        && bytes.get(4) == Some(&b'-')
        && name.ends_with(".md")
}

/// The committed records by file name, sorted, and the committed index.
fn committed() -> Result<(Vec<String>, String), Box<dyn Error>> {
    let mut records = Vec::new();
    for entry in fs::read_dir(adr_dir())? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if is_record(&name) {
            records.push(name);
        }
    }
    records.sort();
    let index = fs::read_to_string(adr_dir().join("README.md"))?;
    Ok((records, index))
}

/// Every way `index` fails to list `records`, sorted: a record linked other
/// than once, and a numbered link that names no record.
fn index_problems(index: &str, records: &[String]) -> BTreeSet<String> {
    let mut problems = BTreeSet::new();
    for record in records {
        let rows = index.matches(&format!("]({record})")).count();
        if rows != 1 {
            problems.insert(format!("{record} is linked {rows} times"));
        }
    }
    let links = index
        .split("](")
        .skip(1)
        .filter_map(|rest| rest.split(')').next());
    for linked in links {
        if is_record(linked) && !records.iter().any(|record| record == linked) {
            problems.insert(format!("{linked} is linked and no such record exists"));
        }
    }
    problems
}

/// The mutated index of one seed, and the problems the model expects of it.
struct Mutated {
    text: String,
    expected: BTreeSet<String>,
}

/// One seed's drops, repeats and stale rows over `index`.
fn mutate(index: &str, records: &[String], rng: &mut Rng) -> Result<Mutated, Box<dyn Error>> {
    let mut text = index.to_owned();
    let mut expected = BTreeSet::new();
    let count = u32::try_from(records.len())?;
    let mut touched = BTreeSet::new();
    for _ in 0..rng.below(4) {
        let pick = usize::try_from(rng.below(count))?;
        let record = records.get(pick).ok_or("a draw past the records")?;
        if !touched.insert(record.clone()) {
            continue;
        }
        if rng.below(2) == 0 {
            text = text.replace(&format!("]({record})"), "](gone)");
            expected.insert(format!("{record} is linked 0 times"));
        } else {
            writeln!(text, "\n| [again]({record}) | repeated |")?;
            expected.insert(format!("{record} is linked 2 times"));
        }
    }
    for stale in 0..rng.below(3) {
        let number = 9_000_u32.saturating_add(rng.below(999));
        let name = format!("{number:04}-stale-{stale}.md");
        writeln!(text, "\n| [stale]({name}) | gone |")?;
        expected.insert(format!("{name} is linked and no such record exists"));
    }
    Ok(Mutated { text, expected })
}

/// One seed judged against the model; its trace is the problems it named.
fn one(index: &str, records: &[String], seed: u64) -> Result<u64, Box<dyn Error>> {
    let mut rng = Rng::new(seed);
    let mutated = mutate(index, records, &mut rng)?;
    let found = index_problems(&mutated.text, records);
    assert_eq!(
        found, mutated.expected,
        "seed {seed}: the check against the model"
    );
    let mut trace = Trace::new();
    trace.record_number("seed", seed);
    for problem in &found {
        trace.record(problem);
    }
    Ok(trace.hash())
}

/// The committed index lists every committed record once and links none that
/// is gone, and every seeded drop, repeat and stale row is named by the model,
/// with the same seed replaying to the same trace.
#[test]
fn sim_the_adr_index_names_every_dropped_repeated_and_stale_row() -> TestResult {
    let (records, index) = committed()?;
    assert!(
        records.len() > 30,
        "the records are read: {}",
        records.len()
    );
    assert_eq!(
        index_problems(&index, &records),
        BTreeSet::new(),
        "the committed index lists every record once"
    );
    for seed in 0..SEEDS {
        let first = one(&index, &records, seed)?;
        let again = one(&index, &records, seed)?;
        assert_eq!(first, again, "seed {seed} replays to the same trace");
    }
    Ok(())
}
