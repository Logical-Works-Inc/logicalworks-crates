//! Seeded retire-mortgage simulation and the binaries' user journey (#788).
//!
//! One seed draws a register with five approvals: one with no retire table,
//! one with a bogus replacement, one with an expired review, one with a zero
//! cost, and one good twin. The seeded sweep requires each defect's typed
//! refusal and the twin's pass, with the same seed replaying to the same
//! BLAKE3 trace hash and distinct seeds diverging. The journey drives the
//! built `lgwks-deps check` and `lgwks-deps retire --report` binaries on a
//! scratch repo.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

use lgwks_deps::Refusal;
use lgwks_deps::contract::Contract;
use lgwks_deps::retire;

use crate::lgwks_scratch::{Scratch, run_deps};
use crate::replay::{Failure, refuse, replay_sweep};
use crate::sim::{Rng, Trace};

/// What every test here returns.
///
/// The workspace forbids `unwrap`/`expect` outright, with no test exemption,
/// so a test propagates its failure with `?` rather than aborting the run.
type TestResult = Result<(), Box<dyn Error>>;

/// Seeds swept by every family in this file.
const SEEDS: u64 = 64;

/// The date every seeded verdict is judged on: production passes the clock's
/// date, tests pass this, so an expiry never depends on the machine.
const TODAY: &str = "2026-11-02";

/// A review date after every run of this suite.
const GOOD_REVIEW: &str = "2027-02-01";

/// A review date before every run of this suite: the seeded sweep draws its own
/// stale dates from the seed, while the binary journey fixes this one.
const BAD_REVIEW: &str = "2026-01-01";

/// Replacements a seeded register draws from: every one resolves beside the
/// workspace root this suite runs on.
const REPLACEMENTS: [&str; 5] = [
    "std::vec",
    "lgwks_std::json",
    "lgwks_bot::rt",
    "lgwks_ast::parser",
    "lgwks_deps::sqlite",
];

/// Members a seeded audit judges replacements against.
fn members() -> Vec<String> {
    ["lgwks_std", "lgwks_bot", "lgwks_ast", "lgwks_deps"]
        .iter()
        .map(|name| name.to_string())
        .collect()
}

/// The workspace root: two levels above this package's manifest dir.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// One approval block for `name`, with `table` as its mortgage or nothing when
/// the entry carries no `[approved.retire]` table.
fn entry(name: &str, table: Option<&str>) -> String {
    let mut text = format!(
        concat!(
            "[[approved]]\n",
            "crate = \"{name}\"\n",
            "tier = \"boundary\"\n",
            "version = \"^1\"\n",
            "owner = \"lgwks_std\"\n",
            "capability = \"test.{name}\"\n",
            "license = \"MIT\"\n",
            "source = \"registry\"\n",
            "allowed_consumers = \"lgwks_std\"\n",
            "allowed_kinds = \"normal\"\n",
            "reason = \"The {name} edge supplies a capability the standard library cannot express.\"\n",
            "approved_by = \"reviewer\"\n",
            "approved_on = \"2026-10-26\"\n",
            "review = \"tests/it/sim_retire.rs\"\n",
        ),
        name = name,
    );
    if let Some(table) = table {
        text.push_str(table);
    }
    text
}

/// One `[approved.retire]` table with the drawn numbers.
fn table(replacement: &str, review: &str, cold: &str, crates: u32, bytes: u32) -> String {
    format!(
        concat!(
            "[approved.retire]\n",
            "replacement = \"{replacement}\"\n",
            "blocker = \"The drawn edge needs an estate module the tree does not own yet.\"\n",
            "cost = {{ cold_build_s = {cold}, crates = {crates}, binary_bytes = {bytes}, measured = \"2026-11-02\", command = \"cargo tree and cargo build with the edge on and off\" }}\n",
            "review = \"{review}\"\n",
        ),
        replacement = replacement,
        review = review,
        cold = cold,
        crates = crates,
        bytes = bytes,
    )
}

