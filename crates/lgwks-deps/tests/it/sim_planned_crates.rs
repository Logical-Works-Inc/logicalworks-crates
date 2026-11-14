//! Seeded fools against the crate plan of ADR 0052. The record's map names the
//! crates the runtime becomes; `CODEBOOK.md` must name each one as planned
//! until the root `Cargo.toml` lists it as a member, and from then on give it
//! its own module-map row and drop it from the planned text.
//!
//! The reference model is the three files themselves. The saboteur deletes a
//! planned name, the early lander adds a member and writes no row, the careful
//! lander adds a member, a row and drops the planned name, and the vandal
//! renames a member's row. The model names each fault and nothing else.
//!
//! Source: #988's proof ("names the seven crates as planned, marked planned
//! until each carve lands") and SD-7, a doc claim with no enforcement.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use crate::seed::{Rng, Trace};

/// A test's result type.
type TestResult = Result<(), Box<dyn Error>>;

/// How many mutated trees each run judges; each is three strings, no disk.
const SEEDS: u64 = 2_048;

/// The record whose map this test reads.
const RECORD: &str = "docs/adr/0052-the-runtime-is-lgwks-runtime-in-seven-crates.md";

/// The marker that opens the planned text in `CODEBOOK.md`.
const PLANNED: &str = "Planned (ADR 0052)";

/// The repository root.
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The crates the record's `text` map names: the first word of each line in
/// its first `text` fence that starts `lgwks_` and does not join two crates.
fn mapped(record: &str) -> Vec<String> {
    let mut inside = false;
    let mut names: Vec<String> = Vec::new();
    for line in record.lines() {
        if line.starts_with("```") {
            if inside {
                break;
            }
            inside = line == "```text";
            continue;
        }
        if !inside || line.contains(" + ") {
            continue;
        }
        let bare = line.trim_start_matches([' ', '└', '├', '─', '│']);
        let Some(word) = bare.split_whitespace().next() else {
            continue;
        };
        if word.starts_with("lgwks_") && !names.iter().any(|name| name == word) {
            names.push(word.to_owned());
        }
    }
    names
}

