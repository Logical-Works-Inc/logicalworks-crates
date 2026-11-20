//! Every cfg arm holds the lint floor, including the arms this host never compiles.
//!
//! PR #1103's hosted run (2026-11-20) failed nine jobs on code no Mac build sees:
//! `let _ = config;` in the arm without `debug-doctor` tripped `let_underscore_drop`,
//! and nineteen `let _ = <param>;` lines in the arms for targets that are neither
//! Unix nor Windows sit under clippy's denied `let_underscore_untyped` (clippy's
//! lint docs: "binding to `_` without a type ... can hide a value's type"). A lint
//! only judges the cfg set it is built with (the Reference, "Conditional
//! compilation"), so this test reads the source instead and holds every arm to
//! the idiom that is clean on every target: an unused parameter is named `_x` in
//! that arm's signature, and an unused local with a destructor is `drop`ped.
//!
//! The next run failed the WASI job the same way: the `not(unix)` arm of
//! `lgwks_bot::durable::sync_directory` was `Ok(())` and nothing else, which
//! clippy's denied `unnecessary_wraps` refuses on WASI alone. An arm whose whole
//! body is a bare `Ok(())` is that shape on whichever target builds it.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

type TestResult = Result<(), Box<dyn Error>>;

/// The workspace root: this crate sits at `crates/lgwks-deps`.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Whether `path` is test code, which the lint floor does not bind.
fn is_test_file(path: &Path) -> bool {
    let named =
        |name: &str| name == "tests.rs" || name.ends_with("_tests.rs") || name.starts_with("sim_");
    path.file_name()
        .and_then(|file| file.to_str())
        .is_some_and(named)
        || path.components().any(|part| part.as_os_str() == "tests")
}

/// Every non-test `.rs` file under `dir`, depth-first.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_sources(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") && !is_test_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Whether `line` is `let _ = <bindings>;`: a silencer of names, not a call whose result is dropped.
fn is_silencer(line: &str) -> bool {
    let Some(rest) = line.trim().strip_prefix("let _ = ") else {
        return false;
    };
    let Some(body) = rest.strip_suffix(';') else {
        return false;
    };
    !body.is_empty()
        && body
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '&' | '(' | ')' | ',' | ' '))
}

/// Whether the function opening at `rows[at]` is a cfg arm whose whole body is
/// `Ok(())`: a `#[cfg(..not(..))]` on the item, a signature returning a
/// `Result`, then `Ok(())` and the closing brace.
fn is_bare_ok_arm(rows: &[&str], at: usize) -> bool {
    let row = |offset: usize| {
        at.checked_add(offset)
            .and_then(|index| rows.get(index))
            .map(|line| line.trim())
    };
    let signature = row(0).is_some_and(|line| {
        line.contains("fn ") && line.contains("Result<") && line.ends_with('{')
    });
    if !signature || row(1) != Some("Ok(())") || row(2) != Some("}") {
        return false;
    }
    // The attributes sit directly above the signature; a doc line or a blank one ends them.
    rows.iter()
        .take(at)
        .rev()
        .map(|line| line.trim())
        .take_while(|line| line.starts_with("#["))
        .any(|line| line.starts_with("#[cfg(") && line.contains("not("))
}

/// Whether `pair` is a `#[cfg(test)]` line followed by a `mod`: the test module that closes a file's production part.
fn opens_test_module(pair: &[&str]) -> bool {
    matches!(pair, [cfg, next] if cfg.trim() == "#[cfg(test)]" && next.trim_start().starts_with("mod "))
}

/// One source file as shown in a finding (its workspace-relative path) and its production rows.
type ProductionFile = (String, Vec<String>);

/// The production rows of every crate's source: each file's lines up to its `#[cfg(test)]` module.
fn production_files(root: &Path) -> Result<Vec<ProductionFile>, Box<dyn Error>> {
    let mut files = Vec::new();
    for krate in fs::read_dir(root.join("crates"))? {
        let src = krate?.path().join("src");
        if src.is_dir() {
            rust_sources(&src, &mut files)?;
        }
    }
    files.sort();
    let mut read = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file)?;
        let shown = match file.strip_prefix(root) {
            Ok(relative) => relative.display().to_string(),
            Err(_outside) => file.display().to_string(),
        };
        let rows: Vec<&str> = text.lines().collect();
        let end = match rows.windows(2).position(opens_test_module) {
            Some(end) => end,
            None => rows.len(),
        };
        read.push((
            shown,
            rows.iter()
                .take(end)
                .map(|line| (*line).to_owned())
                .collect(),
        ));
    }
    Ok(read)
}

