//! The meta prover as a user runs it (#1164, ADR 0091): the shipped
//! `ci-local --lane prove` over a repository whose register leaves one kind
//! short of a proof, then over the same repository with the proof in place.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

/// The eight kinds of ADR 0091, in the prover's order.
const KINDS: [&str; 8] = [
    "process",
    "mount",
    "lease",
    "temp",
    "descriptor",
    "heap",
    "thread",
    "daemon",
];

/// A crate whose sources hold one Kani harness, one Kani function no lane
/// runs, and the census tests the register names.
const LIB: &str = "#[cfg(kani)]\nmod proof {\n    #[kani::proof]\n    fn ledger_fold_holds() {}\n\n    #[kani::proof]\n    fn stray_unrun_harness() {}\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn census_reads_the_process_table() {}\n}\n";

/// The lanes: the prover itself, a required closure lane, a required Kani
/// lane that selects `ledger_` harnesses, and an optional one.
const LANES: &str = concat!(
    "[[lane]]\nid = \"prove\"\nsurfaces = \"local\"\nrequired = true\nbuiltin = \"prove\"\n\n",
    "[[lane]]\nid = \"lint\"\nsurfaces = \"local\"\nrequired = true\n",
    "command = \"cargo clippy --workspace\"\n\n",
    "[[lane]]\nid = \"kani\"\nsurfaces = \"local\"\nrequired = true\n",
    "command = \"cargo kani -p fixture --harness ledger_\"\n\n",
    "[[lane]]\nid = \"kani-soft\"\nsurfaces = \"local\"\nrequired = false\n",
    "command = \"cargo kani -p fixture --harness stray_\"\n",
);

/// A register entry for `kind` with `model` as its one harness and `census`
/// as its one test.
fn entry(kind: &str, model: &str, census: &str) -> String {
    format!(
        "[[resource]]\nkind = \"{kind}\"\nowner = \"fixture::owner\"\nclosure = [\"lint\"]\nmodel = [\"{model}\"]\ncensus = [\"{census}\"]\n\n"
    )
}

/// A fixture repository under the test's own temp directory.
fn fixture(case: &str, register: &str) -> Result<PathBuf, Box<dyn Error>> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("prove-lane-{case}"));
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(root.join("crates/fixture/src"))?;
    fs::create_dir_all(root.join("scripts"))?;
    fs::create_dir_all(root.join("contract"))?;
    fs::write(root.join("crates/fixture/src/lib.rs"), LIB)?;
    fs::write(
        root.join("crates/fixture/Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n",
    )?;
    fs::write(root.join("scripts/gate-lanes.toml"), LANES)?;
    fs::write(root.join("contract/RESOURCES.toml"), register)?;
    Ok(root)
}

/// `ci-local --root <root> --lane prove`: its status and everything it printed.
fn prove(root: &Path) -> Result<(Option<i32>, String), Box<dyn Error>> {
    let ran = Command::new(env!("CARGO_BIN_EXE_ci-local"))
        .args(["--root"])
        .arg(root)
        .args(["--lane", "prove"])
        .output()?;
    let mut said = String::from_utf8_lossy(&ran.stdout).into_owned();
    said.push_str(&String::from_utf8_lossy(&ran.stderr));
    Ok((ran.status.code(), said))
}

/// A register that leaves `daemon` with a harness no required Kani lane runs
/// and `heap` with a census that names no test fails the lane, naming those
/// two kinds and no other. The same register with both proofs in place passes.
#[test]
fn sim_prove_names_every_unproven_kind_and_only_those() -> TestResult {
    let mut short = String::new();
    for kind in KINDS {
        let (model, census) = match kind {
            "daemon" => ("stray_unrun_harness", "census_reads_the_process_table"),
            "heap" => ("ledger_fold_holds", "census_that_was_deleted"),
            _ => ("ledger_fold_holds", "census_reads_the_process_table"),
        };
        short.push_str(&entry(kind, model, census));
    }
    let (status, said) = prove(&fixture("short", &short)?)?;
    assert_ne!(
        status,
        Some(0),
        "a register with two unproven kinds passed:\n{said}"
    );
    assert!(
        said.contains("GAP daemon"),
        "the daemon gap went unnamed:\n{said}"
    );
    assert!(
        said.contains("GAP heap"),
        "the heap gap went unnamed:\n{said}"
    );
    assert!(
        said.contains("2 of 8 resource kinds are not proven"),
        "the lane named a proven kind:\n{said}"
    );
    for kind in KINDS
        .iter()
        .filter(|kind| !matches!(**kind, "daemon" | "heap"))
    {
        assert!(
            !said.contains(&format!("GAP {kind}:")),
            "the lane named the proven {kind}:\n{said}"
        );
    }

    let mut whole = String::new();
    for kind in KINDS {
        whole.push_str(&entry(
            kind,
            "ledger_fold_holds",
            "census_reads_the_process_table",
        ));
    }
    let (status, said) = prove(&fixture("whole", &whole)?)?;
    assert_eq!(
        status,
        Some(0),
        "a register with every proof in place failed:\n{said}"
    );
    Ok(())
}
