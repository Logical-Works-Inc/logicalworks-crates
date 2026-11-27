//! The meta prover against hostile registers (#1164, ADR 0091).
//!
//! One seed draws a register: each catalog kind absent, present or repeated,
//! a kind the catalog lacks, owners blank or named, and proofs that name a
//! required lane, an optional one, a lane that is not there, a real harness,
//! one no required Kani lane runs, a function that is no harness, a real test
//! and a test that is gone. The oracle is a model of the rule written from
//! ADR 0091, not the prover's own answer: the set of kinds it calls unproven
//! and the number of gaps must match.

use std::collections::{BTreeMap, BTreeSet};

use lgwks_std::seeded::{DrawError, Seeded};

use super::{CATALOG, Lane, World, judge};
use crate::toml_subset::read_tables;

/// Registers drawn.
const SEEDS: u64 = 2_000;

/// A closure lane: its id, and whether the model counts it as a proof.
const CLOSURES: [(&str, bool); 3] = [("lint", true), ("advisory", false), ("gone", false)];

/// A model harness and whether a required Kani lane runs a real harness of that name.
const MODELS: [(&str, bool); 3] = [
    ("ledger_fold_proof", true),
    ("soft_only_proof", false),
    ("not_a_harness", false),
];

/// A census test and whether it resolves.
const CENSUSES: [(&str, bool); 2] = [("census_reads_the_table", true), ("census_deleted", false)];

/// The lanes every drawn register is judged against.
fn lanes() -> BTreeMap<String, Lane> {
    let lane = |required: bool, command: &str| Lane {
        required,
        command: command.to_owned(),
    };
    BTreeMap::from([
        ("lint".to_owned(), lane(true, "cargo clippy")),
        ("advisory".to_owned(), lane(false, "cargo clippy")),
        (
            "kani".to_owned(),
            lane(true, "cargo kani -p lgwks_std --harness ledger_"),
        ),
        (
            "kani-soft".to_owned(),
            lane(false, "cargo kani -p lgwks_std --harness soft_"),
        ),
    ])
}

/// One list of names drawn from `pool`, and whether every name proves.
fn draw_list(rng: &mut Seeded, pool: &[(&str, bool)]) -> Result<(String, usize), DrawError> {
    let mut names = Vec::new();
    let mut false_ones = 0_usize;
    for _ in 0..rng.below(3)? {
        let (name, proves) = *pool
            .get(rng.index(pool.len())?)
            .ok_or(DrawError::EmptyRange)?;
        if !proves {
            false_ones = false_ones.saturating_add(1);
        }
        names.push(format!("\"{name}\""));
    }
    // An empty list is one gap of its own.
    let gaps = if names.is_empty() { 1 } else { false_ones };
    Ok((format!("[{}]", names.join(", ")), gaps))
}

/// One register entry from its kind, owner, and closure, model and census lists.
fn entry([kind, owner, closure, model, census]: [&str; 5]) -> String {
    format!(
        "[[resource]]\nkind = \"{kind}\"\nowner = \"{owner}\"\nclosure = {closure}\nmodel = {model}\ncensus = {census}\n\n"
    )
}

/// One seed's register text, with the model's gapped kinds and gap count.
fn draw(rng: &mut Seeded) -> Result<(String, BTreeSet<String>, usize), DrawError> {
    let mut text = String::new();
    let mut gapped = BTreeSet::new();
    let mut count = 0_usize;
    let mut kinds: Vec<&str> = CATALOG.to_vec();
    kinds.push("gpu-buffer");
    for kind in kinds {
        let copies = rng.below(4)?.saturating_sub(1);
        if copies == 0 && CATALOG.contains(&kind) {
            gapped.insert(kind.to_owned());
            count = count.saturating_add(1);
        }
        for copy in 0..copies {
            let owner = if rng.below(5)? == 0 {
                ""
            } else {
                "lgwks_std::process"
            };
            let (closure, closure_gaps) = draw_list(rng, &CLOSURES)?;
            let (model, model_gaps) = draw_list(rng, &MODELS)?;
            let (census, census_gaps) = draw_list(rng, &CENSUSES)?;
            text.push_str(&entry([kind, owner, &closure, &model, &census]));
            let gaps = if !CATALOG.contains(&kind) || copy > 0 {
                1
            } else {
                usize::from(owner.is_empty())
                    .saturating_add(closure_gaps)
                    .saturating_add(model_gaps)
                    .saturating_add(census_gaps)
            };
            if gaps > 0 {
                gapped.insert(kind.to_owned());
            }
            count = count.saturating_add(gaps);
        }
    }
    Ok((text, gapped, count))
}

