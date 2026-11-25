//! The compile cache of every hosted job (ADR 0086): rustc runs through kache,
//! whose one content-addressed store lives in GHCR.
//!
//! Source: the Director, 2026-11-24: "Crates NEEDS to use Kache". GitHub holds
//! 10 GiB of cache per repository, and the per-group rust-cache entries already
//! fill 9 GiB (ADR 0041), so the clippy, docs, timing and feature lanes save
//! nothing and compile cold on every pull request. kache keys each rustc output
//! by its inputs, so one entry serves every job that builds the same unit.
//! Requirement proved: every job that compiles reads that store, a pull request
//! never writes it, the binary is the checksum-pinned release, and each job
//! ends by flushing its uploads and printing its hit rate.

use std::error::Error;
use std::fs;
use std::path::Path;

type TestResult = Result<(), Box<dyn Error>>;

/// The kache release the action installs.
const VERSION: &str = "2.3.0";

/// Each release asset and its SHA-256, from the release's own `.sha256` files.
const ASSETS: [(&str, &str); 6] = [
    (
        "kache-aarch64-unknown-linux-musl.tar.gz",
        "c3f8dfeef93659eabbb00f7d58cdf882e33560ae61306bb784158ccfa7d02bd5",
    ),
    (
        "kache-x86_64-unknown-linux-musl.tar.gz",
        "60b6c507a1bfd396baba8670e0612feb5dda2c8460c8f02644328dde64359701",
    ),
    (
        "kache-aarch64-apple-darwin.tar.gz",
        "66a078e0cb85f80a80a365b61a646791d6fe27f2000832194cadf06deddd6438",
    ),
    (
        "kache-x86_64-apple-darwin.tar.gz",
        "45e33861306133dfd970de7301ed1e560b2db18f999528e2416556064e3e3719",
    ),
    (
        "kache-x86_64-pc-windows-msvc.zip",
        "07b723787880c0de128f6e9d1f52ee4f96948a574f108c574a4f58728daacf59",
    ),
    (
        "kache-aarch64-pc-windows-msvc.zip",
        "943013b221d867742cb4dd2a8e157e323f9d0f2f3edf16daae98f47a57c61a4a",
    ),
];

/// The repository root.
fn repo() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// Each job of `text` with its body, a key two spaces under `jobs:`. The one
/// job reader of the workflow tests; `ci_changes` calls it too.
pub(crate) fn jobs(text: &str) -> Vec<(&str, String)> {
    let mut found: Vec<(&str, String)> = Vec::new();
    let mut in_jobs = false;
    for line in text.lines() {
        if !line.starts_with(' ') && !line.is_empty() {
            in_jobs = line == "jobs:";
            continue;
        }
        let key = line
            .strip_prefix("  ")
            .filter(|rest| !rest.starts_with([' ', '#']));
        match key.and_then(|rest| rest.strip_suffix(':')) {
            Some(id) if in_jobs => found.push((id, String::new())),
            _ => {
                if let Some(job) = found.last_mut() {
                    job.1.push_str(line);
                    job.1.push('\n');
                }
            }
        }
    }
    found
}

