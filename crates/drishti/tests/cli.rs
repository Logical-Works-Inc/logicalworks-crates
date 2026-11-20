//! The `drishti` command, driven as a user and an agent drive it: the built
//! binary, its JSON on stdout, its refusals on stderr and its exit status.
//!
//! Sources: the replay requirement of `docs/ink-engine.md` (one seed, one
//! geometry), the device-epoch contract of `docs/gpu-runtime.md` (no receipt
//! from an old epoch after recovery), WCAG 2.2 §1.4.3 for the contrast table,
//! and the estate rule that every default serves an agent (`--json`) with no
//! mode switch.

use std::process::{Command, Output};

use lgwks_std::json::{self, Value};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Run the shipped binary with `args`.
fn drishti(args: &[&str]) -> Result<Output, std::io::Error> {
    Command::new(env!("CARGO_BIN_EXE_drishti"))
        .args(args)
        .output()
}

/// Run it and parse a successful document.
fn document(args: &[&str]) -> Result<Value, Box<dyn std::error::Error>> {
    let output = drishti(args)?;
    assert!(output.status.success(), "{args:?} failed: {output:?}");
    Ok(json::from_slice(&output.stdout)?)
}

/// The member at `path`; a missing member is an error, never a null.
fn at<'a>(value: &'a Value, path: &[&str]) -> Result<&'a Value, Box<dyn std::error::Error>> {
    let mut current = value;
    for key in path {
        current = current
            .get(*key)
            .ok_or_else(|| format!("missing member `{key}` in {value}"))?;
    }
    Ok(current)
}

/// The elements of the array at `key`; absent or not an array yields none.
fn items<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// Every effect's digest, in brand order.
fn digests(document: &Value) -> Vec<String> {
    items(document, "effects")
        .filter_map(|effect| effect.get("digest").map(ToString::to_string))
        .collect()
}

#[test]
fn ink_replays_by_seed_and_stays_inside_its_bounds() -> TestResult {
    let first = document(&["ink", "--seed", "7", "--frames", "90"])?;
    let again = document(&["ink", "--seed", "7", "--frames", "90"])?;
    let other = document(&["ink", "--seed", "8", "--frames", "90"])?;
    assert_eq!(digests(&first).len(), 11, "every effect ran");
    assert_eq!(digests(&first), digests(&again), "one seed, one geometry");
    assert_ne!(
        digests(&first),
        digests(&other),
        "the seed reaches the geometry"
    );
    for effect in items(&first, "effects") {
        assert_eq!(*at(effect, &["within_bound"])?, true, "{effect}");
        // A finished effect stops demanding frames (reveal settles near frame
        // 62), so every effect rendered at least once and never past the run.
        let rendered = at(effect, &["rendered"])?.as_u64().ok_or("no count")?;
        assert!((1..=90).contains(&rendered), "{effect}");
        assert!(effect.get("refused").is_none(), "{effect}");
    }
    Ok(())
}

#[test]
fn reduced_motion_resolves_the_mark_and_the_count() -> TestResult {
    let resolved = document(&["ink", "--reduce-motion", "--frames", "2"])?;
    let effects = at(&resolved, &["effects"])?
        .as_array()
        .ok_or("no effects")?;
    let named = |name: &str| {
        effects.iter().find(|effect| {
            effect
                .get("effect")
                .is_some_and(|candidate| *candidate == name)
        })
    };
    let mark = named("mark").ok_or("no mark")?;
    assert_eq!(
        *at(mark, &["peak_shapes"])?,
        24,
        "the published twenty-four beams"
    );
    let count = named("count").ok_or("no count")?;
    assert_eq!(
        *at(count, &["label"])?,
        "$12,480.5",
        "grouped, prefixed, settled"
    );
    Ok(())
}