/// The prover calls a kind unproven exactly when the model does, with as many
/// gaps, over 2,000 hostile registers; and a register where every kind holds
/// all three proofs passes.
#[test]
fn sim_prove_judge_matches_the_model_over_hostile_registers()
-> Result<(), Box<dyn std::error::Error>> {
    let lanes = lanes();
    let harnesses: BTreeSet<String> = ["ledger_fold_proof", "soft_only_proof"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let resolves = |name: &str| name == "census_reads_the_table";
    let world = World {
        lanes: &lanes,
        harnesses: &harnesses,
        resolves: &resolves,
    };
    for seed in 0..SEEDS {
        let mut rng = Seeded::from_seed(seed);
        let (text, gapped, count) = draw(&mut rng)?;
        let gaps = judge(&read_tables(&text), &world);
        let named: BTreeSet<String> = gaps.iter().map(|gap| gap.kind.clone()).collect();
        assert_eq!(named, gapped, "seed {seed}: {gaps:?}\n{text}");
        assert_eq!(gaps.len(), count, "seed {seed}: {gaps:?}\n{text}");
    }
    let mut whole = String::new();
    for kind in CATALOG {
        whole.push_str(&entry([
            kind,
            "o",
            "[\"lint\"]",
            "[\"ledger_fold_proof\"]",
            "[\"census_reads_the_table\"]",
        ]));
    }
    assert_eq!(judge(&read_tables(&whole), &world), Vec::new());
    Ok(())
}

/// The live register against the live tree: the process kind names a Kani
/// model, and every model any kind names is a `#[kani::proof]` function that
/// the `kani` lane's `--harness` selects, so that lane verifies what the
/// register claims.
#[test]
fn sim_prove_the_live_register_models_the_process_kind() -> Result<(), crate::GateError> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tables = read_tables(&crate::read(&root.join(super::REGISTER))?);
    let lanes = super::lanes_of(&crate::read(&root.join("scripts/gate-lanes.toml"))?);
    let harnesses = crate::modules::kani_harnesses(&root)?;
    let filters = match lanes.get("kani") {
        Some(kani) => super::harness_filters(&kani.command),
        None => Vec::new(),
    };
    assert!(
        !filters.is_empty(),
        "scripts/gate-lanes.toml has no kani lane that selects a harness"
    );
    let entries: Vec<_> = tables
        .iter()
        .filter(|table| table.path == "resource" && table.array)
        .collect();
    let kinds: Vec<_> = entries
        .iter()
        .filter_map(|table| crate::toml_subset::text(table, "kind").map(|kind| (kind, *table)))
        .collect();
    assert_eq!(kinds.len(), entries.len(), "an entry names no kind");
    let mut modelled = BTreeSet::new();
    for (kind, table) in kinds {
        for harness in super::names(table, "model") {
            assert!(
                harnesses.contains(&harness),
                "{kind}: `{harness}` is no proof"
            );
            assert!(
                filters.iter().any(|filter| harness.contains(filter)),
                "{kind}: the kani lane does not run `{harness}`"
            );
            modelled.insert(kind.to_owned());
        }
    }
    assert!(
        modelled.contains("process"),
        "the process kind names no Kani model: {modelled:?}"
    );
    Ok(())
}
