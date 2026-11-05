//! Deterministic simulation of the missing-example detector and its ratchet.
//!
//! Source: Tokio's API documentation, where every public item carries an
//! `# Examples` block a reader copies, and the Director's order of 2026-11-04
//! that the docs be good enough for a model to write the next line from them
//! (ADR 0012). Requirement: INV-DOC-6, a public library item a change adds
//! shows a fenced example, and the items written before the rule are counted,
//! never blocked.
//!
//! One seed draws a file of items: each of the eight item kinds, at one of
//! three visibilities, with no doc, a prose doc or a doc with a fenced block,
//! sometimes `#[doc(hidden)]`, sometimes inside a `#[cfg(test)]` module, and
//! sometimes in a file rustdoc never documents. An independent model, written
//! from the rule's contract, names the line of every item that must be found;
//! the scan must agree line for line and replay to one trace. The shipped binary
//! then holds a change in a scratch repository to its added lines.

#![cfg(feature = "scan")]

use lgwks_ast::scan::{Rule, scan_source, scan_source_with};

#[cfg(feature = "tool")]
use crate::cli::{plant, text, tool};
#[cfg(feature = "tool")]
use crate::scratch::Scratch;
use crate::sim::{Rng, Trace, receipt};

/// What a test returns.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Seeds the agreement family draws, each one whole file.
const SEEDS: u64 = 2_048;

/// The most items one file holds.
const MAX_ITEMS: u32 = 12;

/// Each item kind, as the template its declaration is written from: `#` is
/// the item's index.
const KINDS: [&str; 8] = [
    "fn item_#() {}",
    "struct Item#;",
    "enum Item# { A }",
    "union Item# { a: u8 }",
    "trait Item# {}",
    "type Item# = u8;",
    "const ITEM_#: u8 = 1;",
    "static ITEM_#: u8 = 1;",
];

/// The visibilities an item is drawn at; only the first is judged.
const VISIBILITY: [&str; 3] = ["pub ", "pub(crate) ", ""];

/// The docs an item is drawn with, and whether each shows an example.
const DOCS: [(&str, bool); 5] = [
    ("", false),
    ("/// Says what it is for.\n", false),
    (
        "/// Says what it is for.\n///\n/// # Examples\n///\n/// ```\n/// let _ = 1;\n/// ```\n",
        true,
    ),
    ("/** Block doc.\n```\nlet _ = 1;\n``` */\n", true),
    ("#[doc = \"```\\nlet _ = 1;\\n```\"]\n", true),
];

/// Paths a file is drawn at, and whether rustdoc documents it.
const PATHS: [(&str, bool); 5] = [
    ("crates/a/src/lib.rs", true),
    ("crates/a/src/scan/doc.rs", true),
    ("crates/a/src/bin/tool/main.rs", false),
    ("crates/a/tests/it/main.rs", false),
    ("crates/a/examples/demo.rs", false),
];

/// One drawn file: its text, its path, and the lines the model says the
/// detector must report.
struct Drawn {
    source: String,
    path: &'static str,
    expected: Vec<usize>,
}

/// Draws one file from `seed`, recording every draw in `trace`.
fn draw(seed: u64, trace: &mut Trace) -> Result<Drawn, Box<dyn std::error::Error>> {
    let mut rng = Rng::new(seed);
    let &(path, documented) = rng.pick_named("paths", &PATHS)?;
    trace.record(path);
    let mut source = String::new();
    let mut expected = Vec::new();
    for index in 0..rng.between(1, MAX_ITEMS) {
        let kind = rng.pick_named("kinds", &KINDS)?;
        let visibility = rng.pick_named("visibility", &VISIBILITY)?;
        let &(doc, shows) = rng.pick_named("docs", &DOCS)?;
        let hidden = rng.coin();
        let in_test = rng.below(4) == 0;
        trace.record(&format!(
            "{index}:{kind}:{visibility}:{shows}:{hidden}:{in_test}"
        ));
        if in_test {
            source.push_str("#[cfg(test)]\nmod tests {\n");
        }
        source.push_str(doc);
        if hidden {
            source.push_str("#[doc(hidden)]\n");
        }
        let line = source.lines().count().saturating_add(1);
        source.push_str(visibility);
        source.push_str(&kind.replace('#', &index.to_string()));
        source.push('\n');
        if in_test {
            source.push_str("}\n");
        }
        if documented && *visibility == "pub " && !shows && !hidden && !in_test {
            expected.push(line);
        }
    }
    Ok(Drawn {
        source,
        path,
        expected,
    })
}

