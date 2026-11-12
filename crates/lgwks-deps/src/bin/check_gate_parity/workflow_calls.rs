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
        // `nextest --no-run` compiles every test binary whatever the filter:
        // the prebuild built all 51 and outlasted the job's 10 minutes (run
        // 43282569391), where v0.4.2's lane builds the 5 holding a property.
        if workflow.lines().any(|line| {
            line.trim_start()
                .strip_prefix("run:")
                .is_some_and(|command| command.contains("--no-run"))
        }) {
            problems.push(
                "logical_ci.yml prebuilds every test binary the engine's property lane skips"
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
        // ADR 0029 states why the prebuild went and the run it outlasted.
        if !adr.contains("so the job runs no\n  prebuild") || !adr.contains("(run 43282569391)") {
            problems.push("ADR 0029 does not record why the job runs no prebuild".to_owned());
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
            3,
            "a tag moved without its header, comment and ADR is named three times"
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
        let unrecorded = adr.replacen("(run 43282569391)", "", 1);
        assert_ne!(
            unrecorded, adr,
            "the ADR text the control mutates must exist"
        );
        assert_eq!(
            inline_gate_problems(&gate, &unrecorded),
            vec!["ADR 0029 does not record why the job runs no prebuild".to_owned()],
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

    /// `ci.yml` is held to rule 7 through `check_parity`, and the repository's
    /// root fails when one of the other workflows makes the call.
    #[test]
    fn every_workflow_the_checker_reads_is_held_to_rule_7() -> TestResult {
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
            "ci.yml is held to rule 7"
        );
        Ok(())
    }
}
