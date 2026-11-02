//! The `lgwks expand` path: determinism, macro equivalence and clippy.
//!
//! The expansion is the one emitter's answer, formatted: the same function
//! `script!` compiles through, so the text the tool prints is byte-for-byte
//! what the macro compiles. One seed expands the reference app twice and
//! requires the same text; the clippy family puts that text in a scratch
//! crate and runs the workspace lint contract on it, so the printed Rust
//! ships as hand-written code would.

use std::error::Error;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use crate::lgwks_scratch::{Scratch, app_target, reference_app, run_lgwks, workspace_root};
use crate::replay::{Failure, refuse, replay_sweep};
use crate::sim::{Rng, Trace};

/// What every test here returns.
type TestResult = Result<(), Failure>;

/// Seeds per family: each seed starts the built binary twice.
const SEEDS: u64 = 8;

/// Expand `file` from `dir` and return its stdout.
fn expand(dir: &Path, file: &Path) -> Result<Vec<u8>, Failure> {
    let file = match file.to_str() {
        Some(file) => file,
        None => return refuse("an expand needs a file with a string path"),
    };
    let outcome = run_lgwks(dir, &["expand", file])?;
    if outcome.code != Some(0) {
        return refuse(format!(
            "lgwks expand refused the reference app: {}",
            outcome.stderr
        ));
    }
    Ok(outcome.stdout)
}

/// The pinned estate release the workspace names for `name`.
fn workspace_version(root: &Path, name: &str) -> Result<String, Failure> {
    let text = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|error| format!("the workspace manifest could not be read: {error}"))?;
    let prefix = format!("{name} = {{ version = \"");
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(prefix.as_str()) {
            let mut version = String::new();
            for cell in rest.chars() {
                if cell == '"' {
                    break;
                }
                version.push(cell);
            }
            if version.is_empty() {
                return refuse(format!("the workspace names no version for {name}"));
            }
            return Ok(version);
        }
    }
    refuse(format!("the workspace names no {name} dependency"))
}

/// The template's pinned releases match the workspace's, so the scaffold
/// can only drift with its own test failing first.
fn template_pins_match(dir: &Path, name: &str) -> TestResult {
    let manifest = std::fs::read_to_string(dir.join(name).join("Cargo.toml"))
        .map_err(|error| format!("the scaffolded manifest could not be read: {error}"))?;
    let root = workspace_root()?;
    for estate in ["lgwks_bot", "lgwks_std"] {
        let pinned = workspace_version(&root, estate)?;
        let wanted = format!("= {pinned}");
        let plain = format!("={pinned}");
        if !manifest.contains(wanted.as_str()) && !manifest.contains(plain.as_str()) {
            return refuse(format!("the scaffold pins no {estate} {pinned}"));
        }
    }
    Ok(())
}

/// Expanding twice answers the same text, carrying the flow and the map.
fn expand_replays(trace: &mut Trace, rng: &mut Rng, seed: u64) -> TestResult {
    let _ = rng;
    let scratch = Scratch::fresh("expand").map_err(|error| format!("no scratch: {error}"))?;
    let name = format!("expand{seed}");
    let reference = reference_app(scratch.dir(), name.as_str())?;
    let file = scratch.dir().join(&name).join("app.rs");
    std::fs::write(&file, reference.as_str())
        .map_err(|error| format!("the reference app could not be written: {error}"))?;
    let first = expand(scratch.dir(), &file)?;
    let again = expand(scratch.dir(), &file)?;
    if first != again {
        return refuse(format!("seed {seed}: the same file expanded twice"));
    }
    let text = match String::from_utf8(first.clone()) {
        Ok(text) => text,
        Err(_) => return refuse("the expansion is not UTF-8 text"),
    };
    for marker in [
        "pub async fn drafts",
        "ARCHITECTURE",
        "TypedRoute",
        "forbid",
    ] {
        if !text.contains(marker) {
            return refuse(format!("the expansion holds no {marker}"));
        }
    }
    template_pins_match(scratch.dir(), name.as_str())?;
    trace.record_number("seed", seed);
    trace.record_number("bytes", first.len());
    Ok(())
}

/// The expanded reference app passes clippy with denied warnings in a
/// scratch crate, as hand-written code would.
#[test]
fn expanded_app_passes_clippy() -> Result<(), Box<dyn Error>> {
    let scratch = Scratch::fresh("clippy").map_err(|error| format!("no scratch: {error}"))?;
    let name = String::from("clippyapp");
    let reference = reference_app(scratch.dir(), name.as_str())?;
    let dir = scratch.dir().join(&name);
    std::fs::write(dir.join("app.rs"), reference.as_str())
        .map_err(|error| format!("the reference app could not be written: {error}"))?;
    let outcome = run_lgwks(&dir, &["expand", "app.rs", "--json"])?;
    if outcome.code != Some(0) {
        return Err(format!("lgwks expand refused the reference app: {}", outcome.stderr).into());
    }
    let value: lgwks_std::json::Value = lgwks_std::json::from_slice(outcome.stdout.as_slice())
        .map_err(|error| format!("lgwks expand --json is not JSON: {error}"))?;
    let expanded = match value
        .get("expanded")
        .and_then(lgwks_std::json::Value::as_str)
    {
        Some(expanded) => expanded,
        None => return Err("the expansion carries no expanded text".into()),
    };
    let shim = "\nfn main() -> Result<(), Box<dyn std::error::Error>> {\n    let scope = lgwks_bot::script::Scope::root(lgwks_bot::script::Tenant::new(\"clippy\")?);\n    let _ = lgwks_bot::block_on(drafts(&scope, 9000))?;\n    Ok(())\n}\n";
    let mut main = expanded.to_owned();
    main.push_str(shim);
    std::fs::write(dir.join("app.rs"), main.as_str())
        .map_err(|error| format!("the expansion could not be staged: {error}"))?;
    let manifest = dir.join("Cargo.toml");
    cargo_in(
        &dir,
        &[
            OsStr::new("generate-lockfile"),
            OsStr::new("--manifest-path"),
            manifest.as_os_str(),
        ],
    )?;
    let target = app_target();
    cargo_in(
        &dir,
        &[
            OsStr::new("clippy"),
            OsStr::new("--locked"),
            OsStr::new("-j"),
            OsStr::new("4"),
            OsStr::new("--target-dir"),
            target.as_os_str(),
            OsStr::new("--manifest-path"),
            manifest.as_os_str(),
            OsStr::new("--all-targets"),
            OsStr::new("--"),
            OsStr::new("-D"),
            OsStr::new("warnings"),
        ],
    )
}

/// Run `cargo` with `args` in `dir`; its stderr is the refusal.
fn cargo_in(dir: &Path, args: &[&OsStr]) -> Result<(), Box<dyn Error>> {
    let output = Command::new("cargo")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|error| format!("cargo could not start: {error}"))?;
    if output.status.code() == Some(0) {
        return Ok(());
    }
    Err(format!(
        "cargo {:?} refused the expansion: {}",
        args.first(),
        String::from_utf8_lossy(output.stderr.as_slice())
    )
    .into())
}

#[test]
fn expand_replays_its_text() -> Result<(), Box<dyn Error>> {
    replay_sweep("expand_replays", SEEDS, |rng, trace| {
        let seed = rng.next_u64();
        expand_replays(trace, rng, seed)
    })
}
