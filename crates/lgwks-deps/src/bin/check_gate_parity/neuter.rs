//! Rule 7: no neutered lane.
//!
//! Rule 2 reads `||` as `&&` so that `a || b` still shows both of its parts to
//! the lane comparison. That made `cargo clippy ... || true` agree with its
//! lane while the step could no longer fail (#911 item 4). A lane the workflow
//! cannot fail is not enforced, so this rule refuses the spellings that make
//! one: a command whose failure is swallowed, and an `if:` or
//! `continue-on-error:` key that stops the job or step from running or from
//! failing the run.

use std::collections::BTreeMap;

use super::reader::Job;
use super::{is_gate_step, logical_lines};

/// The commands that succeed whatever ran before them.
const SUCCESS_FORMS: [&str; 5] = ["true", "/bin/true", "/usr/bin/true", ":", "exit 0"];

/// The text of `text` before the first `separator`.
fn before<'a>(text: &'a str, separator: &str) -> &'a str {
    match text.split_once(separator) {
        Some((head, _)) => head,
        None => text,
    }
}

/// Whether `operand` is a command that always succeeds, once the braces and
/// parentheses that group it are set aside.
fn always_succeeds(operand: &str) -> bool {
    let bare = operand
        .trim_matches(|ch: char| matches!(ch, '(' | ')' | '{' | '}' | ';') || ch.is_whitespace());
    SUCCESS_FORMS.contains(&bare)
}

/// Whether `command` only prints: `echo` and `printf` exit 0 whatever they
/// say, so a `|| echo failed` reports the failure and then swallows it.
fn only_prints(command: &str) -> bool {
    let first = command.split_whitespace().next();
    matches!(first, Some("echo" | "printf"))
}

/// The command whose status a `||` operand leaves: in a `{ ..; }` or `( .. )`
/// group the last command inside it, otherwise the command up to the `;` that
/// ends the list.
fn operand_status(piece: &str) -> &str {
    let operand = before(piece, "&&").trim_start();
    let Some(group) = operand
        .strip_prefix('{')
        .or_else(|| operand.strip_prefix('('))
    else {
        return before(operand, ";");
    };
    let inside = before(before(group, "}"), ")");
    match inside
        .split(';')
        .map(str::trim)
        .rfind(|command| !command.is_empty())
    {
        Some(last) => last,
        None => inside,
    }
}

/// What swallows the failure of a step body, if anything: the operand of a `||`
/// that always succeeds or only prints, or a last command that always succeeds.
fn neutered_command(run: &str) -> Option<String> {
    let lines = logical_lines(run);
    for line in &lines {
        for piece in line.split("||").skip(1) {
            let status = operand_status(piece);
            if always_succeeds(status) || only_prints(status) {
                return Some(format!("`|| {}`", status.trim()));
            }
        }
    }
    let last = lines.last()?.trim_end_matches([';', ' ']);
    let tail = match last.rsplit_once(';') {
        Some((_, tail)) => tail,
        None => last,
    };
    if always_succeeds(tail) {
        return Some(format!("a final `{}`", tail.trim()));
    }
    None
}

