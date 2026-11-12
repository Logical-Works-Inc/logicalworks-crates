//! Simulated split drift: one seed picks a test the `app-builds` lane moved
//! and one of three edits, and rule 6 must name that test for the edit that
//! loses or doubles it and say nothing for the control.
//!
//! Source: run 43144663118, where the scaffolded-app tests held a workspace
//! shard past its 5-minute cap, and ADR 0028, which moved them to rows of their
//! own. A selector the workspace lane stops excluding runs twice; one the
//! `apps` rows drop never runs in CI. The draw is `lgwks_std::seeded`, so a
//! failing seed replays exactly.

use std::error::Error;
use std::fmt;
use std::io;
use std::path::Path;

use lgwks_std::seeded::{DrawError, Seeded};

use super::lane_split::{LEFT_LANE, MOVED_LANE, split_problems, test_selectors};
use super::{MANIFEST_REL, WORKFLOW_REL, read_manifest};

/// Why a simulation could not read or draw what it needs.
#[derive(Debug)]
enum SimError {
    /// A repository file could not be read.
    Read(io::Error),
    /// The seeded stream refused a draw.
    Draw(DrawError),
    /// The committed files are not shaped as a drift assumes.
    Shape(&'static str),
}

impl fmt::Display for SimError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Read(ref cause) => write!(formatter, "reading a repository file: {cause}"),
            Self::Draw(ref cause) => write!(formatter, "the seeded stream refused: {cause}"),
            Self::Shape(what) => write!(formatter, "the files are not shaped for a drift: {what}"),
        }
    }
}

impl Error for SimError {}

impl From<io::Error> for SimError {
    fn from(cause: io::Error) -> Self {
        Self::Read(cause)
    }
}

impl From<DrawError> for SimError {
    fn from(cause: DrawError) -> Self {
        Self::Draw(cause)
    }
}

/// What every test here returns.
type TestResult = Result<(), SimError>;

/// Seeds each simulation sweeps.
const SEEDS: u64 = 64;

/// A selector no test matches, standing in for one a drift removed.
const GONE: &str = "test(=nothing::moved)";

/// The edit a seed makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edit {
    /// Nothing changes: the committed files must agree.
    Control,
    /// The workspace lane stops excluding the selector.
    Unexcluded,
    /// The `apps` rows stop running the selector.
    Unrun,
}

/// A committed file of the repository, by its path from the root.
fn read_repo(rel: &str) -> Result<String, SimError> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    Ok(std::fs::read_to_string(root.join(rel))?)
}

/// The selectors the committed `app-builds` lane runs.
fn moved(manifest: &str) -> Result<Vec<String>, SimError> {
    let lanes = read_manifest(manifest).lanes;
    let lane = lanes
        .iter()
        .find(|lane| lane.id == MOVED_LANE)
        .ok_or(SimError::Shape("the lane table holds no app-builds lane"))?;
    let command = lane
        .command
        .as_deref()
        .ok_or(SimError::Shape("the app-builds lane carries no command"))?;
    Ok(test_selectors(command))
}

