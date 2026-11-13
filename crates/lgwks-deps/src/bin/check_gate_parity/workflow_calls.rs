//! Rule 7: no workflow calls another repository's reusable workflow.
//!
//! Source: `logical_ci.yml` called `Logical-Works-Inc/logical_ci`'s private
//! `gate.yml`. GitHub refuses a public repository's call to a private reusable
//! workflow before any job starts, so all 30 runs since #865 failed in 0 s and
//! the verdict never ran (ADR 0029).

/// The rule-7 violations of one workflow: every `uses:` line that calls a
/// reusable workflow of another repository, by line number. A `./` path is this
/// repository's own and an action is not a workflow call.
pub(crate) fn foreign_workflow_problems(workflow: &str, text: &str) -> Vec<String> {
    text.lines()
        .zip(1_usize..)
        .filter(|&(line, _)| {
            line.trim_start()
                .strip_prefix("uses:")
                .is_some_and(|called| {
                    let called = called.trim_start();
                    !called.starts_with("./") && called.contains("/.github/workflows/")
                })
        })
        .map(|(_, number)| {
            format!(
                "{workflow}:{number} calls another repository's reusable workflow, which a \
                 public repository cannot reach (ADR 0029)"
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::path::{Path, PathBuf};

    use super::foreign_workflow_problems;
    use crate::{MANIFEST_REL, OTHER_WORKFLOWS_REL, WORKFLOW_REL, check_parity, execute};

    type TestResult = Result<(), Box<dyn Error>>;

    /// The repository this crate sits in.
    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// The value of `KEY: value` in `text`, quotes stripped.
    fn env_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
        text.lines()
            .find_map(|line| line.trim_start().strip_prefix(key)?.strip_prefix(':'))
            .map(|value| value.trim().trim_matches('\''))
    }

    /// Where the step that runs the engine starts in `workflow`.
    fn engine_step(workflow: &str) -> Option<usize> {
        workflow.find("name: Run ci and keep the sealed record")
    }

    /// The inline gate names one release everywhere (ADR 0029: a re-pin
    /// moves `LCI_RELEASE` and both pins in one commit). The header, the
    /// runner comment and the ADR name the tag `LCI_RELEASE` downloads, and
    /// both pins are 64 lowercase hex digits, so a half-done re-pin fails here
    /// before the job ever downloads.
    fn inline_gate_problems(workflow: &str, adr: &str) -> Vec<String> {
        let mut problems = Vec::new();
        let Some(tag) = env_value(workflow, "LCI_RELEASE") else {
            return vec!["logical_ci.yml sets no LCI_RELEASE".to_owned()];
        };
        let named = [
            (
                "the header",
                workflow,
                format!("pin the signed {tag} release"),
            ),
            (
                "the runner comment",
                workflow,
                format!("logical_ci {tag} publishes"),
            ),
            (
                "ADR 0029",
                adr,
                format!("the signed logical_ci {tag} release"),
            ),
            // The v0.4.3 re-pin moved the header and missed this one, which
            // told a reader the old release's platforms were the current ones.
            (
                "ADR 0029's runner line",
                adr,
                format!("runs on `macos-latest`: {tag} ships"),
            ),
        ];
        for (place, text, phrase) in named {
            if !text.contains(&phrase) {
                problems.push(format!("{place} does not name {tag}: no `{phrase}`"));
            }
        }
        for key in ["LCI_RELEASE_PUBKEY", "LCI_VERIFIER_SHA256"] {
            match env_value(workflow, key) {
                Some(hex)
                    if hex.len() == 64
                        && hex
                            .bytes()
                            .all(|digit| matches!(digit, b'0'..=b'9' | b'a'..=b'f')) => {}
                Some(hex) => {
                    problems.push(format!("{key} is not 64 lowercase hex digits: `{hex}`"));
                }
                None => problems.push(format!("logical_ci.yml sets no {key}")),
            }
        }
        // ci/property-and-loom probes nextest first; without it A02 seals
        // UNMEASURED in 142 ms (run 43232168262), and a held one is kept.
        for needle in [
            "\"cargo-nextest 0.9.149\"*) echo \"nextest=held\"",
            "if: steps.tools.outputs.nextest != 'held'",
            "tool: nextest@0.9.149",
        ] {
            if !workflow.contains(needle) {
                problems.push(format!(
                    "logical_ci.yml does not install nextest: no `{needle}`"
                ));
            }
        }
        // The seal names every platform's binaries; lci-verify exited 2 on
        // v0.4.1 with the macOS pair alone, so every binary is downloaded.
        for needle in ["--pattern \"logical_ci-*\"", "--pattern \"lci-verify-*\""] {
            if !workflow.contains(needle) {
                problems.push(format!(
                    "logical_ci.yml downloads less than the seal names: no `{needle}`"
                ));
            }
        }
        // The engine's own cache directory is held by no rust-cache entry, so
        // A02 compiled cold and ran out at 397.1 s (run 43273499037).
        if !workflow.contains("LOGICAL_CI_TARGET_DIR: ${{ github.workspace }}/target") {
            problems.push(
                "logical_ci.yml lets the engine build outside the cached workspace target"
                    .to_owned(),
            );
        }
        // clippy-driver, the config's workspace wrapper, linted every crate
        // the property build compiled; A02 was killed at 394 s (run
        // 43300556873). Lints run once, in ci.yml's Clippy jobs.
        if env_value(workflow, "RUSTC_WORKSPACE_WRAPPER") != Some("\"\"") {
            problems.push("logical_ci.yml lints every crate the engine compiles".to_owned());
        }
        // The test profile builds dependencies at opt-level 3, and A02 was
        // killed cold at 394 s (run 43300556873); ci-it's levels took the
        // cold gate from 264.8 s to 167.5 s.
        if env_value(workflow, "CARGO_PROFILE_TEST_OPT_LEVEL") != Some("\"0\"")
            || !workflow.contains("[profile.test.package.\"*\"]\\nopt-level = 1")
        {
            problems.push(
                "logical_ci.yml builds the property lane at the test profile's opt-levels"
                    .to_owned(),
            );
        }
        // The hosted Mac's $CARGO_HOME may not exist, and the append alone
        // failed the job in 30 s (run 43310559327).
        if !workflow.contains("mkdir -p \"$CARGO_HOME\"\n") {
            problems.push(
                "logical_ci.yml writes the opt-levels into a $CARGO_HOME it never creates"
                    .to_owned(),
            );
        }
        // A `nextest --no-run` naming no target compiles every test binary
        // whatever the filter: it built all 51 and outlasted the job's 10
        // minutes (run 43282569391). One naming the lane's targets is what the
        // cold lane needs: v0.4.3's compile alone ran past its 392.8 s cap on
        // a 3-slot hosted Mac (run 43315235672).
        let prebuilds: Vec<(usize, &str)> = workflow
            .match_indices("run:")
            .filter_map(|(at, _)| {
                let command = workflow.get(at..)?.strip_prefix("run:")?.lines().next()?;
                command.contains("--no-run").then_some((at, command))
            })
            .collect();
        if prebuilds
            .iter()
            .any(|&(_, command)| !(command.contains("--lib") && command.contains("--test it")))
        {
            problems.push(
                "logical_ci.yml prebuilds every test binary the engine's property lane skips"
                    .to_owned(),
            );
        }
        let engine = engine_step(workflow);
        if !prebuilds
            .iter()
            .any(|&(at, _)| engine.is_some_and(|engine| at < engine))
        {
            problems.push(
                "logical_ci.yml leaves the property lane's cold compile inside its lane cap"
                    .to_owned(),
            );
        }
        // The show step runs if: always(); with no record jq exited 2 and
        // named a second failure that was not one (run 43296258554).
        if !workflow.contains("test -f \"$RUNNER_TEMP/logical_ci-record.json\" || exit 0") {
            problems.push(
                "logical_ci.yml shows lane output without checking a record exists".to_owned(),
            );
        }
        // ADR 0029 states why the whole prebuild went and why the selected
        // one came back, each with its run.
        if !adr.contains("(run 43282569391)") || !adr.contains("(run 43315235672)") {
            problems.push(
                "ADR 0029 does not record why the job prebuilds only the lane's targets".to_owned(),
            );
        }
        // The engine compiles this repository, whose `.cargo/config.toml`
        // links macOS with ld64.lld; a bare runner failed every compile in
        // 30 s (run 43242590489), so the repository's own setup runs first.
        let setup = workflow.find("uses: ./.github/actions/hosted-rust");
        if !matches!((setup, engine_step(workflow)), (Some(setup), Some(engine)) if setup < engine)
        {
            problems.push(
                "logical_ci.yml runs the engine before this repository's hosted-rust setup"
                    .to_owned(),
            );
        }
        problems
    }

    /// The call that failed every run is named by its line, the committed
    /// workflows make none, and this repository's own workflow and an action are
    /// not such a call. The inline gate the call became names one release in its
    /// header, its runner comment and ADR 0029, and a half-done re-pin is named.
    #[test]
    fn a_call_to_another_repositorys_reusable_workflow_is_refused() -> TestResult {
        let gate = std::fs::read_to_string(root().join(".github/workflows/logical_ci.yml"))?;
        let adr = std::fs::read_to_string(
            root().join("docs/adr/0029-the-logical-ci-verdict-runs-in-this-repository.md"),
        )?;
        assert_eq!(
            inline_gate_problems(&gate, &adr),
            Vec::<String>::new(),
            "the committed inline gate names one release"
        );
        let half = gate.replacen("LCI_RELEASE: v", "LCI_RELEASE: v9", 1);
        assert_ne!(
            half, gate,
            "the fixture text the control mutates must exist"
        );
        assert_eq!(
            inline_gate_problems(&half, &adr).len(),
            4,
            "a tag moved without its header, comment and both ADR lines is named four times"
        );
        let stale = adr.replacen("runs on `macos-latest`: v", "runs on `macos-latest`: v9", 1);
        assert_ne!(stale, adr, "the ADR text the control mutates must exist");
        assert_eq!(
            inline_gate_problems(&gate, &stale).len(),
            1,
            "an ADR runner line left on another release is named"
        );
        let swapped = gate.replacen("LCI_VERIFIER_SHA256: '", "LCI_VERIFIER_SHA256: 'X", 1);
        let swapped_problems = inline_gate_problems(&swapped, &adr);
        assert_eq!(swapped_problems.len(), 1, "only the verifier pin is wrong");
        assert!(
            swapped_problems
                .iter()
                .all(|problem| problem.starts_with("LCI_VERIFIER_SHA256 is not 64")),
            "a malformed verifier pin is named: {swapped_problems:?}"
        );
        let bare = gate.replacen("tool: nextest@0.9.149", "tool: cargo-hack", 1);
        assert_eq!(
            inline_gate_problems(&bare, &adr),
            vec!["logical_ci.yml does not install nextest: no `tool: nextest@0.9.149`".to_owned()],
            "a copy that drops the nextest install is named"
        );
        let unprepared = gate.replacen(
            "uses: ./.github/actions/hosted-rust",
            "uses: ./.github/actions/none",
            1,
        );
        assert_eq!(
            inline_gate_problems(&unprepared, &adr),
            vec![
                "logical_ci.yml runs the engine before this repository's hosted-rust setup"
                    .to_owned()
            ],
            "a copy that runs the engine on a bare runner is named"
        );
        let partial = gate.replacen(
            "--pattern \"lci-verify-*\"",
            "--pattern \"lci-verify-macos-arm64\"",
            1,
        );
        assert_eq!(
            inline_gate_problems(&partial, &adr),
            vec![
                "logical_ci.yml downloads less than the seal names: no `--pattern \"lci-verify-*\"`"
                    .to_owned()
            ],
            "a copy that downloads one platform's verifier alone is named"
        );
        let uncached = gate.replacen(
            "LOGICAL_CI_TARGET_DIR: ${{ github.workspace }}/target",
            "LOGICAL_CI_TARGET_DIR: /tmp/elsewhere",
            1,
        );
        assert_eq!(
            inline_gate_problems(&uncached, &adr),
            vec![
                "logical_ci.yml lets the engine build outside the cached workspace target"
                    .to_owned()
            ],
            "a copy whose engine builds outside the cached target is named"
        );
        let prebuilt = gate.replacen(
            "      - name: Run ci and keep the sealed record",
            "      - name: Build the property lane's test targets\n        \
             run: cargo nextest run --locked -E 'test(/property/)' --no-tests fail --no-run\n      \
             - name: Run ci and keep the sealed record",
            1,
        );
        assert_ne!(prebuilt, gate, "the step the control precedes must exist");
        assert_eq!(
            inline_gate_problems(&prebuilt, &adr),
            vec![
                "logical_ci.yml prebuilds every test binary the engine's property lane skips"
                    .to_owned()
            ],
            "a copy that brings back the 51-binary prebuild is named"
        );
        let in_cap = gate.replacen("--test it --no-run", "--test it", 1);
        assert_ne!(in_cap, gate, "the prebuild the control drops must exist");
        assert_eq!(
            inline_gate_problems(&in_cap, &adr),
            vec![
                "logical_ci.yml leaves the property lane's cold compile inside its lane cap"
                    .to_owned()
            ],
            "a copy with no selected prebuild before the engine is named"
        );
        let unguarded = gate.replacen(
            "test -f \"$RUNNER_TEMP/logical_ci-record.json\" || exit 0\n",
            "",
            1,
        );
        assert_ne!(unguarded, gate, "the guard the control removes must exist");
        assert_eq!(
            inline_gate_problems(&unguarded, &adr),
            vec!["logical_ci.yml shows lane output without checking a record exists".to_owned()],
            "a show step that fails on a missing record is named"
        );
        let linting = gate.replacen("      RUSTC_WORKSPACE_WRAPPER: \"\"\n", "", 1);
        assert_ne!(linting, gate, "the line the control removes must exist");
        assert_eq!(
            inline_gate_problems(&linting, &adr),
            vec!["logical_ci.yml lints every crate the engine compiles".to_owned()],
            "a copy that lets clippy-driver wrap the property build is named"
        );
        let optimised = gate.replacen("      CARGO_PROFILE_TEST_OPT_LEVEL: \"0\"\n", "", 1);
        assert_ne!(optimised, gate, "the line the control removes must exist");
        assert_eq!(
            inline_gate_problems(&optimised, &adr),
            vec![
                "logical_ci.yml builds the property lane at the test profile's opt-levels"
                    .to_owned()
            ],
            "a copy that builds the lane at opt-level 3 is named"
        );
        let homeless = gate.replacen("          mkdir -p \"$CARGO_HOME\"\n", "", 1);
        assert_ne!(homeless, gate, "the line the control removes must exist");
        assert_eq!(
            inline_gate_problems(&homeless, &adr),
            vec![
                "logical_ci.yml writes the opt-levels into a $CARGO_HOME it never creates"
                    .to_owned()
            ],
            "a copy that appends to a missing $CARGO_HOME is named"
        );
        let unrecorded = adr.replacen("(run 43282569391)", "", 1);
        assert_ne!(
            unrecorded, adr,
            "the ADR text the control mutates must exist"
        );
        assert_eq!(
            inline_gate_problems(&gate, &unrecorded),
            vec![
                "ADR 0029 does not record why the job prebuilds only the lane's targets".to_owned()
            ],
            "an ADR that drops the run the prebuild outlasted is named"
        );
        let clobbering = gate.replacen("if: steps.tools.outputs.nextest != 'held'", "", 1);
        assert_eq!(
            inline_gate_problems(&clobbering, &adr).len(),
            1,
            "a copy that writes over a held nextest is named"
        );
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            execute(&root(), &mut out, &mut err)?,
            0,
            "every committed workflow passes: {}",
            String::from_utf8_lossy(&err)
        );
        for rel in OTHER_WORKFLOWS_REL {
            let text = std::fs::read_to_string(root().join(rel))?;
            assert_eq!(
                foreign_workflow_problems(rel, &text),
                Vec::<String>::new(),
                "{rel} calls no other repository's workflow"
            );
        }
        let failed = "jobs:\n  gate:\n    uses: Logical-Works-Inc/logical_ci/.github/workflows/gate.yml@v0.1.0\n";
        assert_eq!(
            foreign_workflow_problems("logical_ci.yml", failed),
            vec![
                "logical_ci.yml:3 calls another repository's reusable workflow, which a public \
                 repository cannot reach (ADR 0029)"
                    .to_owned()
            ],
            "the call that failed every run is named by its line"
        );
        let own = "jobs:\n  gate:\n    uses: ./.github/workflows/consumer-gate.yml\n    steps:\n      \
                   - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4\n";
        assert_eq!(
            foreign_workflow_problems("ci.yml", own),
            Vec::<String>::new(),
            "this repository's own workflow and an action are not another repository's"
        );
        Ok(())
    }

    /// `ci.yml` is held to rule 8 through `check_parity`, and the repository's
    /// root fails when one of the other workflows makes the call.
    #[test]
    fn every_workflow_the_checker_reads_is_held_to_rule_8() -> TestResult {
        let manifest = std::fs::read_to_string(root().join(MANIFEST_REL))?;
        let workflow = std::fs::read_to_string(root().join(WORKFLOW_REL))?;
        let routed = workflow.replacen(
            "jobs:\n",
            "jobs:\n  verdict:\n    uses: Logical-Works-Inc/logical_ci/.github/workflows/gate.yml@v0.2.0\n",
            1,
        );
        assert_ne!(
            routed, workflow,
            "the fixture text the control mutates must exist"
        );
        assert!(
            check_parity(&manifest, &routed)
                .iter()
                .any(|problem| problem.starts_with("ci.yml:") && problem.contains("ADR 0029")),
            "ci.yml is held to rule 8"
        );
        Ok(())
    }

    /// The paths rust-cache saves, in its order: a restore naming any other
    /// set computes another cache version and finds none of its entries.
    const RUST_CACHE_PATHS: &str = "\"${home}/bin\" \"${home}/.crates.toml\" \
        \"${home}/.crates2.json\" \"${home}/registry\" \"${home}/git\" \
        \"${GITHUB_WORKSPACE}/target\"";

    /// What keeps a pull request whose lockfile moved off a cold build (ADR
    /// 0036): after rust-cache's step, a read-only restore of main's newest
    /// entry for the group, on a pull request's miss only, over rust-cache's
    /// own paths. Run 43311128730 walled at 312 s with it absent.
    fn base_build_problems(action: &str) -> Vec<String> {
        let mut problems = Vec::new();
        if action.contains("uses: actions/cache@") || action.contains("uses: actions/cache/save@") {
            problems.push("hosted-rust saves a second cache beside rust-cache".to_owned());
        }
        let (Some(cache), Some(base)) = (
            action.find("id: rust-cache\n"),
            action.find("uses: actions/cache/restore@"),
        ) else {
            problems.push("hosted-rust restores no base build after rust-cache".to_owned());
            return problems;
        };
        if base < cache {
            problems.push("hosted-rust restores its base build before rust-cache".to_owned());
        }
        if !action.contains(
            "github.ref != 'refs/heads/main' && steps.rust-cache.outputs.cache-hit != 'true'",
        ) {
            problems.push(
                "hosted-rust restores a base build where rust-cache already hit or on main"
                    .to_owned(),
            );
        }
        if !action.contains(RUST_CACHE_PATHS) {
            problems.push("hosted-rust's base build names other paths than rust-cache".to_owned());
        }
        if !action.contains("restore-keys: v0-rust-${{ inputs.cache-key }}-\n") {
            problems.push("hosted-rust's base build is not main's entry for its group".to_owned());
        }
        problems
    }

    /// The committed action holds the base build, and each way of losing it
    /// is named on its own.
    #[test]
    fn a_moved_lockfile_restores_mains_nearest_build() -> TestResult {
        let action =
            std::fs::read_to_string(root().join(".github/actions/hosted-rust/action.yml"))?;
        assert_eq!(base_build_problems(&action), Vec::<String>::new());
        let controls = [
            (
                "      id: base-build\n      uses: actions/cache/restore@v6\n",
                "      id: base-build\n",
                "hosted-rust restores no base build after rust-cache",
            ),
            (
                " && steps.rust-cache.outputs.cache-hit != 'true'",
                "",
                "hosted-rust restores a base build where rust-cache already hit or on main",
            ),
            (
                "\"${home}/registry\" \"${home}/git\"",
                "\"${home}/git\" \"${home}/registry\"",
                "hosted-rust's base build names other paths than rust-cache",
            ),
            (
                "restore-keys: v0-rust-${{ inputs.cache-key }}-\n",
                "restore-keys: v0-rust-\n",
                "hosted-rust's base build is not main's entry for its group",
            ),
            (
                "uses: actions/cache/restore@v6",
                "uses: actions/cache@v6",
                "hosted-rust saves a second cache beside rust-cache",
            ),
        ];
        for (from, to, named) in controls {
            let broken = action.replacen(from, to, 1);
            assert_ne!(
                broken, action,
                "the text the control mutates must exist: {from}"
            );
            assert!(
                base_build_problems(&broken)
                    .iter()
                    .any(|problem| problem == named),
                "{named}: {:?}",
                base_build_problems(&broken)
            );
        }
        Ok(())
    }

    /// The `it` group rows share one dependency build (ADR 0037). A group cfg
    /// in RUSTFLAGS reaches every dependency, and the 11 Linux rows saved 11
    /// copies of one build: 18.3 GB on main past the 10 GB cap, whose LRU
    /// eviction dropped logical_ci's entry (run 43340940282).
    fn group_build_problems(ci: &str) -> Vec<String> {
        let mut problems = Vec::new();
        if ci.lines().any(|line| {
            line.trim_start()
                .strip_prefix("RUSTFLAGS:")
                .is_some_and(|flags| flags.contains("lwc_it_group"))
        }) {
            problems.push("ci.yml sets the it group cfg in RUSTFLAGS".to_owned());
        }
        if !ci.contains("cache-key: ${{ matrix.it_group && 'bot-it' || matrix.archive }}") {
            problems.push("ci.yml keys each Linux it group to its own entry".to_owned());
        }
        let wrapped_runs = ci
            .matches(
                "        env:\n          RUSTC_WORKSPACE_WRAPPER: ${{ env.RUST_IT_GROUP_RUSTC }}\n        run: cargo nextest run ${NEXTEST_BUILD:--p lgwks_bot --all-targets",
            )
            .count();
        if !ci.contains("RUST_IT_GROUP_RUSTC: ${{ matrix.it_group && '/tmp/it-group-rustc' || '' }}")
            || !ci.contains(
                "then rustc --edition 2024 -O -o \"$RUST_IT_GROUP_RUSTC\" scripts/it_group_rustc.rs; fi",
            )
            || wrapped_runs != 1
        {
            problems.push("ci.yml's Linux it group rows build without the group wrapper".to_owned());
        }
        problems
    }

    /// The committed ci.yml shares the group build, and each way of losing
    /// it is named.
    #[test]
    fn the_it_group_rows_share_one_dependency_build() -> TestResult {
        let ci = std::fs::read_to_string(root().join(WORKFLOW_REL))?;
        assert_eq!(group_build_problems(&ci), Vec::<String>::new());
        let controls = [
            (
                "      LWC_IT_GROUP: ${{ matrix.it_group }}\n",
                "      RUSTFLAGS: ${{ matrix.it_group && format('--cfg lwc_it_group --cfg lwc_it_group=\"{0}\"', matrix.it_group) || '' }}\n",
                "ci.yml sets the it group cfg in RUSTFLAGS",
            ),
            (
                "cache-key: ${{ matrix.it_group && 'bot-it' || matrix.archive }}",
                "cache-key: ${{ matrix.archive }}",
                "ci.yml keys each Linux it group to its own entry",
            ),
            (
                "then rustc --edition 2024 -O -o \"$RUST_IT_GROUP_RUSTC\" scripts/it_group_rustc.rs; fi",
                "then true; fi",
                "ci.yml's Linux it group rows build without the group wrapper",
            ),
            (
                "        env:\n          RUSTC_WORKSPACE_WRAPPER: ${{ env.RUST_IT_GROUP_RUSTC }}\n        run: cargo nextest run ${NEXTEST_BUILD:--p lgwks_bot --all-targets",
                "        run: cargo nextest run ${NEXTEST_BUILD:--p lgwks_bot --all-targets",
                "ci.yml's Linux it group rows build without the group wrapper",
            ),
        ];
        for (from, to, named) in controls {
            let broken = ci.replacen(from, to, 1);
            assert_ne!(
                broken, ci,
                "the text the control mutates must exist: {from}"
            );
            assert_eq!(
                group_build_problems(&broken),
                vec![named.to_owned()],
                "{named}"
            );
        }
        Ok(())
    }
}
