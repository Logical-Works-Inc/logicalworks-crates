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

    /// The call that failed every run is named by its line, the committed
    /// workflows make none, and this repository's own workflow and an action are
    /// not such a call.
    #[test]
    fn a_call_to_another_repositorys_reusable_workflow_is_refused() -> TestResult {
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