/// `text` with `selector` replaced by [`GONE`] on the lines `keep` picks.
fn drop_on(text: &str, selector: &str, keep: impl Fn(&str) -> bool) -> String {
    text.lines()
        .map(|line| {
            if keep(line) {
                line.replace(selector, GONE)
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What one seed's edit made rule 6 say.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    /// The selector the seed drew.
    selector: String,
    /// The edit the seed drew.
    edit: Edit,
    /// What rule 6 said.
    problems: Vec<String>,
}

/// The verdict `seed` draws over the committed files.
fn verdict(seed: u64, manifest: &str, workflow: &str) -> Result<Verdict, SimError> {
    let selectors = moved(manifest)?;
    let mut stream = Seeded::from_seed(seed);
    let selector = selectors
        .get(stream.index(selectors.len())?)
        .ok_or(SimError::Shape("the app-builds lane selects no test"))?
        .clone();
    let edit = match stream.below(3)? {
        0 => Edit::Control,
        1 => Edit::Unexcluded,
        _ => Edit::Unrun,
    };
    let left_line = |line: &str| line.starts_with("command = ") && line.contains("not (package(");
    let (manifest, workflow) = match edit {
        Edit::Control => (manifest.to_owned(), workflow.to_owned()),
        Edit::Unexcluded => (drop_on(manifest, &selector, left_line), workflow.to_owned()),
        Edit::Unrun => (
            manifest.to_owned(),
            drop_on(workflow, &selector, |line| line.contains("suite: apps")),
        ),
    };
    Ok(Verdict {
        problems: split_problems(&manifest, &workflow),
        selector,
        edit,
    })
}

/// Catches a moved test the workspace lane runs again, and one CI never runs:
/// each edit is named by its selector, and the committed files say nothing.
#[test]
fn sim_a_moved_test_left_in_the_workspace_or_dropped_from_ci_is_named() -> TestResult {
    let manifest = read_repo(MANIFEST_REL)?;
    let workflow = read_repo(WORKFLOW_REL)?;
    let mut seen = Vec::new();
    for seed in 0..SEEDS {
        let case = verdict(seed, &manifest, &workflow)?;
        match case.edit {
            Edit::Control => assert_eq!(case.problems, Vec::<String>::new(), "seed {seed}"),
            Edit::Unexcluded | Edit::Unrun => {
                let word = if case.edit == Edit::Unexcluded {
                    "run twice"
                } else {
                    "never run"
                };
                assert_eq!(case.problems.len(), 1, "seed {seed}: {:?}", case.problems);
                assert!(
                    case.problems
                        .iter()
                        .any(|problem| problem.contains(&case.selector) && problem.contains(word)),
                    "seed {seed}: {word} names {} in {:?}",
                    case.selector,
                    case.problems
                );
            }
        }
        seen.push(case.edit);
    }
    for edit in [Edit::Control, Edit::Unexcluded, Edit::Unrun] {
        assert!(seen.contains(&edit), "the sweep drew {edit:?}");
    }
    Ok(())
}

/// Catches the clean-consumer gate journey going back into a workspace slice.
///
/// Source: run 43203981708 (#927). That one nested cargo build ran 42.7 s
/// behind a cold 165 s dependency build in slice 10/10, and the run's wall
/// read 309 s against 300 with every job green. A count slice cannot split
/// one test, so the `app-builds` lane holds it and rule 6 agrees.
#[test]
fn the_clean_consumer_gate_journey_runs_on_an_apps_row() -> TestResult {
    let manifest = read_repo(MANIFEST_REL)?;
    let workflow = read_repo(WORKFLOW_REL)?;
    let journey = "test(=gate_consumer::sim_a_clean_consumer_passes_the_gate)";
    assert!(
        moved(&manifest)?.iter().any(|selector| selector == journey),
        "{MOVED_LANE} runs {journey}"
    );
    assert!(
        workflow
            .lines()
            .any(|line| line.contains("suite: apps") && line.contains(journey)),
        "an apps row of the workflow runs {journey}"
    );
    assert_eq!(split_problems(&manifest, &workflow), Vec::<String>::new());
    Ok(())
}

/// Catches the `lockfile` lane going back to a path glob.
///
/// Source: main at fa726e2a, where `fixtures/*/Cargo.lock` matched 9 of the 16
/// tracked lockfiles and left the four bench roots and the three under
/// `contract-override/` unchecked. The lane and the workflow step both list
/// what git tracks, refuse an empty list, and check each root `--locked`
/// against the manifest beside it.
#[test]
fn the_lockfile_lane_reads_every_tracked_lockfile() -> TestResult {
    let manifest = read_repo(MANIFEST_REL)?;
    let workflow = read_repo(WORKFLOW_REL)?;
    let lanes = read_manifest(&manifest).lanes;
    let lane = lanes
        .iter()
        .find(|lane| lane.id == "lockfile")
        .ok_or(SimError::Shape("the lane table holds no lockfile lane"))?;
    let command = lane
        .command
        .as_deref()
        .ok_or(SimError::Shape("the lockfile lane carries no command"))?;
    let step = workflow
        .lines()
        .find(|line| line.contains("for f in $locks"))
        .ok_or(SimError::Shape("the workflow has no lockfile loop"))?;
    for (name, text) in [("lane", command), ("workflow", workflow.as_str())] {
        assert!(
            text.contains("locks=$(git ls-files '*Cargo.lock') && test -n \"$locks\""),
            "the {name} lists tracked lockfiles and refuses an empty list"
        );
    }
    for (name, text) in [("lane", command), ("workflow", step)] {
        assert!(
            text.contains("cargo metadata --locked")
                && text.contains("--manifest-path \"${f%.lock}.toml\""),
            "the {name} checks each root --locked against its own manifest"
        );
        assert!(
            !text.contains("fixtures/") && !text.contains("bench/"),
            "the {name} names no directory a lockfile must sit in"
        );
    }
    Ok(())
}

/// Catches the update skill telling a reader to look for a message the
/// `lockfile` lane no longer prints.
///
/// Source: #930 changed the lane's refusal from `stale fixture lockfile` to
/// `stale lockfile` and widened it to every tracked lockfile, while
/// `skills/lgwks-dependency-update/SKILL.md` still quoted the old message.
#[test]
fn the_update_skill_names_what_the_lockfile_lane_prints() -> TestResult {
    let manifest = read_repo(MANIFEST_REL)?;
    let skill = read_repo("skills/lgwks-dependency-update/SKILL.md")?;
    let lanes = read_manifest(&manifest).lanes;
    let command = lanes
        .iter()
        .find(|lane| lane.id == "lockfile")
        .and_then(|lane| lane.command.as_deref())
        .ok_or(SimError::Shape("the lockfile lane carries no command"))?;
    let printed = command
        .split("echo \\\"")
        .nth(1)
        .or_else(|| command.split("echo \"").nth(1))
        .and_then(|rest| rest.split(" $f").next())
        .ok_or(SimError::Shape("the lockfile lane prints no refusal"))?;
    assert!(
        skill.contains(&format!("`{printed}`")),
        "the skill's repair table quotes the lane's refusal {printed:?}"
    );
    assert!(
        skill.contains("every lockfile git\ntracks") || skill.contains("every lockfile git tracks"),
        "the skill says the lane checks every tracked lockfile"
    );
    Ok(())
}

/// Catches the simulation lane going back to compiled test archives.
///
/// Source: run 43228739672 (#931). Every bot-it shard uploaded its compiled
/// suite for the lane, 2 to 177 s each, and bot-it-g5 was cancelled at its
/// 5-minute cap at test 303 of 325 with every test passing (ADR 0032). The lane
/// only counts tests, so each suite's first shard uploads its JSON listing,
/// and every suite the waiter names is one listing the lane reads.
#[test]
fn the_simulation_lane_reads_listings_not_archives() -> TestResult {
    let manifest = read_repo(MANIFEST_REL)?;
    let workflow = read_repo(WORKFLOW_REL)?;
    for (name, text) in [
        ("lane table", manifest.as_str()),
        ("workflow", workflow.as_str()),
    ] {
        let commands = text
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'));
        for line in commands {
            for archive in [
                "cargo nextest archive",
                "--archive-file",
                "nextest-archive",
                "SIMULATION_ARCHIVES",
            ] {
                assert!(
                    !line.contains(archive),
                    "the {name} still runs {archive}: {line}"
                );
            }
        }
    }
    let lanes = read_manifest(&manifest).lanes;
    let command = lanes
        .iter()
        .find(|lane| lane.id == "simulation-listing")
        .and_then(|lane| lane.command.as_deref())
        .ok_or(SimError::Shape(
            "the lane table holds no simulation-listing command",
        ))?;
    assert!(
        command.contains("cargo nextest list ${NEXTEST_BUILD} --message-format json")
            && command.ends_with("> /tmp/nextest-listing/listing.json"),
        "the lane lists the suite it built as JSON, outside the checkout: {command}"
    );
    assert!(
        workflow
            .lines()
            .any(|line| line.trim() == format!("run: {command}")),
        "the workflow runs the lane's listing command"
    );
    let named = workflow
        .lines()
        .find_map(|line| line.trim().strip_prefix("SIMULATION_LISTINGS: "))
        .ok_or(SimError::Shape("the workflow names no SIMULATION_LISTINGS"))?;
    let waited = workflow
        .lines()
        .find_map(|line| line.trim().strip_prefix("for suite in "))
        .and_then(|rest| rest.strip_suffix("; do"))
        .ok_or(SimError::Shape("the workflow has no listing waiter"))?;
    let suites: Vec<&str> = waited.split_whitespace().collect();
    let files: Vec<&str> = named.split_whitespace().collect();
    let wanted: Vec<String> = suites
        .iter()
        .map(|suite| format!("nextest-listing/nextest-listing-{suite}-Linux/listing.json"))
        .collect();
    assert_eq!(
        files, wanted,
        "the lane reads one listing per waited suite, in order"
    );
    let mut distinct = suites.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        suites.len(),
        "no two suites share a listing"
    );
    let upload: Vec<&str> = workflow
        .lines()
        .skip_while(|line| !line.contains("name: Upload the suite listing"))
        .take(8)
        .collect();
    for wanted in [
        "name: nextest-listing-${{ matrix.archive }}-Linux",
        "retention-days: 1",
    ] {
        assert!(
            upload.iter().any(|line| line.trim() == wanted),
            "the listing upload keeps {wanted}: {upload:?}"
        );
    }
    Ok(())
}

/// Catches a verdict that depends on anything but the seed.
#[test]
fn sim_the_same_seed_replays_the_same_split_verdict() -> TestResult {
    let manifest = read_repo(MANIFEST_REL)?;
    let workflow = read_repo(WORKFLOW_REL)?;
    for seed in 0..SEEDS {
        assert_eq!(
            verdict(seed, &manifest, &workflow)?,
            verdict(seed, &manifest, &workflow)?,
            "seed {seed} replays"
        );
    }
    assert!(
        moved(&manifest)?.len() > 1,
        "{LEFT_LANE} gave {MOVED_LANE} more than one selector to draw"
    );
    Ok(())
}