/// A seeded register with five approvals: one with no table, one with a bogus
/// replacement, one with an expired review, one with a zero cost, and one good
/// twin. `defects` false makes all five good twins instead.
fn register(rng: &mut Rng, defects: bool) -> Result<String, Box<dyn Error>> {
    let replacement = rng.pick_named("replacements", &REPLACEMENTS)?.to_string();
    let tenths = rng.between(1, 9999);
    let cold = format!("{}.{}", tenths.div_euclid(10), tenths % 10);
    let crates = rng.between(1, 500);
    let bytes = rng.between(1, 999_999_999);
    // A tag that names this seed's defects: the same seed replays the same
    // verdict, and distinct seeds draw distinct tags, so the trace hash
    // separates seeds without ever depending on the machine.
    let tag = rng.next_u64();
    let bogus = format!("bogus{tag:016x}::missing");
    let stale = format!(
        "2025-{:02}-{:02}",
        (tag % 12).saturating_add(1),
        (tag % 28).saturating_add(1)
    );
    let good = table(&replacement, GOOD_REVIEW, &cold, crates, bytes);
    let mut text = String::from("[policy]\nschema = 3\nenforce = true\n");
    if defects {
        text.push_str(&entry("missing", None));
        text.push_str(&entry(
            "bogus",
            Some(&table(&bogus, GOOD_REVIEW, &cold, crates, bytes)),
        ));
        text.push_str(&entry(
            "expired",
            Some(&table(&replacement, &stale, &cold, crates, bytes)),
        ));
        text.push_str(&entry(
            "zero",
            Some(&table(&replacement, GOOD_REVIEW, &cold, 0, bytes)),
        ));
        text.push_str(&entry("good", Some(&good)));
        Ok(text)
    } else {
        for name in ["missing", "bogus", "expired", "zero", "good"] {
            text.push_str(&entry(name, Some(&good)));
        }
        Ok(text)
    }
}

/// Requires the verdict to hold a refusal matching `pred`, naming the whole
/// verdict when it does not.
fn require(
    found: &[Refusal],
    what: &str,
    mut pred: impl FnMut(&Refusal) -> bool,
) -> Result<(), Failure> {
    if found.iter().any(&mut pred) {
        Ok(())
    } else {
        refuse(format!("{what} has no typed refusal in: {found:?}"))
    }
}

