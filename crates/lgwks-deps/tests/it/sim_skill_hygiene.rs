//! The `skill-hygiene` lane, driven through the shipped `ci-local` binary on
//! seeded skill trees and seeded harness homes (ADR 0084, INV-DOC-10).
//!
//! Source: the Agent Skills specification (agentskills.io): a harness loads a
//! skill from its frontmatter alone, `name` 1 to 64 of a-z, 0-9 and `-`, equal
//! to its folder, `description` at most 1,024 characters. YAML 1.2 section 7.3.3:
//! a plain scalar cannot hold `: ` or ` #`. rustdoc compiles an untagged fence
//! as Rust. Requirement proved: every planted fault fails the lane with its
//! own words, a healthy tree passes, and one seed is one tree.

use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::seed::Rng;

type TestResult = Result<(), Box<dyn Error>>;

/// How many trees the sweep draws. Each is two runs of the binary.
const SEEDS: u64 = 51;

/// One planted fault and the words its finding must carry, spelled here and
/// not taken from the checker, so a checker that rewords goes red.
const FAULTS: [&str; 17] = [
    "is not its folder",
    "does not open with `Use `",
    "YAML reads as a map",
    "past 1024",
    "a fence with no language",
    "never compiled",
    "does not route to it",
    "past 120",
    "not in the specification",
    "no frontmatter",
    "`ignore`d",
    "holds no SKILL.md",
    "another name",
    "`<` or `>`",
    "never closed",
    "is not 1 to 64",
    "no SKILL.md",
];

/// A healthy skill page named `name`.
fn healthy(name: &str, rust: bool) -> String {
    let block = if rust {
        "```rust\nlet total = 1 + 1;\nassert_eq!(total, 2);\n```\n"
    } else {
        "```text\nshape\n```\n"
    };
    format!(
        "---\nname: {name}\ndescription: Use when the seed asks for {name}, and only then.\n---\n\n# {name}\n\nOne line.\n\n{block}"
    )
}

/// Writes `text` at `relative` under `root`, making directories.
fn put(root: &Path, relative: &str, text: &str) -> TestResult {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)?;
    Ok(())
}

/// Links `link` to `target`, making the link's directory.
fn link(link: &Path, target: &Path) -> TestResult {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

/// A scratch directory for one seed, empty, by its real path.
fn scratch(seed: u64, fault: Option<usize>) -> Result<PathBuf, Box<dyn Error>> {
    let tag = match fault {
        Some(kind) => kind.to_string(),
        None => "clean".to_owned(),
    };
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("skill-hygiene-{seed}-{tag}"));
    if path.exists() {
        fs::remove_dir_all(&path)?;
    }
    fs::create_dir_all(&path)?;
    Ok(fs::canonicalize(path)?)
}

/// The page of skill `first` with `fault` planted, and what the plant changes
/// around it: the route, the doctest home and the harness home.
fn plant(
    fault: Option<usize>,
    first: &str,
    seed: u64,
    (root, home): (&Path, &Path),
    (route, doctests): (&mut String, &mut String),
) -> Result<String, Box<dyn Error>> {
    let text = healthy(first, true);
    Ok(match fault {
        Some(0) => text.replacen(&format!("name: {first}"), "name: lgwks-other", 1),
        Some(1) => text.replacen("description: Use when", "description: Load when", 1),
        Some(2) => text.replacen("and only then.", "and only then: always.", 1),
        Some(3) => text.replacen("only then.", &"x".repeat(1100), 1),
        Some(4) => text.replacen("```rust", "```", 1),
        Some(5) => {
            *doctests = doctests.replacen(&format!("skills/{first}/"), "skills/gone/", 1);
            text
        }
        Some(6) => {
            *route = route.replace(&format!("skills/{first}/SKILL.md"), "skills/gone/SKILL.md");
            text
        }
        Some(7) => format!("{text}{}", "line\n".repeat(120)),
        Some(8) => text.replacen("description:", "descripton: x\ndescription:", 1),
        Some(9) => text.replacen("---\nname", "name", 1),
        Some(10) => text.replacen("```rust", "```rust,ignore", 1),
        Some(11) => {
            let gone = root.join("skills/lgwks-gone");
            link(&home.join(".config/opencode/skills/lgwks-gone"), &gone)?;
            text
        }
        Some(12) => {
            let own = root.join("skills").join(first);
            link(&home.join(".commandcode/skills/lgwks-alias"), &own)?;
            text
        }
        Some(13) => text.replacen("only then.", "only <then>.", 1),
        Some(14) => format!("{text}```sh\nopen\n"),
        Some(15) => {
            let upper = format!("Lgwks-Seed-{seed}");
            put(
                root,
                &format!("skills/{upper}/SKILL.md"),
                &healthy(&upper, false),
            )?;
            writeln!(route, "skills/{upper}/SKILL.md")?;
            text
        }
        Some(16) => {
            fs::create_dir_all(root.join("skills/lgwks-empty"))?;
            text
        }
        _ => text,
    })
}

