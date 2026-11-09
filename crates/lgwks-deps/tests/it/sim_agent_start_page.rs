//! Seeded fools against the start-page check (ADR 0022): the fat-finger who
//! renames a lane, the neighbour whose machine path leaks in, the saboteur who
//! hides a dead path in a fence. The oracle is the fault the generator planted,
//! never the check's own output. Mutation-style evidence after Pactflow's drift
//! testing.

use std::collections::BTreeSet;

use crate::agent_start_page::{Known, findings};
use crate::seed::{Rng, Trace};

/// How many pages each simulation draws.
const SEEDS: u64 = 2_048;

/// One thing a page can get wrong, and the word its finding must carry.
const FAULTS: [(&str, &str); 8] = [
    ("See `docs/gone.md` first.", "docs/gone.md"),
    ("Run `./scripts/gone.sh --fast`.", "scripts/gone.sh"),
    (
        "```sh\nprintf crates/gone/Cargo.toml\n```",
        "crates/gone/Cargo.toml",
    ),
    ("`./scripts/real.sh --lane gone-lane`", "gone-lane"),
    ("`cargo run --bin gone-bin`", "gone-bin"),
    ("`cargo publish -p gone_pkg --locked`", "gone_pkg"),
    ("[the guide](docs/gone-link.md#top)", "docs/gone-link.md"),
    ("Open /srv/someone/secret.md here.", "/srv/someone"),
];

/// Lines a healthy page holds, each naming only what exists or a placeholder.
const CLEAN: [&str; 8] = [
    "Plain prose with no path at all.",
    "`docs/real.md` and `scripts/real.sh --fast`.",
    "`./scripts/real.sh --lane real-lane`",
    "`cargo run --bin real-bin -p real_pkg`",
    "[a link](docs/real.md#section), and [a site](https://example.com/x).",
    "`crates/<name>/src` and `docs/**/*.md` name families.",
    "`--lane <id>` and `-p <crate>` are placeholders.",
    "`lgwks_std::json::serde` is a path in code, not in the tree.",
];

/// The tree the drawn pages are judged against.
fn known() -> Known {
    let set = |name: &str| BTreeSet::from([name.to_owned()]);
    Known {
        lanes: set("real-lane"),
        binaries: set("real-bin"),
        packages: set("real_pkg"),
    }
}

/// Whether the drawn tree holds `path`.
fn exists(path: &str) -> bool {
    ["docs/real.md", "scripts/real.sh"].contains(&path)
}

/// A place in a list of `width`, or `None` for a list too long to draw from.
fn draw(rng: &mut Rng, width: usize) -> Option<usize> {
    usize::try_from(rng.below(u32::try_from(width).ok()?)).ok()
}

/// A page of 1 to 16 healthy lines drawn from `rng`.
fn clean_lines(rng: &mut Rng) -> Vec<&'static str> {
    let mut lines = Vec::new();
    for _ in 0..rng.between(1, 16) {
        if let Some(line) = draw(rng, CLEAN.len()).and_then(|index| CLEAN.get(index)) {
            lines.push(*line);
        }
    }
    lines
}

/// One seed's page with one planted fault, judged; the trace records what it saw.
fn run(seed: u64) -> Trace {
    let mut rng = Rng::new(seed);
    let known = known();
    let mut trace = Trace::new();
    let mut lines = clean_lines(&mut rng);
    let healthy = lines.join("\n");
    assert_eq!(
        findings(&healthy, "", &known, &exists),
        Vec::<String>::new(),
        "seed {seed}: a healthy page was refused:\n{healthy}"
    );
    let (Some(index), Some(at)) = (draw(&mut rng, FAULTS.len()), draw(&mut rng, lines.len()))
    else {
        return trace;
    };
    let Some((fault, word)) = FAULTS.get(index).copied() else {
        return trace;
    };
    lines.insert(at, fault);
    let found = findings(&lines.join("\n"), "", &known, &exists);
    assert!(
        found.iter().any(|finding| finding.contains(word)),
        "seed {seed}: the fault {fault:?} went unnamed, found {found:?}"
    );
    trace.record_number("fault", index);
    trace.record_number("at", at);
    trace.record_number("found", found.len());
    trace
}

/// Catches a check that goes silent on any fault kind, or refuses a healthy line.
#[test]
fn sim_every_planted_fault_is_named_and_a_clean_page_is_silent() {
    let mut seen = BTreeSet::new();
    for seed in 0..SEEDS {
        let mut rng = Rng::new(seed);
        clean_lines(&mut rng);
        seen.insert(rng.below(8));
        run(seed);
    }
    assert_eq!(seen.len(), FAULTS.len(), "every fault kind was drawn");
}

/// Catches an unbalanced fence that flips every later line's reading.
#[test]
fn sim_a_dead_path_after_a_fence_is_still_named() {
    for seed in 0..SEEDS {
        let mut rng = Rng::new(seed);
        let mut lines = clean_lines(&mut rng);
        lines.push("```sh\n./scripts/real.sh --fast\n```");
        lines.push("Then read `docs/gone.md`.");
        let found = findings(&lines.join("\n"), "", &known(), &exists);
        assert_eq!(
            found,
            vec!["a dead path, docs/gone.md".to_owned()],
            "seed {seed}"
        );
    }
}

/// Catches a link judged from the root instead of from the page that holds it.
#[test]
fn sim_a_link_resolves_from_the_page_that_holds_it() {
    let inside = |path: &str| path == "skills/one/../two/SKILL.md";
    for seed in 0..SEEDS {
        let mut rng = Rng::new(seed);
        let lines = clean_lines(&mut rng).len();
        let page = "[next](../two/SKILL.md)\n".repeat(lines);
        assert_eq!(
            findings(&page, "skills/one", &known(), &inside),
            Vec::<String>::new()
        );
        assert_eq!(
            findings(&page, "", &known(), &inside).len(),
            lines,
            "seed {seed}"
        );
    }
}

/// One seed gives one trace and two seeds differ, so a failure replays exactly.
#[test]
fn sim_the_same_seed_replays_to_one_trace_and_seeds_diverge() {
    let mut hashes = BTreeSet::new();
    for seed in 0..SEEDS {
        let hash = run(seed).hash();
        assert_eq!(hash, run(seed).hash(), "seed {seed} replayed differently");
        hashes.insert(hash);
    }
    assert!(
        hashes.len() > 8,
        "seeds collapsed to {} traces",
        hashes.len()
    );
}