/// Each row a `judge` flags, as `path:line: text`.
fn flagged(root: &Path, judge: fn(&[&str], usize) -> bool) -> Result<Vec<String>, Box<dyn Error>> {
    let mut found = Vec::new();
    for (shown, owned) in production_files(root)? {
        let rows: Vec<&str> = owned.iter().map(String::as_str).collect();
        for (index, line) in rows.iter().enumerate() {
            if judge(&rows, index) {
                found.push(format!(
                    "{shown}:{}: {}",
                    index.saturating_add(1),
                    line.trim()
                ));
            }
        }
    }
    Ok(found)
}

/// [`is_silencer`] in the row form [`flagged`] takes.
fn silencer_row(rows: &[&str], at: usize) -> bool {
    rows.get(at).is_some_and(|line| is_silencer(line))
}

#[test]
fn no_cfg_arm_silences_a_binding_with_let_underscore() -> TestResult {
    let found = flagged(&workspace_root(), silencer_row)?;
    assert!(
        found.is_empty(),
        "{} cfg-arm silencer(s); name the parameter `_x` in that arm, or `drop` a local:\n{}",
        found.len(),
        found.join("\n")
    );
    // The reader itself: a name is a silencer, a dropped call result is the error-swallowing ban judged elsewhere.
    assert!(is_silencer("    let _ = pid;"));
    assert!(is_silencer("        let _ = (spec, on_line);"));
    assert!(is_silencer("let _ = &root;"));
    assert!(!is_silencer("    let _ = writer.flush();"));
    assert!(
        !is_silencer("    let _guard = lock;"),
        "a named binding lives to scope end"
    );
    assert!(!is_silencer("    let _ = ;"));
    assert!(is_test_file(Path::new("crates/x/src/a/tests.rs")));
    assert!(is_test_file(Path::new("crates/x/src/sim_walk.rs")));
    assert!(!is_test_file(Path::new("crates/x/src/process.rs")));
    Ok(())
}

#[test]
fn no_cfg_arm_wraps_a_bare_ok_in_a_result() -> TestResult {
    let found = flagged(&workspace_root(), is_bare_ok_arm)?;
    assert!(
        found.is_empty(),
        "{} cfg arm(s) return only `Ok(())`, which `unnecessary_wraps` refuses on the target that builds them; \
         give the arm the work its other arm does (a stat, a check) or a typed refusal:\n{}",
        found.len(),
        found.join("\n")
    );
    // The reader itself, on the WASI arm PR #1103 shipped and on the shapes beside it.
    let wasi_arm = [
        "#[cfg(not(unix))]",
        "pub(crate) fn sync_directory(_dir: &Path) -> io::Result<()> {",
        "    Ok(())",
        "}",
    ];
    assert!(
        is_bare_ok_arm(&wasi_arm, 1),
        "the shipped WASI arm is the shape"
    );
    let stat_arm = [
        "#[cfg(not(unix))]",
        "pub(crate) fn sync_directory(dir: &Path) -> io::Result<()> {",
        "    std::fs::metadata(dir).map(|_metadata| ())",
        "}",
    ];
    assert!(!is_bare_ok_arm(&stat_arm, 1), "an arm that stats does work");
    let unix_arm = [
        "#[cfg(unix)]",
        "pub(crate) fn sync_directory(_dir: &Path) -> io::Result<()> {",
        "    Ok(())",
        "}",
    ];
    assert!(
        !is_bare_ok_arm(&unix_arm, 1),
        "an arm this host builds is clippy's to judge"
    );
    let trait_method = [
        "    fn flush(&mut self) -> io::Result<()> {",
        "        Ok(())",
        "    }",
    ];
    assert!(
        !is_bare_ok_arm(&trait_method, 0),
        "a trait's signature is not the arm's to change"
    );
    Ok(())
}
