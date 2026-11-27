//! The `prove` lane, the meta prover (#1164, ADR 0091): every kind of resource
//! a process can acquire through the crates holds three proofs, or the lane
//! names the kind and the proof it lacks.
//!
//! The kinds are [`CATALOG`], code and not data: a register cannot drop one.
//! `contract/RESOURCES.toml` names, for each kind, its one owner and its proofs:
//!
//! - `closure`: required lanes that refuse an acquisition of the kind outside
//!   its owner;
//! - `model`: `#[kani::proof]` harnesses on the owner's real code, each run by
//!   a required lane whose command is `cargo kani` with a `--harness` that
//!   selects it, so Kani's `VERIFIED` is that lane's verdict;
//! - `census`: tests that drive the real code to its end, SIGKILL included,
//!   and read the OS table the kind lives in.
//!
//! This lane reads names and lanes and runs nothing. The Kani lanes and the
//! test lanes run the proofs, and a proof that fails fails its own lane.

use std::collections::{BTreeMap, BTreeSet};

use super::builtins::{pass, refuse};
use super::invariants::resolution_candidates;
use super::modules::{collect_module_paths, kani_harnesses};
use super::toml_subset::{Table, list, raw, read_tables, text};
use super::{Context, GateError, Verdict, read};

#[cfg(test)]
mod sim_prove;

/// Every kind of resource a process acquires through the crates and must end
/// holding none of: one per OS table a census reads.
pub(super) const CATALOG: [&str; 8] = [
    "process",
    "mount",
    "lease",
    "temp",
    "descriptor",
    "heap",
    "thread",
    "daemon",
];

/// Where the register lives, from the repository root.
const REGISTER: &str = "contract/RESOURCES.toml";

/// One lane as the prover reads it.
pub(super) struct Lane {
    /// Whether CI refuses a change the lane fails.
    pub(super) required: bool,
    /// The command it runs, empty for a builtin.
    pub(super) command: String,
}

/// What the register's names are judged against.
pub(super) struct World<'a> {
    /// Every lane, by id.
    pub(super) lanes: &'a BTreeMap<String, Lane>,
    /// Every function under `#[kani::proof]`.
    pub(super) harnesses: &'a BTreeSet<String>,
    /// Whether a test name resolves to a test the sources define.
    pub(super) resolves: &'a dyn Fn(&str) -> bool,
}

/// A kind and what keeps it from proven.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Gap {
    /// The kind, or the register entry's spelling of one.
    pub(super) kind: String,
    /// The missing or false proof.
    pub(super) why: String,
}

/// The `--harness` values a `cargo kani` command selects; none for any other.
fn harness_filters(command: &str) -> Vec<&str> {
    if !command.contains("cargo kani") {
        return Vec::new();
    }
    let words: Vec<&str> = command.split_whitespace().collect();
    words
        .windows(2)
        .filter_map(|pair| match *pair {
            ["--harness", filter] => Some(filter),
            _ => None,
        })
        .collect()
}

/// Whether a required Kani lane runs `harness`: Kani selects every harness
/// whose name contains a `--harness` value.
fn run_by_kani(world: &World<'_>, harness: &str) -> bool {
    world.lanes.values().any(|lane| {
        lane.required
            && harness_filters(&lane.command)
                .iter()
                .any(|filter| harness.contains(filter))
    })
}

/// The names under `key` of an entry; none when it lists none.
fn names(table: &Table, key: &str) -> Vec<String> {
    match list(table, key) {
        Some(found) => found.to_vec(),
        None => Vec::new(),
    }
}

