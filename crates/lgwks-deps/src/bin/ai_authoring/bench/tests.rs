//! The committed manifest and the refusals `Bench::parse` owes a damaged one.

#[cfg(test)]
use std::path::Path;

use super::{Bench, Setting, fill};
use crate::sim_kit::{BENCH_DIR, COMMITTED, committed};

/// Whether parsing `edited` is refused with a message naming `needle`.
fn refused(edited: &str, needle: &str) -> bool {
    match Bench::parse(Path::new(BENCH_DIR), edited) {
        Ok(_) => false,
        Err(error) => error.to_string().contains(needle),
    }
}

/// bench/ai-authoring/README.md and tasks.md: six arms, six tasks, five
/// required profiles, and the three cells the old and futures sheets cannot
/// express (#270).
#[test]
fn the_committed_manifest_declares_the_benchmark_the_readme_describes()
-> Result<(), Box<dyn std::error::Error>> {
    let bench = committed()?;
    let arms: Vec<&str> = bench.arms.iter().map(|arm| arm.name.as_str()).collect();
    assert_eq!(
        arms,
        ["old", "new", "fan", "futures", "script", "lexicon"],
        "the six arms, in the order the README tables list them"
    );
    assert_eq!(bench.tasks.len(), 6, "six held-out tasks");
    assert_eq!(bench.profiles.len(), 5, "the five required user profiles");
    assert_eq!(
        bench.exclusions.len(),
        3,
        "three cells no sheet can express"
    );
    assert_eq!(
        bench.defaults.trials, 1,
        "one trial per profile: two trials of one cell put the same prompt twice, a repeat in meaning the Director's order of 2026-10-17 forbids"
    );
    assert!(
        bench.limits.token_budget < 10_000_000
            && bench.limits.token_budget >= bench.limits.call_tokens,
        "the budget is under the Director's 10M tokens and holds at least one reply"
    );
    assert!(
        bench.excludes("old", "capture") && bench.excludes("futures", "recovery"),
        "the old sheet has no process spawn and the futures sheet no durable store"
    );
    assert!(
        bench
            .protocol
            .iter()
            .any(|statement| statement.0 == "hidden_oracle"
                && matches!(statement.1, Setting::Flag(true))),
        "the protocol states the oracle is hidden as a flag"
    );
    Ok(())
}

/// The lexicon arm authors `script!` from the generated lexicon, and the fan
/// arm's recovery reference is a new-surface solution (trial.py, 2026-10-11).
#[test]
fn a_reference_comes_from_the_arm_then_from_what_it_borrows()
-> Result<(), Box<dyn std::error::Error>> {
    let bench = committed()?;
    let lexicon = bench.arm("lexicon").ok_or("no lexicon arm")?;
    let fan = bench.arm("fan").ok_or("no fan arm")?;
    assert_eq!(
        bench.reference_surface(lexicon, "aggregate"),
        "script",
        "the lexicon arm compiles the script reference"
    );
    assert_eq!(
        bench.reference_surface(fan, "recovery"),
        "new",
        "fan borrows the new reference for recovery"
    );
    assert_eq!(
        bench.reference_surface(fan, "aggregate"),
        "fan",
        "fan keeps its own reference elsewhere"
    );
    Ok(())
}

/// A benchmark that names a file that is not there fails its first trial,
/// hours into a run; every file the manifest and its layout name must exist.
#[test]
fn every_file_the_manifest_names_exists() -> Result<(), Box<dyn std::error::Error>> {
    let bench = committed()?;
    let mut missing = Vec::new();
    let mut want = |path: std::path::PathBuf| {
        if !path.is_file() {
            missing.push(path.display().to_string());
        }
    };
    want(bench.path(&bench.layout.lock));
    want(bench.path(&bench.layout.template).join("Cargo.toml"));
    for arm in &bench.arms {
        want(bench.path(&arm.sheet));
    }
    for task in &bench.tasks {
        want(bench.path(&fill(&bench.layout.prompt, &[("task", &task.name)])));
        want(bench.path(&fill(&bench.layout.oracle, &[("task", &task.name)])));
        want(bench.path(&fill(&bench.layout.mutant, &[("task", &task.name)])));
        want(bench.path(&fill(
            &bench.layout.reference,
            &[("surface", &task.mutant_base), ("task", &task.name)],
        )));
        for arm in &bench.arms {
            if !bench.excludes(&arm.name, &task.name) {
                let surface = bench.reference_surface(arm, &task.name);
                want(bench.path(&fill(
                    &bench.layout.reference,
                    &[("surface", &surface), ("task", &task.name)],
                )));
            }
        }
    }
    for entry in &bench.template.paths {
        if !bench.path(&entry.target).is_dir() {
            missing.push(entry.target.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "the manifest names files that are not there: {missing:?}"
    );
    Ok(())
}

#[test]
fn a_name_declared_twice_is_refused() {
    assert!(
        refused(
            &COMMITTED.replacen("name = \"fan\"", "name = \"old\"", 1),
            "declared twice"
        ),
        "two arms called old"
    );
}

/// A name becomes a directory, a package and a file name, so one with a
/// separator or a dot-dot would let a manifest reach outside its work root.
#[test]
fn a_name_that_could_leave_its_directory_is_refused() {
    assert!(
        refused(
            &COMMITTED.replacen("name = \"fan\"", "name = \"../escape\"", 1),
            "use only"
        ),
        "a path separator in an arm name"
    );
    assert!(
        refused(
            &COMMITTED.replacen("name = \"fan\"", "name = \"..\"", 1),
            "use only"
        ),
        "a dot-dot arm name"
    );
}

#[test]
fn a_missing_key_is_refused_naming_it() {
    assert!(
        refused(&COMMITTED.replacen("framing =", "framed =", 1), "framing"),
        "the reply contract is required"
    );
}

#[test]
fn an_exclusion_of_an_undeclared_cell_is_refused() {
    let edited = COMMITTED.replacen(
        "task = \"recovery\"\narm = \"old\"",
        "task = \"nonesuch\"\narm = \"old\"",
        1,
    );
    assert!(
        refused(&edited, "does not declare"),
        "an exclusion of a task nobody declared"
    );
}

#[test]
fn a_limit_that_is_not_a_positive_whole_number_is_refused() {
    // The committed value is the one replaced, so a limit changed in the
    // manifest cannot leave this edit a no-op that the test then misreads.
    let edited = COMMITTED.replacen("oracle_s = 300", "oracle_s = 0", 1);
    assert_ne!(
        edited, COMMITTED,
        "the committed manifest declares `oracle_s = 300`"
    );
    assert!(
        refused(&edited, "oracle_s"),
        "an oracle bound of zero seconds would time out every trial"
    );
}

#[test]
fn fill_replaces_every_named_placeholder_and_only_those() {
    assert_eq!(
        fill("tasks/{task}/{task}.rs {other}", &[("task", "aggregate")]),
        "tasks/aggregate/aggregate.rs {other}",
        "each occurrence of a named placeholder is replaced and an unnamed one is left"
    );
}
