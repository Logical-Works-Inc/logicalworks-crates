//! Deterministic simulation of the docs.rs lane's plans: one seed draws crate
//! manifests with every shape of `[package.metadata.docs.rs]` (absent,
//! all-features on or off, a feature list or none), and each plan is held to
//! the manifest it came from. A plan never names a toolchain: a `+nightly`
//! outranks `rust-toolchain.toml`, and the hosted runner holds the pinned
//! nightly alone (ADR 0066). The same seed replays the same trace.

use std::path::Path;

use lgwks_std::seeded::{DrawError, Seeded};

use super::{doc_plan, doc_plans};
use crate::GateError;

/// Manifests drawn per seed.
const MANIFESTS: u64 = 2_000;
/// The seeds every run replays.
const SEEDS: [u64; 4] = [0, 1, 0x5EED, u64::MAX];
/// Features a docs.rs list draws from.
const FEATURES: [&str; 4] = ["hash", "json", "net", "fs"];

/// One drawn manifest and what its plan must hold.
struct Drawn {
    /// The manifest text.
    text: String,
    /// The crate's name.
    name: &'static str,
    /// Whether the manifest declares a docs.rs table at all.
    declared: bool,
    /// Whether that table sets `all-features = true`.
    all: bool,
    /// The comma-joined feature list it names, empty for none.
    features: String,
}

/// A manifest of one of every docs.rs shape, drawn from `stream`.
fn draw(stream: &mut Seeded) -> Result<Drawn, DrawError> {
    let name = match stream.below(4)? {
        0 => "lgwks_std",
        1 => "lgwks_bot",
        2 => "a-b_c",
        _ => "x",
    };
    let mut text = format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n");
    let shape = stream.below(4)?;
    let mut picked: Vec<&str> = Vec::new();
    if shape == 3 {
        for feature in FEATURES {
            if stream.below(2)? == 1 {
                picked.push(feature);
            }
        }
    }
    match shape {
        0 => {}
        1 => text.push_str("\n[package.metadata.docs.rs]\nall-features = true\n"),
        2 => text.push_str("\n[package.metadata.docs.rs]\nall-features = false\n"),
        _ => {
            let quoted: Vec<String> = picked.iter().map(|one| format!("\"{one}\"")).collect();
            text.push_str("\n[package.metadata.docs.rs]\nfeatures = [");
            text.push_str(&quoted.join(", "));
            text.push_str("]\n");
        }
    }
    Ok(Drawn {
        text,
        name,
        declared: shape != 0,
        all: shape == 1,
        features: picked.join(","),
    })
}

/// Plans every manifest of `seed`, holds each plan to its manifest, and
/// returns the trace hash of the plans.
fn simulate(seed: u64) -> Result<u64, DrawError> {
    let mut stream = Seeded::from_seed(seed);
    let mut trace: u64 = 0xCBF2_9CE4_8422_2325;
    for _ in 0..MANIFESTS {
        let drawn = draw(&mut stream)?;
        let plan = doc_plan(&drawn.text);
        assert_eq!(
            plan.is_some(),
            drawn.declared,
            "seed {seed}: {}",
            drawn.text
        );
        let Some((label, command)) = plan else {
            continue;
        };
        assert!(
            command.iter().all(|arg| !arg.starts_with('+')),
            "seed {seed}: a plan named a toolchain: {command:?}"
        );
        let head = ["cargo", "doc", "--no-deps", "--locked", "-p", drawn.name];
        assert!(
            command.len() >= head.len() && command.iter().zip(head).all(|(arg, want)| arg == want),
            "seed {seed}: {command:?}"
        );
        assert_eq!(
            command.iter().any(|arg| arg == "--all-features"),
            drawn.all,
            "seed {seed}: {}",
            drawn.text
        );
        let listed = command
            .iter()
            .position(|arg| arg == "--features")
            .and_then(|at| command.get(at.saturating_add(1)));
        let expected = (!drawn.all && !drawn.features.is_empty()).then_some(&drawn.features);
        assert_eq!(listed, expected, "seed {seed}: {}", drawn.text);
        assert!(label.starts_with(drawn.name), "seed {seed}: {label}");
        for byte in command.join(" ").bytes() {
            trace = (trace ^ u64::from(byte)).wrapping_mul(0x0100_0000_01B3);
        }
    }
    Ok(trace)
}

#[test]
fn sim_docsrs_plans_name_no_toolchain_and_follow_the_manifest() -> Result<(), DrawError> {
    for seed in SEEDS {
        let first = simulate(seed)?;
        assert_eq!(first, simulate(seed)?, "seed {seed} replays its trace");
    }
    Ok(())
}

#[test]
fn sim_docsrs_plans_of_this_repository_name_no_toolchain() -> Result<(), GateError> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let plans = doc_plans(&root)?;
    assert!(!plans.is_empty(), "the workspace declares docs.rs metadata");
    for plan in &plans {
        assert!(
            plan.1.iter().all(|arg| !arg.starts_with('+')),
            "{}: {:?}",
            plan.0,
            plan.1
        );
    }
    Ok(())
}
