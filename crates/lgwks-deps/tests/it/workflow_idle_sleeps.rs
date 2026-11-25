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

/// logical_ci's A02 lane probes loom under `--release` inside a 97 279 ms timer; cold, the probe's
/// own compile outlasted the timer and sealed A02 UNMEASURED (run 44733566537). The job builds the
/// release target before the engine runs. Since logical_ci v0.4.10 (logical_ci#549) the engine
/// prints that build with its wrapper and its `loom-target`, and the job runs every printed cargo
/// build with its own environment, so the probe's target is the one built.
#[test]
fn the_loom_probe_release_target_is_built_before_the_engine() -> TestResult {
    let text = fs::read_to_string(workflows().join("logical_ci.yml"))?;
    let engine = text
        .find("- name: Run ci and keep the sealed record")
        .ok_or("logical_ci.yml has no engine step")?;
    for part in [
        "plan --builds --tenant \"$LCI_TENANT\" --json",
        "select(.program == \"cargo\")",
        "[.env[] | \"\\(.name)=\\(.value)\"]",
        "sh -e \"$RUNNER_TEMP/lci-builds.sh\"",
    ] {
        let built = text
            .find(part)
            .ok_or_else(|| format!("logical_ci.yml never runs the engine's builds: no `{part}`"))?;
        assert!(
            built < engine,
            "`{part}` comes after the engine already timed the loom probe"
        );
    }
    Ok(())
}

/// The first run on logical_ci v0.4.10 (run 45442648134) spent 12.3 min cold on the builds the
/// engine printed (4 m 03 s, 1 m 45 s, 6 m 28 s), and its 15-minute cap cancelled A11 91 s into
/// the engine's 420 s budget. A cancelled job saves no cache, so the next run is cold again. The
/// cap holds setup, those builds, the budget and the upload.
#[test]
fn the_logical_ci_job_outlasts_its_cold_builds_and_lane_budget() -> TestResult {
    const SETUP_S: u64 = 60;
    const COLD_BUILDS_S: u64 = 243 + 105 + 388;
    const LANE_BUDGET_S: u64 = 420;
    const UPLOAD_S: u64 = 60;
    let text = fs::read_to_string(workflows().join("logical_ci.yml"))?;
    let minutes: u64 = text
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("timeout-minutes:"))
        .ok_or("logical_ci.yml sets no timeout-minutes")?
        .trim()
        .parse()?;
    let need = SETUP_S + COLD_BUILDS_S + LANE_BUDGET_S + UPLOAD_S;
    assert!(
        minutes * 60 >= need,
        "the job's {minutes}-minute cap ends before its cold run's {need} s"
    );
    Ok(())
}

/// Director 2026-11-24: "LOGICAL CI WILL NEVER FUCKING CAUSE A REPO TO STOP MERGE IF IT FAILS".
/// Every step of the logical_ci job continues on error and the last one reports the verdict with
/// exit 0, so no verdict and no setup failure turns the check red. It still fails closed: each
/// step that runs the downloaded engine waits for the seal to verify. The controls: a step that
/// can fail the job, a report that exits 1, and an engine step that runs unverified.
#[test]
fn the_logical_ci_verdict_never_fails_the_job() -> TestResult {
    let text = fs::read_to_string(workflows().join("logical_ci.yml"))?;
    assert_eq!(never_blocks_problems(&text), Vec::<String>::new());
    let controls = [
        (
            "      - name: Prepare this repository's Rust\n        continue-on-error: true\n",
            "      - name: Prepare this repository's Rust\n",
            "step `Prepare this repository's Rust` can fail the job",
        ),
        (
            "(reported, never a merge gate)\" ;; esac",
            "(reported, never a merge gate)\"; exit 1 ;; esac",
            "the verdict report exits 1",
        ),
        (
            "      - name: Init tenant policy (idempotent by policy version)\n        if: steps.verify.outcome == 'success'\n",
            "      - name: Init tenant policy (idempotent by policy version)\n",
            "step `Init tenant policy (idempotent by policy version)` runs the engine before its seal verifies",
        ),
    ];
    for (from, to, named) in controls {
        let broken = text.replacen(from, to, 1);
        assert_ne!(
            broken, text,
            "the text the control mutates must exist: {from}"
        );
        assert_eq!(
            never_blocks_problems(&broken),
            vec![named.to_owned()],
            "{named}"
        );
    }
    Ok(())
}