/// Every way the action, the report and the workflow can lose the cache, by
/// name. Empty means every compiling job reads the one store.
fn problems(action: &str, report: &str, workflow: &str) -> Vec<String> {
    let mut found = Vec::new();
    if action.contains("sccache") || workflow.contains("sccache") {
        found.push("a second compile cache, sccache, is still wired".to_owned());
    }
    if !action.contains(&format!("KACHE_VERSION: \"{VERSION}\"")) {
        found.push(format!("hosted-rust does not pin kache {VERSION}"));
    }
    for (asset, sum) in ASSETS {
        if !action.contains(&format!("asset={asset}; sum={sum}")) {
            found.push(format!("hosted-rust does not pin {asset} to its checksum"));
        }
    }
    // Each text a file must hold, and the problem its absence is.
    let required = [
        // Hashed from stdin: Git Bash printed `\` before a path's (run 45522616196).
        (
            action,
            "sha256sum < \"${dir}/${asset}\")\"; else got=\"$(shasum -a 256 < \"${dir}/${asset}\")\"; fi\n        got=\"${got%% *}\"\n        if [ \"${got}\" != \"${sum}\" ]; then",
            "hosted-rust runs a kache it never checked",
        ),
        (
            action,
            "echo \"RUSTC_WRAPPER=${bin}\"",
            "hosted-rust installs kache but never makes it the wrapper",
        ),
        (
            action,
            "type = \"oci\"\n        repository = \"ghcr.io/logical-works-inc/kache-cache\"\n",
            "hosted-rust's kache has no shared store",
        ),
        // Run 45441661494: kache's layout advice left a marker file in each
        // test's TMPDIR, and the residue census failed 17 Linux rows.
        (
            action,
            "        storage_layout_advice = false\n",
            "kache leaves its layout-advice marker in a test's TMPDIR",
        ),
        (
            action,
            "case \"${PARTITION:-}\" in \"\" | slice:1/*) slice=first ;; *) slice=later ;; esac\n          if [ \"${GITHUB_EVENT_NAME}\" = pull_request ] || [ \"${slice}\" = later ]; then\n            echo \"KACHE_REMOTE_READONLY=1\"\n          else\n            echo \"KACHE_S3_CONCURRENCY=4\"\n          fi\n",
            "a pull request or a repeated slice can write the shared store",
        ),
        (
            action,
            "echo \"KACHE_GITHUB_WRITES=merge_group\"",
            "the merge queue cannot write the shared store",
        ),
        // Run 45504674831: 125 s against 76 s for 8 crates (hosted-rust).
        (
            action,
            "echo \"KACHE_SCHEDULER=0\"",
            "kache's scheduler halves the one build a hosted job runs",
        ),
        // Run 45471907504: a pull request restored main's build from before
        // kache, then compiled 399 of 399 crates through the empty store.
        (
            action,
            "shared-key: ${{ inputs.cache-key }}-kache-${{ steps.deps-key.outputs.hash }}\n",
            "a build compiled through kache is not marked as one",
        ),
        (
            action,
            "if [ \"${EXACT}\" != true ] && [ -n \"${BASE}\" ] && [[ \"${BASE}\" != *-kache-* ]]; then",
            "kache runs in front of a build it did not compile, or is off where it did",
        ),
        (
            report,
            "\"${kache}\" daemon stop",
            "a job ends before kache's uploads drain",
        ),
        (
            report,
            "if ! \"${kache}\" stats --full --since 6h; then\n          echo \"::warning::kache stats failed\"\n        fi\n",
            "a job never prints its hit rate",
        ),
        (
            report,
            "{ grep -iE 'warn|error' \"${log}\" || true; } | tail -n 40",
            "a job never says why kache's remote failed",
        ),
        (
            workflow,
            "permissions:\n  contents: read\n  packages: write\n",
            "the workflow's token cannot write the shared store",
        ),
    ];
    for (text, needle, problem) in required {
        if !text.contains(needle) {
            found.push(problem.to_owned());
        }
    }
    // Run 45441661494: `stats` after the stop started a second daemon.
    if report.find("daemon stop") < report.rfind("\"${kache}\" stats") {
        found.push("the report starts a daemon after it drained the first".to_owned());
    }
    found.extend(job_problems(workflow));
    found
}

/// Each compiling job of `workflow` that skips the report or narrows its token.
fn job_problems(workflow: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (id, body) in jobs(workflow) {
        if !body.contains("uses: ./.github/actions/hosted-rust") {
            continue;
        }
        if !body.contains(
            "      - name: Upload the compile cache\n        if: ${{ always() }}\n        uses: ./.github/actions/kache-report\n",
        ) {
            found.push(format!("job {id} compiles but never reports its cache"));
        }
        if body.contains("    permissions:\n") && !body.contains("      packages: write\n") {
            found.push(format!("job {id} narrows its token below the shared store"));
        }
    }
    found
}

