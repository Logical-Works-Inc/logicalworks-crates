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

/// Each silencer in the non-test source of every crate, as `path:line: text`.
fn silencers(root: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut files = Vec::new();
    for krate in fs::read_dir(root.join("crates"))? {
        let src = krate?.path().join("src");
        if src.is_dir() {
            rust_sources(&src, &mut files)?;
        }
    }
    files.sort();
    let mut found = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file)?;
        let shown = match file.strip_prefix(root) {
            Ok(relative) => relative.display().to_string(),
            Err(_outside) => file.display().to_string(),
        };
        let mut rows = text.lines().enumerate().peekable();
        while let Some((index, line)) = rows.next() {
            // A `#[cfg(test)]` module closes the production part of the file.
            let test_module = rows
                .peek()
                .is_some_and(|&(_, next)| next.trim_start().starts_with("mod "));
            if line.trim() == "#[cfg(test)]" && test_module {
                break;
            }
            if is_silencer(line) {
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

#[test]
fn no_cfg_arm_silences_a_binding_with_let_underscore() -> TestResult {
    let found = silencers(&workspace_root())?;
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