/// The tree and the home one seed names, with `fault` planted in its first
/// skill, and the binary's exit code and output on them.
fn judge(seed: u64, fault: Option<usize>) -> Result<(Option<i32>, String), Box<dyn Error>> {
    let mut rng = Rng::new(seed);
    let base = scratch(seed, fault)?;
    let (root, home) = (base.join("tree"), base.join("home"));
    let names: Vec<String> = (0..rng.between(1, 4))
        .map(|index| format!("lgwks-seed-{seed}-{index}"))
        .collect();
    let mut route = String::from("# Skills\n\n");
    let mut doctests = String::from("//! doctests\n");
    for (index, name) in names.iter().enumerate() {
        let rust = index == 0 || rng.below(2) == 0;
        put(
            &root,
            &format!("skills/{name}/SKILL.md"),
            &healthy(name, rust),
        )?;
        writeln!(
            route,
            "| x | [`skills/{name}/SKILL.md`](skills/{name}/SKILL.md) |"
        )?;
        writeln!(
            doctests,
            "#[doc = include_str!(\"../../../skills/{name}/SKILL.md\")]"
        )?;
        let installed = home.join(".agents/skills").join(name);
        link(&installed, &root.join("skills").join(name))?;
        link(&home.join(".claude/skills").join(name), &installed)?;
    }
    fs::create_dir_all(home.join(".commandcode/skills/somebody-elses"))?;
    let first = names.first().ok_or("a tree with no skill")?;
    let page = plant(
        fault,
        first,
        seed,
        (&root, &home),
        (&mut route, &mut doctests),
    )?;
    put(&root, &format!("skills/{first}/SKILL.md"), &page)?;
    put(&root, "SKILLS.md", &route)?;
    put(&root, "crates/lgwks-bot/src/lib.rs", &doctests)?;
    put(
        &root,
        "scripts/gate-lanes.toml",
        "[toolchain]\nrust = \"0.0.0\"\n\n[[lane]]\nid = \"skill-hygiene\"\nsurfaces = \"local\"\nrequired = true\nfast = true\nbuiltin = \"skill-hygiene\"\n",
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_ci-local"))
        .args([
            "--root",
            &root.to_string_lossy(),
            "--lane",
            "skill-hygiene",
            "--jobs",
            "1",
        ])
        .env("HOME", &home)
        .output()?;
    fs::remove_dir_all(&base)?;
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok((output.status.code(), said))
}

/// Catches a lane that passes a broken skill, refuses a healthy one, or names
/// a fault in words other than its own: every kind is planted three times.
#[test]
fn sim_skill_hygiene_matches_the_rules() -> TestResult {
    for seed in 0..SEEDS {
        let (code, said) = judge(seed, None)?;
        assert_eq!(code, Some(0), "seed {seed}: a healthy tree passes: {said}");
        let kind = usize::try_from(seed)?
            .checked_rem(FAULTS.len())
            .ok_or("an empty fault table")?;
        let words = FAULTS.get(kind).ok_or("a fault past the table")?;
        let (code, said) = judge(seed, Some(kind))?;
        assert_eq!(
            code,
            Some(1),
            "seed {seed}: fault {kind} fails the lane: {said}"
        );
        assert!(
            said.contains(words),
            "seed {seed}: fault {kind} names {words:?}: {said}"
        );
    }
    Ok(())
}

/// The tree as committed passes its own lane, which is what CI runs.
#[test]
fn the_repository_skills_pass_the_lane() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(env!("CARGO_BIN_EXE_ci-local"))
        .args([
            "--root",
            &root.to_string_lossy(),
            "--lane",
            "skill-hygiene",
            "--jobs",
            "1",
        ])
        .env("HOME", Path::new(env!("CARGO_TARGET_TMPDIR")))
        .output()?;
    let said = String::from_utf8_lossy(&output.stdout).into_owned();
    let err = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(0), "{said}{err}");
    Ok(())
}
