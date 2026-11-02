//! Seeded mutations of the reference app through `lgwks check`.
//!
//! One seed draws one mutation of the app `lgwks new` writes — a single-edit
//! typo in a block keyword, an unknown word, a truncation, or a construct
//! past its ceiling — and every family asserts the same verdict twice: the
//! command never panics, replays byte-identical output for the seed, refuses
//! with the kind and location, and suggests the right word for single-edit
//! typos.

use std::error::Error;
use std::path::Path;

use crate::lgwks_scratch::{Outcome, Scratch, reference_app, run_lgwks};
use crate::replay::{Failure, refuse, replay_sweep};
use crate::sim::{Rng, Trace};

/// What every test here returns.
///
/// The workspace forbids `unwrap`/`expect` outright, with no test exemption,
/// so a test propagates its failure with `?` instead of aborting the run.
type TestResult = Result<(), Failure>;

/// Seeds per family: each seed starts the built binary twice, so the sweep
/// is narrower than the pure parsers' and still draws every outcome.
const SEEDS: u64 = 8;

/// Run `lgwks check` on `file` from `dir`, with `--json`, and require UTF-8
/// streams back.
fn check(dir: &Path, file: &Path) -> Result<Outcome, Failure> {
    let file = match file.to_str() {
        Some(file) => file,
        None => return refuse("a check needs a file with a string path"),
    };
    run_lgwks(dir, &["check", file, "--json"]).map_err(Failure::from)
}

/// The verdict's `ok` and its first refusal, or the refusal to read it.
fn verdict(stdout: &[u8]) -> Result<(bool, lgwks_std::json::Value), Failure> {
    let value: lgwks_std::json::Value = lgwks_std::json::from_slice(stdout)
        .map_err(|error| format!("lgwks check --json is not JSON: {error}"))?;
    match value.get("ok").and_then(lgwks_std::json::Value::as_bool) {
        Some(ok) => Ok((ok, value)),
        None => refuse("the verdict carries no boolean ok"),
    }
}

/// The only refusal's field, or the refusal to read it.
fn refusal_field<'a>(
    verdict: &'a lgwks_std::json::Value,
    name: &str,
) -> Result<&'a lgwks_std::json::Value, Failure> {
    let refusals = match verdict
        .get("refusals")
        .and_then(lgwks_std::json::Value::as_array)
    {
        Some(refusals) => refusals,
        None => return refuse("the verdict carries no refusals array"),
    };
    if refusals.len() != 1 {
        return refuse(format!(
            "the verdict carries {} refusals, not one",
            refusals.len()
        ));
    }
    match refusals.first().and_then(|first| first.get(name)) {
        Some(field) => Ok(field),
        None => refuse(format!("the refusal carries no {name}")),
    }
}

/// The string a refusal field carries, or the refusal to read it.
fn refusal_str<'a>(verdict: &'a lgwks_std::json::Value, name: &str) -> Result<&'a str, Failure> {
    match refusal_field(verdict, name)?.as_str() {
        Some(text) => Ok(text),
        None => refuse(format!("the refusal carries no string {name:?}")),
    }
}

/// Draw `below` below `bound`, refusing an empty range without drawing.
fn draw_below(rng: &mut Rng, bound: usize) -> Result<usize, Failure> {
    let bound = match u32::try_from(bound) {
        Ok(bound) if bound > 0 => bound,
        Ok(_) | Err(_) => return refuse("a draw needs a non-empty range"),
    };
    match usize::try_from(rng.below(bound)) {
        Ok(drawn) => Ok(drawn),
        Err(_) => refuse("a draw does not fit this target"),
    }
}

/// Pick one element of `table`, consuming one draw whatever the table holds.
fn pick<'table, Item>(rng: &mut Rng, table: &'table [Item]) -> Result<&'table Item, Failure> {
    let at = draw_below(rng, table.len())?;
    match table.get(at) {
        Some(item) => Ok(item),
        None => refuse("a pick landed past its table"),
    }
}

/// A seeded exit code and stream assertion: only 0 and 2 are answers, never
/// a panic (101) or a signal (no code).
fn assert_answered(outcome: &Outcome, because: &str) -> TestResult {
    match outcome.code {
        Some(0) | Some(2) => Ok(()),
        Some(code) => refuse(format!("{because}: exit {code}, a panic, not a verdict")),
        None => refuse(format!("{because}: no exit code, a signal, not a verdict")),
    }
}