/// The package names the root manifest's `members` list holds, read as Cargo's
/// directory convention: `crates/lgwks-std` is `lgwks_std`.
fn members(manifest: &str) -> BTreeSet<String> {
    let mut inside = false;
    let mut found = BTreeSet::new();
    for raw in manifest.lines() {
        let line = raw.trim();
        if line.starts_with("members = [") {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line.starts_with(']') {
            break;
        }
        let quoted = line.trim_end_matches(',');
        let Some(path) = quoted
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
        else {
            continue;
        };
        if let Some(directory) = path.rsplit('/').next() {
            found.insert(directory.replace('-', "_"));
        }
    }
    found
}

/// The planned text: what follows the marker, up to the end of its table cell.
fn planned_text(codebook: &str) -> &str {
    let Some((_, rest)) = codebook.lines().find_map(|line| line.split_once(PLANNED)) else {
        return "";
    };
    match rest.split_once(" | ") {
        Some((cell, _)) => cell,
        None => rest,
    }
}

/// Every way `codebook` fails the plan, sorted.
fn problems(mapped: &[String], members: &BTreeSet<String>, codebook: &str) -> BTreeSet<String> {
    let planned = planned_text(codebook);
    let mut found = BTreeSet::new();
    for name in mapped {
        let ticked = format!("`{name}`");
        let row = format!("| {ticked} |");
        let has_row = codebook.lines().any(|line| line.starts_with(&row));
        let is_planned = planned.contains(&ticked);
        if members.contains(name) {
            if !has_row {
                found.insert(format!("{name} is a member and has no module-map row"));
            }
            if is_planned {
                found.insert(format!("{name} is a member and is still marked planned"));
            }
        } else if !is_planned {
            found.insert(format!("{name} is not a member and is not marked planned"));
        }
    }
    found
}

/// The committed record, manifest and codebook.
struct Tree {
    mapped: Vec<String>,
    members: BTreeSet<String>,
    codebook: String,
}

/// Reads the committed tree.
fn committed() -> Result<Tree, Box<dyn Error>> {
    let record = fs::read_to_string(root().join(RECORD))?;
    let manifest = fs::read_to_string(root().join("Cargo.toml"))?;
    let codebook = fs::read_to_string(root().join("CODEBOOK.md"))?;
    Ok(Tree {
        mapped: mapped(&record),
        members: members(&manifest),
        codebook,
    })
}

/// `codebook` with `ticked` replaced by `` `gone` `` inside the planned text only.
fn unplan(codebook: &str, ticked: &str) -> String {
    let mut out = String::with_capacity(codebook.len());
    for line in codebook.lines() {
        match line.split_once(PLANNED) {
            Some((head, rest)) => {
                out.push_str(head);
                out.push_str(PLANNED);
                out.push_str(&rest.replacen(ticked, "`gone`", 1));
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// One seed's mutated tree and the problems the model expects of it.
fn mutate(tree: &Tree, rng: &mut Rng) -> Result<(Tree, BTreeSet<String>), Box<dyn Error>> {
    let mut members = tree.members.clone();
    let mut codebook = tree.codebook.clone();
    let mut expected = BTreeSet::new();
    let mut touched = BTreeSet::new();
    let count = u32::try_from(tree.mapped.len())?;
    for _ in 0..rng.between(1, 4) {
        let pick = usize::try_from(rng.below(count))?;
        let name = tree.mapped.get(pick).ok_or("a draw past the map")?;
        if !touched.insert(name.clone()) {
            continue;
        }
        let ticked = format!("`{name}`");
        if tree.members.contains(name) {
            codebook = codebook.replace(&format!("| {ticked} |"), "| `vandal` |");
            expected.insert(format!("{name} is a member and has no module-map row"));
            continue;
        }
        match rng.below(3) {
            0 => {
                codebook = unplan(&codebook, &ticked);
                expected.insert(format!("{name} is not a member and is not marked planned"));
            }
            1 => {
                members.insert(name.clone());
                expected.insert(format!("{name} is a member and has no module-map row"));
                expected.insert(format!("{name} is a member and is still marked planned"));
            }
            _ => {
                members.insert(name.clone());
                codebook = unplan(&codebook, &ticked);
                writeln!(codebook, "| {ticked} | `crates/{name}` | landed | none |")?;
            }
        }
    }
    let mutated = Tree {
        mapped: tree.mapped.clone(),
        members,
        codebook,
    };
    Ok((mutated, expected))
}

/// One seed judged against the model; its trace is the problems it named.
fn one(tree: &Tree, seed: u64) -> Result<u64, Box<dyn Error>> {
    let mut rng = Rng::new(seed);
    let (mutated, expected) = mutate(tree, &mut rng)?;
    let found = problems(&mutated.mapped, &mutated.members, &mutated.codebook);
    assert_eq!(found, expected, "seed {seed}: the check against the model");
    let mut trace = Trace::new();
    trace.record_number("seed", seed);
    for problem in &found {
        trace.record(problem);
    }
    Ok(trace.hash())
}

/// The committed tree names every planned crate until it lands, and every
/// seeded drop, early landing, careful landing and vandal row is judged as the
/// model expects, with the same seed replaying to the same trace.
#[test]
fn sim_every_planned_crate_is_named_until_it_lands() -> TestResult {
    let tree = committed()?;
    assert!(
        tree.mapped.len() >= 8 && tree.mapped.iter().any(|name| name == "lgwks_runtime"),
        "the map is read: {:?}",
        tree.mapped
    );
    assert!(
        tree.members.contains("lgwks_std"),
        "the members are read: {:?}",
        tree.members
    );
    assert_eq!(
        problems(&tree.mapped, &tree.members, &tree.codebook),
        BTreeSet::new(),
        "the committed CODEBOOK.md names every planned crate"
    );
    for seed in 0..SEEDS {
        let first = one(&tree, seed)?;
        let again = one(&tree, seed)?;
        assert_eq!(first, again, "seed {seed} replays to the same trace");
    }
    Ok(())
}