/// The canonical verdict of one seeded register: every refusal in order.
fn verdict(found: &[Refusal]) -> String {
    found
        .iter()
        .map(|refusal| refusal.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Requires each seeded defect's typed refusal, and folds the verdict's
/// BLAKE3 hash into `trace`.
fn run(rng: &mut Rng, trace: &mut Trace) -> Result<(), Failure> {
    let text = register(rng, true)?;
    let parsed = Contract::parse(&text)
        .map_err(|error| format!("a seeded register refused to parse: {error}"))?;
    let found = retire::refusals(&parsed, &members(), &workspace_root(), TODAY);
    require(
        &found,
        "a missing retire table",
        |refusal| matches!(refusal, Refusal::RetireMissing { krate, .. } if krate.as_str() == "missing"),
    )?;
    require(
        &found,
        "a bogus replacement",
        |refusal| matches!(refusal, Refusal::RetireInvalid { krate, field, .. } if krate.as_str() == "bogus" && field == "retire.replacement"),
    )?;
    require(
        &found,
        "an expired review",
        |refusal| matches!(refusal, Refusal::RetireExpired { krate, .. } if krate.as_str() == "expired"),
    )?;
    require(
        &found,
        "a zero cost",
        |refusal| matches!(refusal, Refusal::RetireInvalid { krate, field, .. } if krate.as_str() == "zero" && field == "retire.cost.crates"),
    )?;
    trace.record(&lgwks_std::hash::blake3(verdict(&found).as_bytes()).to_hex());
    Ok(())
}

#[test]
/// A seeded register refuses a missing table, a bogus replacement, an expired review and a zero cost, each with its typed refusal (#788).
fn a_seeded_register_refuses_each_defect_with_its_typed_refusal() -> TestResult {
    for seed in 1..=SEEDS {
        let mut trace = Trace::new();
        run(&mut Rng::new(seed), &mut trace).map_err(|error| format!("seed {seed}: {error}"))?;
    }
    Ok(())
}

#[test]
/// The same seed replays to the same retire trace hash (#788).
fn the_same_seed_replays_to_the_same_retire_trace() -> TestResult {
    replay_sweep("retire", SEEDS, run)?;
    Ok(())
}

#[test]
/// Distinct seeds diverge in their retire trace (#788).
fn distinct_seeds_diverge_in_their_retire_trace() -> TestResult {
    let mut seen: Vec<u64> = Vec::new();
    for seed in 1..=SEEDS {
        let mut trace = Trace::new();
        run(&mut Rng::new(seed), &mut trace).map_err(|error| format!("seed {seed}: {error}"))?;
        let receipt = crate::sim::receipt(&trace)
            .map_err(|error| format!("seed {seed} recorded nothing: {error}"))?;
        if seen.contains(&receipt) {
            return Err(format!("seed {seed} replays another seed's trace").into());
        }
        seen.push(receipt);
    }
    Ok(())
}

#[test]
/// The good twin of a seeded register passes with no retire refusal (#788).
fn the_good_twin_of_a_seeded_register_passes() -> TestResult {
    let text = register(&mut Rng::new(SEEDS), false)?;
    let parsed = Contract::parse(&text)?;
    let found = retire::refusals(&parsed, &members(), &workspace_root(), TODAY);
    if found.is_empty() {
        Ok(())
    } else {
        Err(format!("the good twin is refused: {found:?}").into())
    }
}

/// A scratch repo whose one path edge carries the mortgage `review`.
fn scratch_repo(label: &str, review: &str) -> Result<(Scratch, PathBuf), Box<dyn Error>> {
    let scratch = Scratch::fresh(label)?;
    let root = scratch.dir().join("retireapp");
    let helper = scratch.dir().join("helper");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::create_dir_all(helper.join("src"))?;
    std::fs::create_dir_all(root.join("contract"))?;
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"retireapp\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = { path = \"../helper\" }\n\n[workspace]\n",
    )?;
    std::fs::write(
        helper.join("Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n",
    )?;
    std::fs::write(root.join("src/lib.rs"), "")?;
    std::fs::write(helper.join("src/lib.rs"), "")?;
    // Cargo reports a path dependency in its canonical form, with the
    // platform's temporary-directory symlink resolved, so the approved origin
    // is the canonical path rather than the lexical scratch spelling.
    let origin = std::fs::canonicalize(&helper)?.display().to_string();
    std::fs::write(
        root.join("contract/APPROVED.toml"),
        format!(
            concat!(
                "[policy]\nschema = 3\nenforce = true\naccepted_licenses = \"MIT\"\n\n",
                "[[approved]]\n",
                "crate = \"helper\"\n",
                "tier = \"boundary\"\n",
                "version = \"*\"\n",
                "owner = \"retireapp\"\n",
                "capability = \"test.helper\"\n",
                "license = \"MIT\"\n",
                "source = \"path\"\n",
                "origin = \"{origin}\"\n",
                "allowed_consumers = \"retireapp\"\n",
                "allowed_kinds = \"normal\"\n",
                "reason = \"The scratch helper stands in for an edge the standard library cannot express.\"\n",
                "approved_by = \"reviewer\"\n",
                "approved_on = \"2026-10-26\"\n",
                "review = \"tests/it/sim_retire.rs\"\n",
                "[approved.retire]\n",
                "replacement = \"std::vec\"\n",
                "blocker = \"The scratch edge needs an estate module the tree does not own yet.\"\n",
                "cost = {{ cold_build_s = 0.10, crates = 1, binary_bytes = 100, measured = \"2026-11-02\", command = \"the sim_retire scratch probe\" }}\n",
                "review = \"{review}\"\n",
            ),
            origin = origin,
            review = review,
        ),
    )?;
    // The scratch repo has no lock yet; the gate's own metadata call is
    // `--locked`, so the lock is written here, offline, before it runs.
    let output = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(&root)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let refusal = Err(format!("the scratch repo has no lockfile: {stderr}").into());
        lgwks_std::trace::debug!(%stderr, "sim_retire: the scratch lock was not written");
        return refusal;
    }
    Ok((scratch, root))
}

#[test]
/// The check binary refuses an expired mortgage on a scratch repo (#788).
fn the_check_binary_refuses_an_expired_mortgage_on_a_scratch_repo() -> TestResult {
    let (_scratch, root) = scratch_repo("retire-expired", BAD_REVIEW)?;
    let outcome = run_deps(&root, &["check", "."])?;
    if outcome.code != Some(2) {
        return Err(format!(
            "an expired mortgage must exit 2, not {:?}:\nstdout: {:?}\nstderr: {:?}",
            outcome.code,
            String::from_utf8_lossy(&outcome.stdout),
            outcome.stderr,
        )
        .into());
    }
    if !outcome.stderr.contains("retire.review") {
        return Err(format!(
            "the refusal must name the lapsed mortgage field:\n{}",
            outcome.stderr
        )
        .into());
    }
    if !outcome.stdout.is_empty() {
        return Err(format!(
            "a refused audit writes nothing to stdout: {:?}",
            String::from_utf8_lossy(&outcome.stdout)
        )
        .into());
    }
    Ok(())
}

#[test]
/// The check binary passes the good twin on a scratch repo (#788).
fn the_check_binary_passes_the_good_twin_on_a_scratch_repo() -> TestResult {
    let (_scratch, root) = scratch_repo("retire-good", GOOD_REVIEW)?;
    let outcome = run_deps(&root, &["check", "."])?;
    if outcome.code != Some(0) {
        return Err(format!(
            "the good twin must exit 0, not {:?}:\nstdout: {:?}\nstderr: {:?}",
            outcome.code,
            String::from_utf8_lossy(&outcome.stdout),
            outcome.stderr,
        )
        .into());
    }
    let stdout = String::from_utf8(outcome.stdout)?;
    if !stdout.contains("OK") {
        return Err(format!("the good verdict must read OK:\n{stdout}").into());
    }
    Ok(())
}

#[test]
/// The retire binary reports every edge with its measured cost as one JSON object (#788).
fn the_retire_binary_reports_every_edge_with_its_measured_cost_as_json() -> TestResult {
    let (_scratch, root) = scratch_repo("retire-report", GOOD_REVIEW)?;
    let text = run_deps(&root, &["retire", "--report"])?;
    if text.code != Some(0) {
        return Err(format!(
            "the text report must exit 0, not {:?}:\n{}",
            text.code, text.stderr
        )
        .into());
    }
    let stdout = String::from_utf8(text.stdout)?;
    // The totals sum per-edge measurements, so they must say they bound the
    // saving from above rather than read as the exact distance to zero.
    for needle in [
        "edge helper",
        "replacement",
        "totals",
        "basis",
        "upper bound",
    ] {
        if !stdout.contains(needle) {
            return Err(format!("the text report must name {needle}:\n{stdout}").into());
        }
    }
    let json = run_deps(&root, &["retire", "--report", "--json"])?;
    if json.code != Some(0) {
        return Err(format!(
            "the JSON report must exit 0, not {:?}:\n{}",
            json.code, json.stderr
        )
        .into());
    }
    let document: lgwks_std::json::Value = lgwks_std::json::from_slice(&json.stdout)?;
    if document.is_null() {
        return Err("the JSON report must be one object, not null".into());
    }
    let rendered = lgwks_std::json::to_string(&document)?;
    for needle in [
        "\"crate\":\"helper\"",
        "\"replacement\":\"std::vec\"",
        "\"cold_build_s\":0.1",
        "\"totals\"",
        "\"basis\":\"sum of per-edge measurements",
    ] {
        if !rendered.contains(needle) {
            return Err(format!("the JSON report must carry {needle}:\n{rendered}").into());
        }
    }
    Ok(())
}