/// The line numbers (0-based) whose first word is `word` and which open a
/// block: only a block opener reaches the language reader, so a typo there
/// is refused at the word rather than passing through as plain Rust.
fn keyword_lines(text: &str, word: &str) -> Vec<usize> {
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if !line.trim_end().ends_with(':') {
            continue;
        }
        let trimmed = line.trim_start_matches([' ', '\t']);
        let mut first = String::new();
        for cell in trimmed.chars() {
            if cell.is_alphanumeric() || cell == '_' {
                first.push(cell);
            } else {
                break;
            }
        }
        if first.as_str() == word {
            found.push(index);
        }
    }
    found
}

/// Replace the character at `at` of line `line` with `with`.
fn substitute(text: &str, line: usize, at: usize, with: char) -> Result<String, Failure> {
    let rows: Vec<&str> = text.lines().collect();
    let row = match rows.get(line) {
        Some(row) => row,
        None => return refuse("a mutation landed past its file"),
    };
    let mut cells: Vec<char> = row.chars().collect();
    match cells.get_mut(at) {
        Some(cell) => *cell = with,
        None => return refuse("a mutation landed past its line"),
    };
    let mut mutated = String::new();
    for (index, row) in rows.iter().enumerate() {
        if index == line {
            mutated.extend(cells.iter());
        } else {
            mutated.push_str(row);
        }
        mutated.push('\n');
    }
    Ok(mutated)
}

/// Write `mutated` over the reference app, check it twice, and require the
/// same verdict both times.
fn check_replays(
    dir: &Path,
    file: &Path,
    mutated: &str,
    trace: &mut Trace,
    seed: u64,
) -> Result<Outcome, Failure> {
    std::fs::write(file, mutated)
        .map_err(|error| format!("the mutated app could not be written: {error}"))?;
    let first = check(dir, file)?;
    let again = check(dir, file)?;
    if first.stdout != again.stdout {
        return refuse(format!("seed {seed}: the same file gave two verdicts"));
    }
    trace.record_number("seed", seed);
    match first.code {
        Some(code) => trace.record_number("code", code),
        None => trace.record("no-code"),
    }
    trace.record_number("bytes", first.stdout.len());
    Ok(first)
}

/// Write `mutated` as the app `name` in `scratch` for `seed`, check it twice,
/// and require a refused verdict (`what` names the mutation): answered, exit
/// 2, `ok` false, and its one refusal of `kind` when one is named. Answers the
/// verdict for the family's own assertions.
fn refused_verdict(
    (scratch, name, seed): (&Scratch, &str, u64),
    mutated: &str,
    trace: &mut Trace,
    (what, kind): (&str, Option<&str>),
) -> Result<lgwks_std::json::Value, Failure> {
    let file = scratch.dir().join(name).join("app.rs");
    let outcome = check_replays(scratch.dir(), &file, mutated, trace, seed)?;
    assert_answered(&outcome, what)?;
    if outcome.code != Some(2) {
        return refuse(format!("{what} was admitted"));
    }
    let (ok, value) = verdict(outcome.stdout.as_slice())?;
    if ok {
        return refuse(format!("the verdict on {what} claims ok"));
    }
    if let Some(kind) = kind
        && refusal_str(&value, "kind")? != kind
    {
        return refuse(format!("{what} is not {kind}"));
    }
    Ok(value)
}

/// One drawn line of `text` whose first word is `word`: its index, its text
/// and its indent width.
fn keyword_row<'text>(
    text: &'text str,
    word: &str,
    rng: &mut Rng,
) -> Result<(usize, &'text str, usize), Failure> {
    let lines = keyword_lines(text, word);
    if lines.is_empty() {
        return refuse(format!("the reference app names no {word} line to mutate"));
    }
    let line = *pick(rng, lines.as_slice())?;
    let row = match text.lines().nth(line) {
        Some(row) => row,
        None => return refuse("a mutation landed past its file"),
    };
    let indent = row
        .chars()
        .take_while(|cell| *cell == ' ' || *cell == '\t')
        .count();
    Ok((line, row, indent))
}

