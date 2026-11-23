//! Simulated job graphs for rule 11 (#1095): one seed draws jobs, their
//! `needs:`, their cache keys and which of them run the doc tests, renders the
//! graph the way a careless author would write it, and rule 11 must report
//! exactly what a reference model computes from the drawn graph.
//!
//! The bug each fool exists for: a block-list or quoted `needs:` read as no
//! needs (fail open), a second doc-test job never checked, a doubled step or
//! need reported twice, a commented or quoted `Doc tests` or a job-shaped line
//! above `jobs:` taken for a job, and a panic on text that is no workflow.
//! The draw is `lgwks_std::seeded`; a failure prints its seed.

use std::error::Error;

use lgwks_std::seeded::{DrawError, Seeded};

use super::problems;

type TestResult = Result<(), Box<dyn Error>>;

/// Seeds each sweep draws.
const SEEDS: u64 = 2_000;

/// The fewest waiting pairs the fat-finger sweep must draw to prove anything.
const MIN_WAITERS: u64 = 500;

/// The job ids a graph draws from, the verdict among them.
const IDS: [&str; 10] = [
    "changes",
    "workspace_tail",
    "doc_tests",
    "debug_e2e",
    "build_verdict",
    "package_smoke",
    "gate",
    "tests-linux",
    "clippy_verdict",
    "x9",
];

/// The cache keys jobs draw from, so neighbours collide.
const KEYS: [&str; 4] = ["workspace", "doc-tests", "gate", "${{ matrix.archive }}"];

/// The one job rule 11 lets wait on the doc tests.
const VERDICT: &str = "build_verdict";

/// The rule's message when no job runs the doc tests.
const NO_STEP: &str = "ci.yml runs no `Doc tests` step";

/// One drawn job.
struct Spec {
    /// The job id.
    id: &'static str,
    /// The ids it waits on, each once.
    needs: Vec<&'static str>,
    /// Its cache keys, each once.
    keys: Vec<&'static str>,
    /// Whether it runs the doc tests.
    doc: bool,
}

/// How carelessly a graph is written.
#[derive(Clone, Copy)]
struct Fools {
    /// Doubles each doc step and the first need.
    double_clicker: bool,
    /// Writes `needs:` quoted, spaced, commented or as a block list.
    fat_finger: bool,
    /// Writes `needs:` after the steps.
    wrong_order: bool,
    /// Adds job-shaped lines above `jobs:`, comments and quoted doc steps.
    saboteur: bool,
}

/// `true` one time in `odds`.
fn chance(stream: &mut Seeded, odds: u64) -> Result<bool, DrawError> {
    Ok(stream.below(odds)? == 0)
}

/// A graph of 1 to `most` distinct jobs.
fn draw(stream: &mut Seeded, most: usize) -> Result<Vec<Spec>, DrawError> {
    let mut pool: Vec<&'static str> = IDS.to_vec();
    let count = stream.index(most.min(pool.len()))?.saturating_add(1);
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        let at = stream.index(pool.len())?;
        ids.push(pool.swap_remove(at));
    }
    let mut specs = Vec::with_capacity(count);
    for &id in &ids {
        let mut needs = Vec::new();
        for &other in &ids {
            if other != id && chance(stream, 3)? {
                needs.push(other);
            }
        }
        let mut keys = Vec::new();
        for &key in &KEYS {
            if chance(stream, 3)? {
                keys.push(key);
            }
        }
        let doc = chance(stream, 3)?;
        specs.push(Spec {
            id,
            needs,
            keys,
            doc,
        });
    }
    Ok(specs)
}

/// `needs` as `fools` write it.
fn render_needs(needs: &[&str], fools: Fools, stream: &mut Seeded) -> Result<String, DrawError> {
    let mut shown: Vec<String> = needs.iter().map(|id| (*id).to_owned()).collect();
    if fools.double_clicker
        && let Some(&first) = needs.first()
    {
        shown.push(first.to_owned());
    }
    if !fools.fat_finger || chance(stream, 3)? {
        if shown.len() == 1 {
            return Ok(format!("    needs: {}\n", shown.join("")));
        }
        return Ok(format!("    needs: [{}]\n", shown.join(", ")));
    }
    if chance(stream, 2)? {
        let quoted: Vec<String> = shown.iter().map(|id| format!("\"{id}\"")).collect();
        return Ok(format!(
            "    needs: [ {} ]  # fat-fingered\n",
            quoted.join(" ,")
        ));
    }
    let items: Vec<String> = shown.iter().map(|id| format!("      - {id}\n")).collect();
    Ok(format!("    needs:\n{}", items.concat()))
}