/// Each way of losing decisions 1 to 3: the file, the text a careless edit
/// replaces, what it leaves, and the problem [`problems`] must name.
const CONTROLS: [(&str, &str, &str, &str); 17] = [
    (
        "report",
        "grep -iE 'warn|error'",
        "grep -iE 'never'",
        "a job never says why kache's remote failed",
    ),
    (
        "action",
        "sha256sum < \"${dir}/${asset}\"",
        "sha256sum \"${dir}/${asset}\"",
        "hosted-rust runs a kache it never checked",
    ),
    (
        "action",
        "KACHE_VERSION: \"2.3.0\"",
        "KACHE_VERSION: \"2.2.0\"",
        "hosted-rust does not pin kache 2.3.0",
    ),
    (
        "action",
        "if [ \"${got}\" != \"${sum}\" ]; then",
        "if false; then",
        "hosted-rust runs a kache it never checked",
    ),
    (
        "action",
        "c3f8dfeef93659eabbb00f7d58cdf882e33560ae61306bb784158ccfa7d02bd5",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "hosted-rust does not pin kache-aarch64-unknown-linux-musl.tar.gz to its checksum",
    ),
    (
        "action",
        "[ \"${GITHUB_EVENT_NAME}\" = pull_request ] || ",
        "",
        "a pull request or a repeated slice can write the shared store",
    ),
    (
        "action",
        "*) slice=later ;;",
        "*) slice=first ;;",
        "a pull request or a repeated slice can write the shared store",
    ),
    (
        "action",
        "echo \"KACHE_S3_CONCURRENCY=4\"",
        "true",
        "a pull request or a repeated slice can write the shared store",
    ),
    (
        "action",
        "echo \"KACHE_SCHEDULER=0\"",
        "",
        "kache's scheduler halves the one build a hosted job runs",
    ),
    (
        "action",
        "        storage_layout_advice = false\n",
        "",
        "kache leaves its layout-advice marker in a test's TMPDIR",
    ),
    (
        "action",
        "-kache-${{ steps.deps-key.outputs.hash }}",
        "-${{ steps.deps-key.outputs.hash }}",
        "a build compiled through kache is not marked as one",
    ),
    (
        "action",
        " && [[ \"${BASE}\" != *-kache-* ]]",
        "",
        "kache runs in front of a build it did not compile, or is off where it did",
    ),
    (
        "action",
        "[ \"${EXACT}\" != true ] && ",
        "",
        "kache runs in front of a build it did not compile, or is off where it did",
    ),
    (
        "report",
        "\"${kache}\" daemon stop",
        "true",
        "a job ends before kache's uploads drain",
    ),
    (
        "report",
        "        if ! \"${kache}\" stats --full --since 6h; then",
        "        \"${kache}\" daemon stop\n        if ! \"${kache}\" stats --full --since 6h; then",
        "the report starts a daemon after it drained the first",
    ),
    (
        "workflow",
        "  contents: read\n  packages: write\n",
        "  contents: read\n",
        "the workflow's token cannot write the shared store",
    ),
    (
        "workflow",
        "      - name: Upload the compile cache\n        if: ${{ always() }}\n        uses: ./.github/actions/kache-report\n",
        "",
        "job workspace_all_targets compiles but never reports its cache",
    ),
];

#[test]
fn every_compiling_job_compiles_through_kache() -> TestResult {
    let action = fs::read_to_string(repo().join(".github/actions/hosted-rust/action.yml"))?;
    let report = fs::read_to_string(repo().join(".github/actions/kache-report/action.yml"))?;
    let workflow = fs::read_to_string(repo().join(".github/workflows/ci.yml"))?;
    assert_eq!(problems(&action, &report, &workflow), Vec::<String>::new());
    for (file, from, to, named) in CONTROLS {
        let (mut broken_action, mut broken_report, mut broken_workflow) =
            (action.clone(), report.clone(), workflow.clone());
        let text = match file {
            "action" => &mut broken_action,
            "report" => &mut broken_report,
            _ => &mut broken_workflow,
        };
        let changed = text.replacen(from, to, 1);
        assert_ne!(
            &changed, text,
            "the text the control mutates must exist: {from}"
        );
        *text = changed;
        let said = problems(&broken_action, &broken_report, &broken_workflow);
        assert!(
            said.iter().any(|problem| problem == named),
            "{named}: {said:?}"
        );
    }
    Ok(())
}