/// The rule-7 violations of a workflow's jobs. Empty means no lane is neutered.
///
/// A job's own keys and every `- run:` body are read whatever they are named.
/// A named step is read when the gate runs it or a lane claims it, so a
/// setup step may end `|| true` (the sccache statistics do).
pub(super) fn problems(jobs: &[Job], claimed: &BTreeMap<String, String>) -> Vec<String> {
    let mut found = Vec::new();
    for job in jobs {
        for finding in &job.findings {
            found.push(format!("ci.yml job {:?}: {finding}", job.name));
        }
        for step in &job.steps {
            let gate = is_gate_step(step) || claimed.contains_key(step.name.as_str());
            if gate && let Some(form) = neutered_command(&step.run) {
                found.push(format!(
                    "ci.yml step {:?} (job {:?}) cannot fail: it ends in {form}",
                    step.name, job.name
                ));
            }
        }
        for run in &job.loose_runs {
            if let Some(form) = neutered_command(run) {
                found.push(format!(
                    "ci.yml job {:?} has a `- run:` step that cannot fail: it ends in {form}",
                    job.name
                ));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::super::check_parity;

    /// A one-lane table: `cargo fmt --all -- --check` is the `Check formatting`
    /// step, local and in CI.
    const MANIFEST: &str = "[[lane]]\nid = \"format\"\nsurfaces = \"local ci\"\n\
                            command = \"cargo fmt --all -- --check\"\n\
                            ci_step = \"Check formatting\"\n";

    /// The workflow of that lane: `job_keys` sit on the job at four spaces and
    /// `steps` is the list under `steps:` at six, written by the test.
    fn workflow(job_keys: &str, steps: &str) -> String {
        format!(
            "jobs:\n  gate:\n    name: Gate checks\n    runs-on: ubuntu-24.04-arm\n{job_keys}    steps:\n{steps}"
        )
    }

    /// The format step with `step_keys` (eight spaces each) and `run` as its
    /// `run:` value.
    fn format_step(step_keys: &str, run: &str) -> String {
        format!(
            "      - uses: actions/checkout@v7\n      - name: Check formatting\n{step_keys}        run: {run}\n"
        )
    }

    /// The problems parity finds in the one-lane workflow.
    fn refusals(job_keys: &str, steps: &str) -> Vec<String> {
        check_parity(MANIFEST, &workflow(job_keys, steps))
    }

    /// Asserts exactly one problem and that it contains `needle`.
    fn assert_refused(problems: &[String], needle: &str) {
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems.iter().all(|problem| problem.contains(needle)),
            "{problems:?} should name {needle}"
        );
    }

    /// #911 item 4, the control: the clean workflow agrees, so each refusal
    /// below comes from the evasion and not from the fixture.
    #[test]
    fn a_clean_lane_agrees_with_its_workflow() {
        let clean = format_step("", "cargo fmt --all -- --check");
        assert_eq!(refusals("", &clean), Vec::<String>::new());
    }

    /// #911 item 4: `line.replace("||", "&&")` made `cmd || true` agree with its
    /// lane. Rule 7 refuses it by name, the segment comparison being unchanged.
    #[test]
    fn a_command_ending_or_true_is_refused() {
        let step = format_step("", "cargo fmt --all -- --check || true");
        assert_refused(&refusals("", &step), "|| true");
    }

    /// #911 item 4: `|| :` is `|| true` in the shell's other spelling.
    #[test]
    fn a_command_ending_or_colon_is_refused() {
        let step = format_step("", "cargo fmt --all -- --check || :");
        assert_refused(&refusals("", &step), "|| :");
    }

    /// #911 item 4: `|| exit 0` leaves the step green after a failed command.
    #[test]
    fn a_command_ending_or_exit_zero_is_refused() {
        let step = format_step("", "cargo fmt --all -- --check || exit 0");
        assert_refused(&refusals("", &step), "|| exit 0");
    }

    /// #911 item 4: a trailing `; true` replaces the step's status with its own.
    /// On one line rule 2 also sees a segment that is not the lane's command, so
    /// the test asks for rule 7's own report among the problems.
    #[test]
    fn a_trailing_semicolon_true_is_refused() {
        let step = format_step("", "cargo fmt --all -- --check; true");
        let problems = refusals("", &step);
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("a final `true`")),
            "{problems:?}"
        );
    }

    /// #911 item 4: the same, on a line of its own in a block `run: |`, and
    /// with a trailing semicolon after it.
    #[test]
    fn a_final_true_line_of_a_block_is_refused() {
        let block = "      - name: Check formatting\n        run: |\n          \
                     cargo fmt --all -- --check\n          true;\n";
        assert_refused(&refusals("", block), "a final `true`");
    }

    /// #911 item 4: `|| echo ...` prints the failure and exits 0, so the step is
    /// green; a group is judged by its last command, so `{ echo; true; }` and
    /// `( exit 0 )` are refused too.
    #[test]
    fn a_failure_that_is_only_printed_or_grouped_away_is_refused() {
        for (tail, named) in [
            ("|| echo clippy failed", "|| echo clippy failed"),
            ("|| printf 'bad\\n' >&2", "|| printf"),
            ("|| { echo bad >&2; true; }", "|| true"),
            ("|| { exit 0; }", "|| exit 0"),
            ("|| ( exit 0 )", "|| exit 0"),
        ] {
            let step = format_step("", &format!("cargo fmt --all -- --check {tail}"));
            assert_refused(&refusals("", &step), named);
        }
    }

    /// #911 item 4: `continue-on-error: true` on the step reports the job green
    /// whatever the step did.
    #[test]
    fn a_step_that_continues_on_error_is_refused() {
        let step = format_step(
            "        continue-on-error: true\n",
            "cargo fmt --all -- --check",
        );
        assert_refused(&refusals("", &step), "continue-on-error: true");
    }

    /// #911 item 4: the key comes after `run:` as readily as before it, and a
    /// quoted or `${{ }}` spelling of true is still true.
    #[test]
    fn a_continue_on_error_after_the_run_or_quoted_is_refused() {
        let after = "      - name: Check formatting\n        run: cargo fmt --all -- --check\n        \
                     continue-on-error: ${{ true }}\n";
        assert_refused(&refusals("", after), "continue-on-error: true");
        let quoted = "      - name: Check formatting\n        run: |\n          \
                      cargo fmt --all -- --check\n        continue-on-error: 'true'\n";
        assert_refused(&refusals("", quoted), "continue-on-error: true");
    }

    /// #911 item 4: the same key on the job.
    #[test]
    fn a_job_that_continues_on_error_is_refused() {
        let step = format_step("", "cargo fmt --all -- --check");
        assert_refused(
            &refusals("    continue-on-error: true\n", &step),
            "continue-on-error: true",
        );
    }

    /// #911 item 4: `if: false` on the step means the lane never runs.
    #[test]
    fn a_step_with_an_if_false_is_refused() {
        let step = format_step("        if: false\n", "cargo fmt --all -- --check");
        assert_refused(&refusals("", &step), "always false");
    }

    /// #911 item 4: `if: false` on the job; the quoted and `${{ }}` spellings and
    /// `false && ...` are the same condition.
    #[test]
    fn a_job_with_an_if_false_is_refused() {
        let step = format_step("", "cargo fmt --all -- --check");
        for condition in [
            "false",
            "${{ false }}",
            "'false'",
            "false && github.event_name",
        ] {
            let keys = format!("    if: {condition}\n");
            assert_refused(&refusals(&keys, &step), "always false");
        }
    }

    /// #911 item 4: a step that opens with `- if: false` is read like one that
    /// carries the key on a later line.
    #[test]
    fn a_step_opening_with_an_if_false_is_refused() {
        let steps = "      - if: false\n        name: Skipped\n        uses: actions/checkout@v7\n      \
                     - name: Check formatting\n        run: cargo fmt --all -- --check\n";
        assert_refused(&refusals("", steps), "always false");
    }

    /// #911 item 4: a `- run:` step has no name for a lane to claim, and was
    /// invisible to the reader; its `|| true` is refused all the same.
    #[test]
    fn an_unnamed_run_step_ending_or_true_is_refused() {
        let steps = format!(
            "{}      - run: cargo test --locked || true\n",
            format_step("", "cargo fmt --all -- --check")
        );
        assert_refused(&refusals("", &steps), "`- run:` step");
    }

    /// #911 item 4, the controls: conditions that can be true, a failure that is
    /// reported (`|| { ...; exit 1; }`, `|| exit 1`), and a setup step that ends
    /// `|| true` as `Cache hit rate` does in ci.yml, all stay in agreement.
    #[test]
    fn real_conditions_reported_failures_and_setup_steps_still_agree() {
        let steps = "      - uses: actions/checkout@v7\n      - name: Check formatting\n        \
                     if: ${{ always() && github.event_name != 'never' }}\n        \
                     continue-on-error: false\n        \
                     run: cargo fmt --all -- --check || { echo bad >&2; exit 1; }\n      \
                     - name: Cache hit rate\n        if: ${{ always() }}\n        \
                     run: command -v sccache >/dev/null 2>&1 && sccache --show-stats || true\n";
        let job = "    if: needs.changes.outputs.code == 'true'\n";
        assert_eq!(refusals(job, steps), Vec::<String>::new());
        let exit_one = format_step("", "cargo fmt --all -- --check || exit 1");
        assert_eq!(refusals("", &exit_one), Vec::<String>::new());
    }
}