#[test]
fn a_session_survives_loss_and_never_presents_a_stale_receipt() -> TestResult {
    let session = document(&["present", "--seed", "3", "--frames", "120"])?;
    assert_eq!(*at(&session, &["hosts"])?, 11);
    assert_eq!(
        at(&session, &["refused_setup"])?.as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(
        *at(&session, &["epochs"])?,
        json::json!([1, 2]),
        "one recovery, one bump"
    );
    assert_eq!(
        *at(&session, &["refused_unavailable"])?,
        2,
        "draining and lost refuse"
    );
    assert_eq!(*at(&session, &["stale_receipts"])?, 0);
    assert_eq!(*at(&session, &["state"])?, "ready");
    assert_eq!(*at(&session, &["totals", "frames_presented"])?, 120);
    assert_eq!(*at(&session, &["present_ms", "samples"])?, 122);
    let again = document(&["present", "--seed", "3", "--frames", "120"])?;
    assert_eq!(
        at(&session, &["digest"])?,
        at(&again, &["digest"])?,
        "receipts replay by seed"
    );
    Ok(())
}

#[test]
fn tokens_publish_every_role_in_both_themes_with_grades() -> TestResult {
    let tokens = document(&["tokens"])?;
    assert_eq!(
        at(&tokens, &["primitives"])?.as_array().map(Vec::len),
        Some(18)
    );
    for primitive in items(&tokens, "primitives") {
        assert_eq!(*at(primitive, &["round_trips"])?, true, "{primitive}");
    }
    assert_eq!(
        at(&tokens, &["colourways"])?.as_array().map(Vec::len),
        Some(9)
    );
    for theme in items(&tokens, "themes") {
        assert_eq!(at(theme, &["roles"])?.as_array().map(Vec::len), Some(18));
        // Body text on the canvas meets AAA (7:1) in both themes.
        assert_eq!(
            *at(theme, &["contrast", "text-on-canvas", "text"])?,
            "AAA",
            "{theme}"
        );
    }
    let css = at(&tokens, &["css"])?.as_str().ok_or("no css")?;
    assert!(css.contains("--lw-p-clay-500: #c84025;"), "{css}");
    assert!(css.contains("[data-theme=\"dark\"]"), "{css}");
    Ok(())
}

#[test]
fn every_page_of_the_publication_is_readable() -> TestResult {
    let index = document(&["docs"])?;
    let pages = at(&index, &["pages"])?.as_array().ok_or("no index")?;
    assert_eq!(pages.len(), 10);
    for page in pages {
        let name = at(page, &["page"])?.as_str().ok_or("unnamed page")?;
        let read = document(&["docs", name])?;
        let markdown = at(&read, &["markdown"])?.as_str().ok_or("no markdown")?;
        assert!(markdown.len() > 500, "{name} is a placeholder");
    }
    Ok(())
}

#[test]
fn limits_report_the_declared_bounds() -> TestResult {
    let limits = document(&["limits"])?;
    assert_eq!(*at(&limits, &["ink", "max_shapes"])?, 512);
    assert_eq!(*at(&limits, &["ink", "max_beads"])?, 64);
    assert_eq!(*at(&limits, &["runtime", "conservative", "max_hosts"])?, 64);
    Ok(())
}

#[test]
fn malformed_input_is_refused_as_json_with_a_failing_status() -> TestResult {
    for args in [
        &[][..],
        &["bogus"][..],
        &["ink", "--frames", "x"][..],
        &["ink", "--frames", "36001"][..],
        &["ink", "--seed"][..],
        &["present", "--frames", "-1"][..],
        &["docs", "nope"][..],
    ] {
        let output = drishti(args)?;
        assert!(!output.status.success(), "{args:?} was accepted");
        assert!(output.stdout.is_empty(), "{args:?} wrote a document");
        let refusal: Value = json::from_slice(&output.stderr)?;
        assert!(
            at(&refusal, &["refused"])?.is_string(),
            "{args:?}: {refusal}"
        );
    }
    Ok(())
}