/// The gaps of the one entry of `kind`.
fn judge_entry(kind: &str, table: &Table, world: &World<'_>, gaps: &mut Vec<Gap>) {
    let mut gap = |why: String| {
        gaps.push(Gap {
            kind: kind.to_owned(),
            why,
        });
    };
    if text(table, "owner").is_none_or(str::is_empty) {
        gap("names no owner module".to_owned());
    }
    let closure = names(table, "closure");
    if closure.is_empty() {
        gap("no closure lane refuses an acquisition outside its owner".to_owned());
    }
    for id in &closure {
        match world.lanes.get(id) {
            Some(lane) if lane.required => {}
            Some(_) => gap(format!("closure lane `{id}` is not required")),
            None => gap(format!(
                "closure lane `{id}` is not in scripts/gate-lanes.toml"
            )),
        }
    }
    let model = names(table, "model");
    if model.is_empty() {
        gap("no Kani harness models its owner".to_owned());
    }
    for harness in &model {
        if !world.harnesses.contains(harness) {
            gap(format!("`{harness}` is no `#[kani::proof]` function"));
        } else if !run_by_kani(world, harness) {
            gap(format!("no required `cargo kani` lane runs `{harness}`"));
        }
    }
    let census = names(table, "census");
    if census.is_empty() {
        gap("no census reads the OS table it lives in".to_owned());
    }
    for test in &census {
        if !(world.resolves)(test) {
            gap(format!("census `{test}` names no test"));
        }
    }
}

/// A gap of `kind` for `why`.
fn gap_of(kind: &str, why: &str) -> Gap {
    Gap {
        kind: kind.to_owned(),
        why: why.to_owned(),
    }
}

/// Every gap of the register `tables` against `world`: a catalog kind with no
/// entry or a false proof, and an entry that is no catalog kind or repeats one.
pub(super) fn judge(tables: &[Table], world: &World<'_>) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let mut seen = BTreeSet::new();
    for table in tables
        .iter()
        .filter(|table| table.path == "resource" && table.array)
    {
        let Some(kind) = text(table, "kind") else {
            gaps.push(gap_of("?", "an entry names no kind"));
            continue;
        };
        if !CATALOG.contains(&kind) {
            gaps.push(gap_of(kind, "is not a catalog kind; the catalog is code"));
        } else if !seen.insert(kind.to_owned()) {
            gaps.push(gap_of(kind, "has two entries"));
        } else {
            judge_entry(kind, table, world, &mut gaps);
        }
    }
    for kind in CATALOG {
        if !seen.contains(kind) {
            gaps.push(gap_of(kind, &format!("has no entry in {REGISTER}")));
        }
    }
    gaps.sort();
    gaps
}

/// Every lane of `scripts/gate-lanes.toml`, by id.
fn lanes_of(table_text: &str) -> BTreeMap<String, Lane> {
    let mut lanes = BTreeMap::new();
    for table in read_tables(table_text)
        .iter()
        .filter(|table| table.path == "lane" && table.array)
    {
        let Some(id) = text(table, "id") else {
            continue;
        };
        let command = match text(table, "command") {
            Some(command) => command.to_owned(),
            None => String::new(),
        };
        let lane = Lane {
            required: raw(table, "required") == Some("true"),
            command,
        };
        lanes.insert(id.to_owned(), lane);
    }
    lanes
}

/// The `prove` builtin: every catalog kind holds its three proofs.
pub(super) fn prove(context: &Context<'_>) -> Result<Verdict, GateError> {
    let root = context.root;
    let tables = read_tables(&read(&root.join(REGISTER))?);
    let lanes = lanes_of(&read(&root.join("scripts/gate-lanes.toml"))?);
    let harnesses = kani_harnesses(root)?;
    let modules = collect_module_paths(root)?;
    let resolves = |name: &str| !resolution_candidates(&modules, name).is_empty();
    let world = World {
        lanes: &lanes,
        harnesses: &harnesses,
        resolves: &resolves,
    };
    let gaps = judge(&tables, &world);
    if gaps.is_empty() {
        return Ok(pass(&format!(
            "all {} resource kinds hold a closure lane, a Kani model and a census",
            CATALOG.len()
        )));
    }
    let gapped: BTreeSet<&str> = gaps.iter().map(|gap| gap.kind.as_str()).collect();
    let lines: Vec<String> = gaps
        .iter()
        .map(|gap| format!("GAP {}: {}", gap.kind, gap.why))
        .collect();
    // One line: the summary table shows a lane's detail up to its first break.
    Ok(refuse(format!(
        "{} of {} resource kinds are not proven: {}",
        gapped.len(),
        CATALOG.len(),
        lines.join("; ")
    )))
}