/// Every seeded file agrees with the model line for line, a whole scan never
/// reports the opt-in rule, and the family reaches both verdicts.
#[test]
fn sim_every_drawn_file_agrees_with_the_model() -> TestResult {
    let (mut found, mut clean) = (0_usize, 0_usize);
    for seed in 0..SEEDS {
        let drawn = draw(seed, &mut Trace::new())?;
        let hits = scan_source_with(&drawn.source, drawn.path, &Rule::OPT_IN)?;
        let lines: Vec<usize> = hits.iter().map(|hit| hit.line()).collect();
        assert_eq!(lines, drawn.expected, "seed {seed}:\n{}", drawn.source);
        assert!(
            hits.iter().all(|hit| hit.rule() == "missing-example"),
            "seed {seed}: {hits:?}"
        );
        assert!(
            scan_source(&drawn.source, drawn.path)?
                .iter()
                .all(|hit| hit.rule() != "missing-example"),
            "seed {seed}: a whole scan ran the opt-in rule"
        );
        if lines.is_empty() {
            clean = clean.saturating_add(1);
        } else {
            found = found.saturating_add(1);
        }
    }
    assert!(found > 0 && clean > 0, "found {found}, clean {clean}");
    Ok(())
}

/// The same seed draws the same file to the same trace; distinct seeds diverge.
#[test]
fn sim_the_same_seed_replays_to_one_trace() -> TestResult {
    let mut receipts = std::collections::BTreeSet::new();
    for seed in 0..SEEDS {
        let (mut first, mut second) = (Trace::new(), Trace::new());
        let drawn = draw(seed, &mut first)?;
        let again = draw(seed, &mut second)?;
        assert_eq!(drawn.source, again.source, "seed {seed}");
        assert_eq!(receipt(&first)?, receipt(&second)?, "seed {seed}");
        receipts.insert(receipt(&first)?);
    }
    assert!(receipts.len() > 1_000, "{} distinct traces", receipts.len());
    Ok(())
}

/// Runs `git` with `args` in `dir`, refusing a failed run.
#[cfg(feature = "tool")]
fn git(dir: &std::path::Path, args: &[&str]) -> TestResult {
    let status = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=sim",
            "-c",
            "user.email=sim@example.invalid",
        ])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git {args:?} exited {status}").into())
    }
}

/// The shipped binary holds a change to the lines it added: an older item with
/// no example is counted by a bare run and passed with `--base`, an added one
/// and an untracked file are refused at their lines, and a base git cannot read
/// is a refusal (exit 2), never a pass.
#[cfg(feature = "tool")]
#[test]
fn the_shipped_scan_holds_a_change_to_its_added_lines() -> TestResult {
    let tree = Scratch::new("scan-examples-base")?;
    let root = tree.path();
    plant(
        root,
        "src/lib.rs",
        "/// Old, with no example.\npub fn old() {}\n",
    )?;
    git(root, &["init", "-q"])?;
    git(root, &["add", "-A"])?;
    git(root, &["commit", "-q", "-m", "base"])?;
    plant(
        root,
        "src/lib.rs",
        "/// Old, with no example.\npub fn old() {}\n\n/// New, with none.\npub fn new() {}\n",
    )?;
    plant(root, "src/fresh.rs", "pub struct Fresh;\n")?;

    let bare = tool(root, &["scan", "--rule", "missing-example", "src"])?;
    let (out, err) = text(&bare);
    assert_eq!(bare.status.code(), Some(1), "{out}{err}");
    assert!(
        err.contains("REFUSED  scan: 3 findings across 2 files"),
        "{out}{err}"
    );

    let held = tool(
        root,
        &["scan", "--rule", "missing-example", "--base", "HEAD", "src"],
    )?;
    let (out, err) = text(&held);
    assert_eq!(held.status.code(), Some(1), "{out}{err}");
    assert!(
        out.contains("src/lib.rs:5: [missing-example] pub fn new"),
        "{out}"
    );
    assert!(
        out.contains("src/fresh.rs:1: [missing-example] pub struct Fresh"),
        "{out}"
    );
    assert!(!out.contains("pub fn old"), "{out}");
    assert!(
        err.contains("REFUSED  scan: 2 findings across 2 files"),
        "{out}{err}"
    );

    let unread = tool(
        root,
        &[
            "scan",
            "--rule",
            "missing-example",
            "--base",
            "no-such-rev",
            "src",
        ],
    )?;
    let (out, err) = text(&unread);
    assert_eq!(unread.status.code(), Some(2), "{out}{err}");
    assert!(err.contains("--base no-such-rev"), "{err}");
    Ok(())
}
