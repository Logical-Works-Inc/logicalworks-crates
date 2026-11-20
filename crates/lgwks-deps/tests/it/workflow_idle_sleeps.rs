//! No workflow step idles on the hosted run.
//!
//! PR #1103's run on 6fe18a1 (2026-11-20) passed every test and failed its own
//! wall rule, "run wall 301 s, limit 300 s", while `debug_e2e` and
//! `simulation_evidence` each ended in `sleep 12` after their pass-line grep. A
//! step's verdict is its exit status (GitHub's workflow syntax: a step that exits
//! non-zero fails its job), so a sleep after the check decides nothing and every
//! second of it lands on the critical path through `needs:`. This test reads
//! every workflow and refuses a `sleep` command outside a loop. A sleep inside
//! a bounded `for`/`while`/`until` loop is a poll on an event (the suite
//! listings a sibling job uploads, which no GitHub event signals in-job); a
//! sleep outside one waits on nothing.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

type TestResult = Result<(), Box<dyn Error>>;

/// The repository's workflow directory: this crate sits at `crates/lgwks-deps`.
fn workflows() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(".github")
        .join("workflows")
}

/// Whether `line` runs `sleep` as a command: alone, or after `&&`, `;` or `|`.
fn idles(line: &str) -> bool {
    let code = match line.split_once('#') {
        Some((before, _comment)) => before,
        None => line,
    };
    code.split(['&', ';', '|'])
        .map(str::trim_start)
        .any(|command| command == "sleep" || command.starts_with("sleep "))
}

/// Whether `line` opens a shell loop body's header (`for`, `while`, `until`).
fn opens_loop(line: &str) -> bool {
    let trimmed = line.trim_start();
    ["for ", "while ", "until "]
        .iter()
        .any(|word| trimmed.starts_with(word))
}

/// Whether `line` closes a shell loop (`done`, alone or before a redirect or `;`).
fn closes_loop(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed == "done" || trimmed.starts_with("done ") || trimmed.starts_with("done;")
}

/// The lines of `text` that sleep outside every loop, each with its 1-based number.
fn idle_lines(text: &str) -> Vec<(usize, &str)> {
    let mut depth = 0_usize;
    let mut idle = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if opens_loop(line) {
            depth = depth.saturating_add(1);
        }
        if depth == 0 && idles(line) {
            idle.push((index.saturating_add(1), line.trim()));
        }
        if closes_loop(line) {
            depth = depth.saturating_sub(1);
        }
    }
    idle
}

/// Each `file:line: text` in every workflow that sleeps outside a loop.
fn idle_steps(dir: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .map(|entry| entry.map(|found| found.path()))
        .collect::<Result<_, _>>()?;
    files.retain(|path| {
        path.extension()
            .is_some_and(|ext| ext == "yml" || ext == "yaml")
    });
    files.sort();
    let mut found = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file)?;
        let name = file.strip_prefix(dir)?.display().to_string();
        for (number, line) in idle_lines(&text) {
            found.push(format!("{name}:{number}: {line}"));
        }
    }
    Ok(found)
}

/// logical_ci's A02 lane probes `cargo test --release -p lgwks_std --lib loom` inside a 97 279 ms
/// timer. loom runs under `--release`, so the job builds that exact target before the engine runs:
/// cold, the probe's own compile outlasted the timer and sealed A02 UNMEASURED (run 44733566537).
#[test]
fn the_loom_probe_release_target_is_built_before_the_engine() -> TestResult {
    let text = fs::read_to_string(workflows().join("logical_ci.yml"))?;
    let prebuild = "cargo test --locked --release -p lgwks_std --lib --no-run";
    let built = text
        .find(prebuild)
        .ok_or("logical_ci.yml builds no release target for the loom probe")?;
    let engine = text
        .find("- name: Run ci and keep the sealed record")
        .ok_or("logical_ci.yml has no engine step")?;
    assert!(
        built < engine,
        "the loom probe's release target is built after the engine already timed it"
    );
    // The prebuild is the probe's own target: same package, same lib, same profile, from the lockfile.
    for part in ["--release", "-p lgwks_std --lib", "--locked"] {
        assert!(prebuild.contains(part), "the prebuild lost `{part}`");
    }
    Ok(())
}

#[test]
fn no_workflow_step_sleeps() -> TestResult {
    let found = idle_steps(&workflows())?;
    assert!(
        found.is_empty(),
        "{} workflow line(s) sleep; a step's exit status is its verdict, so wait on the event or drop the line:\n{}",
        found.len(),
        found.join("\n")
    );
    // The reader itself, on the line the 301 s run shipped and the shapes beside it.
    assert!(idles("          sleep 12"), "the shipped trailing sleep");
    assert!(
        idles("  grep -q pass out.txt && sleep 5"),
        "a sleep chained after the check"
    );
    assert!(idles("  true; sleep 1"));
    assert!(
        !idles("  # sleep 12 would pad the job"),
        "a comment runs nothing"
    );
    assert!(
        !idles("  cargo nextest run -E 'test(sleep_sort)'"),
        "a word containing sleep is not the command"
    );
    assert!(!idles("      timeout-minutes: 5"));
    let trailing = "run: |\n  grep -q pass e2e.txt\n  sleep 12\n";
    assert_eq!(
        idle_lines(trailing),
        [(3, "sleep 12")],
        "a sleep after the check waits on nothing"
    );
    let poll = "for attempt in $(seq 1 60); do\n  check && exit 0\n  sleep 4\ndone\nsleep 1\n";
    assert_eq!(
        idle_lines(poll),
        [(5, "sleep 1")],
        "a bounded poll may sleep; the line after it may not"
    );
    Ok(())
}