/// A single-edit typo in a block keyword is refused as unknown-word with the
/// right suggestion, at the same output on replay.
fn typo_suggests(trace: &mut Trace, rng: &mut Rng, seed: u64) -> TestResult {
    let scratch = Scratch::fresh("typo").map_err(|error| format!("no scratch: {error}"))?;
    let name = format!("typo{seed}");
    let reference = reference_app(scratch.dir(), name.as_str())?;
    let words = ["route", "let"];
    let word: &str = pick(rng, words.as_slice())?;
    let (line, row, indent) = keyword_row(reference.as_str(), word, rng)?;
    let at = indent.saturating_add(draw_below(rng, word.len())?);
    let alphabet: Vec<u8> = b"abcdefghijklmnopqrstuvwxyz".to_vec();
    let with = loop {
        let drawn = char::from(*pick(rng, alphabet.as_slice())?);
        let original = match row.chars().nth(at) {
            Some(original) => original,
            None => return refuse("a mutation landed past its word"),
        };
        if drawn != original {
            break drawn;
        }
    };
    let mutated = substitute(reference.as_str(), line, at, with)?;
    let target = (&scratch, name.as_str(), seed);
    let wanted = ("a typoed keyword", Some("unknown-word"));
    let value = refused_verdict(target, mutated.as_str(), trace, wanted)?;
    match refusal_field(&value, "suggestion")?.as_str() {
        Some(found) if found == word => {}
        Some(found) => return refuse(format!("a typo suggested {found}, not {word}")),
        None => return refuse("a typo earned no suggestion"),
    }
    trace.record("unknown-word");
    trace.record(word);
    Ok(())
}

/// An unknown word with no near neighbor is malformed with no suggestion.
fn unknown_word_refused(trace: &mut Trace, rng: &mut Rng, seed: u64) -> TestResult {
    let scratch = Scratch::fresh("unknown").map_err(|error| format!("no scratch: {error}"))?;
    let name = format!("unknown{seed}");
    let reference = reference_app(scratch.dir(), name.as_str())?;
    let (_, row, indent) = keyword_row(reference.as_str(), "route", rng)?;
    let mutated_line = format!(
        "{}frobnicate POST /drafts (body: Vec<u8>) -> Vec<u8>:",
        " ".repeat(indent)
    );
    let mutated = reference.replacen(row, mutated_line.as_str(), 1);
    let target = (&scratch, name.as_str(), seed);
    let wanted = ("an unknown word without a neighbor", Some("malformed"));
    let value = refused_verdict(target, mutated.as_str(), trace, wanted)?;
    if !refusal_field(&value, "suggestion")?.is_null() {
        return refuse("a far word earned a suggestion it should not have");
    }
    trace.record("malformed-far");
    Ok(())
}

/// A file cut mid-stream is malformed, never a panic, at the same output on
/// replay.
fn truncation_refused(trace: &mut Trace, rng: &mut Rng, seed: u64) -> TestResult {
    let scratch = Scratch::fresh("cut").map_err(|error| format!("no scratch: {error}"))?;
    let name = format!("cut{seed}");
    let reference = reference_app(scratch.dir(), name.as_str())?;
    let total = reference.len();
    let quarter = total.saturating_div(4);
    let half = total.saturating_div(2);
    let at = quarter.saturating_add(draw_below(rng, half.max(1))?);
    let mut cells: Vec<char> = reference.chars().collect();
    cells.truncate(at);
    let mutated: String = cells.into_iter().collect();
    let target = (&scratch, name.as_str(), seed);
    refused_verdict(target, mutated.as_str(), trace, ("a truncation", None))?;
    trace.record("malformed-cut");
    Ok(())
}

/// The flow's last line: insertions go before it, so they stay inside the
/// flow rather than landing after the script block ends.
const FLOW_TAIL: &str = "        give back served.port()\n";