/// The workflow text of `specs`, written by `fools`.
fn render(specs: &[Spec], fools: Fools, stream: &mut Seeded) -> Result<String, DrawError> {
    let mut text = String::from("name: CI\non:\n  push:\n    branches: [main]\n");
    if fools.saboteur {
        text.push_str(
            "  doc_tests:\n    needs: [debug_e2e]\n    steps:\n      - name: Doc tests\n",
        );
    }
    text.push_str("jobs:\n");
    for spec in specs {
        if fools.saboteur {
            text.push_str("  # debug_e2e:\n      # - name: Doc tests\n");
        }
        text.extend(["  ", spec.id, ":\n    name: Job ", spec.id, "\n"]);
        let needs = render_needs(&spec.needs, fools, stream)?;
        if !fools.wrong_order {
            text.push_str(&needs);
        }
        text.push_str(
            "    runs-on: ubuntu-24.04-arm\n    steps:\n      - uses: actions/checkout@v7\n",
        );
        for key in &spec.keys {
            text.extend([
                "      - name: Pin Rust nightly\n        uses: ./.github/actions/hosted-rust\n        with:\n          cache-key: ",
                key,
                "\n",
            ]);
        }
        if spec.doc {
            text.push_str(
                "      - name: Doc tests\n        run: cargo test --workspace --doc --locked\n",
            );
            if fools.double_clicker {
                text.push_str("      - name: Doc tests\n        run: cargo test --doc\n");
            }
        }
        if fools.saboteur {
            text.push_str("      - name: Echo\n        run: echo \"- name: Doc tests\"\n");
        }
        if fools.wrong_order {
            text.push_str(&needs);
        }
    }
    Ok(text)
}

/// What rule 11 must report on `specs`, sorted: the reference model.
fn expected(specs: &[Spec]) -> Vec<String> {
    let mut want = Vec::new();
    if !specs.iter().any(|spec| spec.doc) {
        return vec![NO_STEP.to_owned()];
    }
    for doc in specs.iter().filter(|spec| spec.doc) {
        for job in specs {
            if job.id != VERDICT && job.needs.contains(&doc.id) {
                want.push(format!(
                    "{} waits on {}, which runs the doc tests: only {VERDICT} may (#1095)",
                    job.id, doc.id
                ));
            }
        }
        if doc.keys.is_empty() {
            want.push(format!("{} restores no dependency build (#1095)", doc.id));
        }
        for key in &doc.keys {
            for other in specs
                .iter()
                .filter(|other| other.id != doc.id && other.keys.contains(key))
            {
                want.push(format!(
                    "{}'s cache key {key} is also {}'s: the first saver owns the entry (#1095)",
                    doc.id, other.id
                ));
            }
        }
    }
    want.sort();
    want
}

/// Draws a graph and its fools from `seed`, returning what the rule reported
/// (sorted) and what the model wants.
fn trial(seed: u64, forced: Option<Fools>) -> Result<(Vec<String>, Vec<String>), DrawError> {
    let mut stream = Seeded::from_seed(seed);
    let fools = match forced {
        Some(fools) => fools,
        None => Fools {
            double_clicker: chance(&mut stream, 2)?,
            fat_finger: chance(&mut stream, 2)?,
            wrong_order: chance(&mut stream, 2)?,
            saboteur: chance(&mut stream, 2)?,
        },
    };
    let specs = draw(&mut stream, IDS.len())?;
    let text = render(&specs, fools, &mut stream)?;
    let mut got = problems(&text);
    got.sort();
    Ok((got, expected(&specs)))
}

/// The monkey with every other fool drawn per seed, the neighbours' keys
/// colliding: any graph, however written, gets exactly the model's report.
#[test]
fn sim_doc_path_monkey_any_drawn_graph_matches_the_reference_model() -> TestResult {
    for seed in 0..SEEDS {
        let (got, want) = trial(seed, None)?;
        assert_eq!(got, want, "seed {seed}");
    }
    Ok(())
}