/// Each way the logical_ci job could turn its check red or run an unverified engine.
fn never_blocks_problems(text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for step in text.split("\n      - ").skip(1) {
        // A named step's first line is its `name:`; the checkout's is its `uses:`.
        let Some(first) = step.lines().next() else {
            continue;
        };
        let name = first.trim_start_matches("name: ");
        if name == "Report the verdict" {
            if step.contains("exit 1") {
                problems.push("the verdict report exits 1".to_owned());
            }
            continue;
        }
        if !step.contains("\n        continue-on-error: true\n") {
            problems.push(format!("step `{name}` can fail the job"));
        }
        let runs_engine = step.contains("logical_ci-macos-arm64\"")
            && !step.contains("test -f \"$RUNNER_TEMP/logical_ci-record.json\" || exit 0");
        if runs_engine
            && !name.starts_with("Download")
            && !step.contains("if: steps.verify.outcome == 'success'")
        {
            problems.push(format!(
                "step `{name}` runs the engine before its seal verifies"
            ));
        }
    }
    if !text.contains("      - name: Report the verdict\n        if: always()\n") {
        problems.push("the job has no verdict report that always runs".to_owned());
    }
    problems
}

/// Run 45490255730: the v0.4.11 pin was pushed before release.yml published, and the download
/// at 21:21:52 printed only `release not found`; v0.4.11 published at 21:22:12. The download
/// names the cause.
#[test]
fn the_logical_ci_download_names_an_unpublished_release() -> TestResult {
    let text = fs::read_to_string(workflows().join("logical_ci.yml"))?;
    let step = text
        .split("- name: ")
        .find(|step| step.starts_with("Download pinned release"))
        .ok_or("logical_ci.yml has no download step")?;
    assert!(
        step.contains("|| { echo \"$LCI_RELEASE is not published on Logical-Works-Inc/logical_ci: a pin lands only after its release.yml run publishes\"; exit 1; }"),
        "the download does not name an unpublished release as the cause"
    );
    Ok(())
}

/// Run 45448206410: the wait step saw bot-rest's listing in the REST answer at 17:27:09.15, and
/// `download-artifact`, which reads its own listing, fetched 12 of 13 at 17:27:09.49, so the
/// simulation lane could not run. The job fetches each listing by id from the answer it waited on.
/// It fetches only this run's `SUITES`: with the bot rows routed off (ADR 0086), their listings
/// are main's from the cache, and this run uploaded none to fetch.
#[test]
fn the_simulation_lane_fetches_the_listings_it_waited_for() -> TestResult {
    let text = fs::read_to_string(workflows().join("ci.yml"))?;
    let step = text
        .split("- name: ")
        .find(|step| step.starts_with("Wait for and fetch the Linux suite listings"))
        .ok_or("ci.yml has no step that waits for and fetches the suite listings")?;
    for part in [
        "actions/runs/${GITHUB_RUN_ID}/artifacts?per_page=100",
        "for suite in ${SUITES}; do",
        "dir=\"nextest-listing/nextest-listing-${suite}-Linux\"",
        "id=\"${before##*\\\"id\\\":}\"",
        "/actions/artifacts/${id}/zip",
        "file=\"${dir}/listing.json\"",
        "test -s \"${file}\"",
    ] {
        assert!(step.contains(part), "the listing step lost `{part}`");
    }
    assert!(
        !text.contains("pattern: nextest-listing-*-Linux"),
        "a second listing of the artifacts can miss one the wait saw"
    );
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
