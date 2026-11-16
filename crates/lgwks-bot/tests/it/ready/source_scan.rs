//! The source-scan family: the readiness module and the stdout observer arm no timer and poll nothing.

use super::*;

// ── No readiness sleeps in task code ────────────────────────────────────────

/// The readiness module arms no timer of its own, its wait re-reads the
/// readiness only after a wake, and the observer that gates a child's readiness
/// arms no timer either.
///
/// The row's "no readiness sleeps in task code" is a property of the source, so
/// it is checked against the source. Doc comments are excluded: they describe
/// the defects they rule out and are allowed to name them.
#[test]
fn no_readiness_path_sleeps_or_polls() -> TestResult {
    let code = read_module("src/script/ready")?;
    // `sleep(` and `while ` are the shapes a guessed readiness takes: a fixed
    // wait, and a re-read on a condition. A `loop` is *not* banned outright,
    // because the wait legitimately re-reads the channel after every wake from
    // it; the two halves of that claim are checked below rather than by
    // pattern-matching the word.
    for banned in ["sleep(", "while "] {
        assert!(
            !code.contains(banned),
            "the readiness module must contain no {banned:?}: a wait that sleeps or \
             re-reads on a condition is the defect this type replaces"
        );
    }
    // The wait's own bound goes through `within`, which does sleep — on the
    // caller's budget, not on a guess. That is the one timer the module may
    // reach, so its presence is asserted rather than merely tolerated.
    assert!(
        code.contains("within(scope"),
        "the readiness wait must be charged through the crate's own `within`, not a \
         hand-rolled timer"
    );

    // The loop in the wait is legitimate only because it awaits a change. Both
    // halves are asserted: it must await one, and it must not re-read on a
    // timer. A loop that did neither would spin, which is the defect.
    let subscribe = between(&code, "async fn subscribe", "fn resolve")?;
    assert!(
        subscribe.contains("watch::Receiver::changed"),
        "the wait must await a change on the readiness channel, or it is polling: \
         {subscribe}"
    );
    assert!(
        !subscribe.contains("time::sleep"),
        "the wait must not re-read the readiness on a timer: {subscribe}"
    );

    // The stdout observer that gates a child's readiness must not arm a timer
    // either: a readiness gated on a clock is the guess this type replaces.
    // The marker is a line of code, not a doc comment: `read_source` strips
    // comments, so a doc marker would never be found.
    let observer = between(
        &read_source("src/rt/supervise/drive.rs")?,
        "async fn capture<R>",
        "const MAX_OBSERVED_LINE_BYTES",
    )?;
    assert!(
        !observer.contains("sleep"),
        "the stdout observer must not arm a timer:\n{observer}"
    );
    Ok(())
}

/// A crate source file with its comment lines removed.
///
/// The removal is line-wise rather than by a parser because the claim is about
/// *code*: a `//` comment naming `sleep(` in prose is documentation, and a block
/// comment in this crate's idiom is used only for docs.
fn read_source(relative: &str) -> Result<String, Box<dyn Error>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    let source = std::fs::read_to_string(&path)?;
    Ok(source
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("//")
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// A module's source without comment lines: its root file, then each file in the
/// directory beside it in name order.
///
/// The readiness module is a root and four leaves, and the claim is about all of
/// it, so the leaves are read in a fixed order and joined.
fn read_module(stem: &str) -> Result<String, Box<dyn Error>> {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut leaves = std::fs::read_dir(base.join(stem))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    leaves.sort();
    let mut code = read_source(&format!("{stem}.rs"))?;
    for leaf in leaves {
        let name = leaf.to_string_lossy();
        code.push('\n');
        code.push_str(&read_source(&format!("{stem}/{name}"))?);
    }
    Ok(code)
}

/// The lines of `source` between two markers, as one string.
fn between(source: &str, from: &str, to: &str) -> Result<String, Box<dyn Error>> {
    let after = source
        .split_once(from)
        .map(|(_, rest)| rest)
        .ok_or_else(|| format!("the marker {from:?} is not in the file"))?;
    after
        .split_once(to)
        .map(|(body, _)| body.to_owned())
        .ok_or_else(|| format!("the marker {to:?} is not after {from:?}").into())
}