/// The double-clicker doubles every doc step and need, written carelessly too:
/// each waiting pair is reported once, never twice.
#[test]
fn sim_doc_path_double_clicker_doubled_steps_and_needs_report_each_pair_once() -> TestResult {
    let fools = Fools {
        double_clicker: true,
        fat_finger: true,
        wrong_order: false,
        saboteur: false,
    };
    for seed in 0..SEEDS {
        let (got, want) = trial(seed, Some(fools))?;
        let mut unique = got.clone();
        unique.dedup();
        assert_eq!(
            unique.len(),
            got.len(),
            "seed {seed}: a report repeats: {got:?}"
        );
        assert_eq!(got, want, "seed {seed}");
    }
    Ok(())
}

/// The fat-finger writes block lists and quoted, commented needs after the
/// steps: a job that waits on the doc tests is never missed (fail closed).
#[test]
fn sim_doc_path_fat_finger_block_and_quoted_needs_are_never_missed() -> TestResult {
    let fools = Fools {
        double_clicker: false,
        fat_finger: true,
        wrong_order: true,
        saboteur: false,
    };
    let mut waiters = 0_u64;
    for seed in 0..SEEDS {
        let (got, want) = trial(seed, Some(fools))?;
        for line in want.iter().filter(|line| line.contains(" waits on ")) {
            assert!(got.contains(line), "seed {seed}: missed {line}: {got:?}");
            waiters = waiters.saturating_add(1);
        }
    }
    assert!(
        waiters >= MIN_WAITERS,
        "the sweep drew {waiters} waiters, too few to prove anything"
    );
    Ok(())
}

/// The saboteur writes job-shaped lines above `jobs:`, a commented job and a
/// quoted doc step, then feeds the rule text that is no workflow at all, from
/// empty to a ten-thousand-job chain: no panic, no invented doc job, and the
/// one real waiter in the chain is the only one named.
#[test]
fn sim_doc_path_saboteur_noise_and_giant_or_empty_text_invent_nothing() -> TestResult {
    let fools = Fools {
        double_clicker: false,
        fat_finger: false,
        wrong_order: false,
        saboteur: true,
    };
    for seed in 0..SEEDS {
        let (got, want) = trial(seed, Some(fools))?;
        assert_eq!(got, want, "seed {seed}");
    }
    let mut stream = Seeded::from_seed(SEEDS);
    let fragments = [
        "jobs:\n",
        "  a:\n",
        "    needs: [",
        "]\n",
        "- name: Doc tests\n",
        "\0",
        "\u{202e}",
        "      - \n",
        "    needs:\n",
        "#",
    ];
    for round in 0..SEEDS {
        let mut text = String::new();
        for _ in 0..stream.below(40)? {
            if let Some(fragment) = fragments.get(stream.index(fragments.len())?) {
                text.push_str(fragment);
            }
        }
        let found = problems(&text);
        if !text
            .lines()
            .any(|line| line.trim_start() == "- name: Doc tests")
        {
            assert_eq!(found, vec![NO_STEP.to_owned()], "round {round}: {text:?}");
        }
        assert!(
            found
                .iter()
                .all(|line| line == NO_STEP || line.ends_with("(#1095)")),
            "round {round}: {found:?}"
        );
    }
    assert_eq!(problems(""), vec![NO_STEP.to_owned()]);
    let mut chain = String::from("jobs:\n");
    for job in 0..10_000_u32 {
        let (id, before) = (job.to_string(), job.saturating_sub(1).to_string());
        chain.extend(["  j", &id, ":\n    needs: [j", &before, "]\n"]);
        chain.extend([
            "    steps:\n      - name: Pin\n        with:\n          cache-key: k",
            &id,
            "\n",
        ]);
        if job == 5_000 {
            chain.push_str("      - name: Doc tests\n");
        }
    }
    assert_eq!(
        problems(&chain),
        vec![
            "j5001 waits on j5000, which runs the doc tests: only build_verdict may (#1095)"
                .to_owned()
        ]
    );
    Ok(())
}