/// A construct past its ceiling is refused naming the bound, at the same
/// output on replay.
fn bound_refused(trace: &mut Trace, rng: &mut Rng, seed: u64) -> TestResult {
    let scratch = Scratch::fresh("bound").map_err(|error| format!("no scratch: {error}"))?;
    let name = format!("bound{seed}");
    let reference = reference_app(scratch.dir(), name.as_str())?;
    let which = draw_below(rng, 3)?;
    let mut insert = String::new();
    let ceiling = if which == 0 {
        for index in 0..300 {
            insert.push_str(format!("        step s{index}:\n            1\n").as_str());
        }
        "256"
    } else if which == 1 {
        insert.push_str("        let report = graph:\n");
        for index in 0..70 {
            insert.push_str(format!("            node n{index}:\n                1\n").as_str());
        }
        "64"
    } else {
        let mut indent = 8_usize;
        for index in 0..40 {
            insert.push_str(format!("{}step d{index}:\n", " ".repeat(indent)).as_str());
            indent = indent.saturating_add(4);
        }
        insert.push_str(format!("{}1\n", " ".repeat(indent)).as_str());
        "32"
    };
    if !reference.contains(FLOW_TAIL) {
        return refuse("the reference app names no flow tail to extend");
    }
    let grown = reference.replacen(FLOW_TAIL, format!("{insert}{FLOW_TAIL}").as_str(), 1);
    let target = (&scratch, name.as_str(), seed);
    // One construct can pass several ceilings at once, so every refusal is
    // read below rather than one demanded here.
    let value = refused_verdict(
        target,
        grown.as_str(),
        trace,
        ("an unbounded construct", None),
    )?;
    let refusals = match value
        .get("refusals")
        .and_then(lgwks_std::json::Value::as_array)
    {
        Some(refusals) if refusals.is_empty() => {
            return refuse("an unbounded verdict carries no refusal");
        }
        Some(refusals) => refusals,
        None => return refuse("an unbounded verdict carries no refusals array"),
    };
    let mut saw_ceiling = false;
    for refusal in refusals {
        match refusal.get("kind").and_then(lgwks_std::json::Value::as_str) {
            Some("bound-exceeded") => {}
            Some(other) => {
                return refuse(format!("an unbounded arm is {other}, not bound-exceeded"));
            }
            None => return refuse("a refusal carries no kind"),
        }
        match refusal
            .get("message")
            .and_then(lgwks_std::json::Value::as_str)
        {
            Some(message) if message.contains(ceiling) => saw_ceiling = true,
            Some(_) | None => {}
        }
    }
    if !saw_ceiling {
        return refuse(format!("an unbounded construct names no ceiling {ceiling}"));
    }
    trace.record("bound-exceeded");
    trace.record(ceiling);
    Ok(())
}

/// The untouched reference app is admitted.
fn reference_admitted(trace: &mut Trace, rng: &mut Rng, seed: u64) -> TestResult {
    let _ = rng;
    let scratch = Scratch::fresh("valid").map_err(|error| format!("no scratch: {error}"))?;
    let name = format!("valid{seed}");
    let reference = reference_app(scratch.dir(), name.as_str())?;
    let file = scratch.dir().join(&name).join("app.rs");
    let outcome = check_replays(scratch.dir(), &file, reference.as_str(), trace, seed)?;
    assert_answered(&outcome, "the reference app")?;
    if outcome.code != Some(0) {
        return refuse("the reference app was refused");
    }
    let (ok, value) = verdict(outcome.stdout.as_slice())?;
    if !ok {
        return refuse("the reference verdict claims refusal");
    }
    match value
        .get("refusals")
        .and_then(lgwks_std::json::Value::as_array)
    {
        Some(refusals) if refusals.is_empty() => {}
        Some(_) | None => return refuse("the admitted verdict carries refusals"),
    }
    trace.record("admitted");
    Ok(())
}

#[test]
fn typo_suggests_the_right_word() -> Result<(), Box<dyn Error>> {
    replay_sweep("typo_suggests", SEEDS, |rng, trace| {
        let seed = rng.next_u64();
        typo_suggests(trace, rng, seed)
    })
}

#[test]
fn unknown_word_without_a_neighbor_is_malformed() -> Result<(), Box<dyn Error>> {
    replay_sweep("unknown_word", SEEDS, |rng, trace| {
        let seed = rng.next_u64();
        unknown_word_refused(trace, rng, seed)
    })
}

#[test]
fn truncation_is_malformed_never_a_panic() -> Result<(), Box<dyn Error>> {
    replay_sweep("truncation", SEEDS, |rng, trace| {
        let seed = rng.next_u64();
        truncation_refused(trace, rng, seed)
    })
}

#[test]
fn unbounded_constructs_name_their_ceiling() -> Result<(), Box<dyn Error>> {
    replay_sweep("bound", SEEDS, |rng, trace| {
        let seed = rng.next_u64();
        bound_refused(trace, rng, seed)
    })
}

#[test]
fn reference_app_is_admitted() -> Result<(), Box<dyn Error>> {
    replay_sweep("reference", SEEDS, |rng, trace| {
        let seed = rng.next_u64();
        reference_admitted(trace, rng, seed)
    })
}
