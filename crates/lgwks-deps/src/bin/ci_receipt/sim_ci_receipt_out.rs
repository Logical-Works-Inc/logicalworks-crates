//! Seeded simulations of the receipt `ci-receipt` writes: the cut to `--max`,
//! the bounded fields, the repro an agent runs, the markdown table and the
//! command line. Each is held to a model built from the generator. The sims of
//! how a log is read are in `sim_ci_receipt.rs`.
//!
//! Source: run 45430022570 and 45424270372. Fools: wrong-order, logs and flags
//! in any order; weird-bytes, a cut inside a character; saboteur, a hostile
//! test name and a cell that breaks the table.

use std::ffi::OsString;

use lgwks_std::seeded::Seeded;

use super::ci_receipt_tests::failure;
use super::extract::test_repro;
use super::failure::{Failure, Receipt};
use super::line::clip;
use super::sim_draw::{SEEDS, SimResult, below, pick, word};
use super::{parse_args, render};

/// The jobs the cut sim draws from.
const JOBS: [&str; 4] = ["a", "b", "build", "tests on linux"];

/// The files the cut sim draws from.
const FILES: [&str; 3] = ["a.rs", "b/c.rs", "z.rs"];

/// Source: run 45430022570. Fool: wrong-order. Bug caught: a cut that keeps
/// failures in the order the logs were read, drops one it should keep, or counts
/// the cut wrong; the model is a sort of everything at once.
#[test]
fn sim_ci_receipt_the_cut_keeps_the_sorted_first_max_in_any_order() -> SimResult {
    for seed in 0..SEEDS {
        let mut draw = Seeded::from_seed(seed);
        let total = below(&mut draw, 60)?;
        let max = below(&mut draw, 30)?.saturating_add(1);
        let mut all = Vec::new();
        for index in 0..total {
            let file = match below(&mut draw, 3)? {
                0 => None,
                _ => Some(*pick(&mut draw, &FILES)?),
            };
            let line = match file {
                Some(_) => Some(u32::try_from(below(&mut draw, 50)?)?),
                None => None,
            };
            let mut item = failure(pick(&mut draw, &JOBS)?, file, line);
            item.test = Some(format!("t{index}"));
            all.push(item);
        }
        let order = |items: &[Failure]| -> Vec<_> {
            items
                .iter()
                .map(|item| {
                    (
                        item.job.clone(),
                        item.file.clone(),
                        item.line,
                        item.test.clone(),
                    )
                })
                .collect()
        };
        let mut model = order(&all);
        model.sort();
        model.truncate(max);
        for at in (1..all.len()).rev() {
            let other = below(&mut draw, at.saturating_add(1))?;
            all.swap(at, other);
        }
        let mut receipt = Receipt::new(max);
        while !all.is_empty() {
            let take = below(&mut draw, 8)?.saturating_add(1).min(all.len());
            receipt.absorb(all.drain(..take).collect());
        }
        let kept = order(&receipt.failures);
        assert_eq!(kept, model, "seed {seed}: the first {max} in sorted order");
        let cut = total.saturating_sub(max);
        assert_eq!(receipt.cut, cut, "seed {seed}: the count cut");
    }
    Ok(())
}

/// Source: run 45430022570. Fool: weird-bytes. Bug caught: a cut by bytes that
/// splits a multi-byte character, or keeps more or fewer characters than asked.
#[test]
fn sim_ci_receipt_a_cut_never_splits_a_character() -> SimResult {
    let alphabet = ['a', '\u{e9}', '\u{65e5}', '\u{1f600}'];
    for seed in 0..SEEDS {
        let mut draw = Seeded::from_seed(seed);
        for _ in 0..40 {
            let length = below(&mut draw, 300)?;
            let text = word(&mut draw, &alphabet, length)?;
            let limit = below(&mut draw, 200)?;
            let cut = clip(&text, limit);
            let wanted = text.chars().count().min(limit);
            assert!(text.starts_with(&cut), "seed {seed}: the cut is a prefix");
            assert_eq!(cut.chars().count(), wanted, "seed {seed}: its length");
        }
    }
    Ok(())
}

/// The characters, beside letters and digits, a test name may hold in a repro:
/// written here, not read from the code under test.
const SAFE_PUNCTUATION: &str = "_:-./$<>#[]";

/// Source: run 45424270372. Fool: saboteur. Bug caught: a test name from a hostile
/// log reaching a repro an agent runs: a quote, a space, a substitution or a
/// newline must leave the repro empty, and a repro always has its two quotes.
#[test]
fn sim_ci_receipt_a_hostile_name_never_reaches_a_repro() -> SimResult {
    let alphabet = [
        'a', 'Z', '7', '_', ':', '-', '.', '/', '$', '<', '>', '#', '[', ']', ' ', '\'', ';', '`',
        '(', ')', '|', '&', '\n', '"', '\\', '*',
    ];
    for seed in 0..SEEDS {
        let mut draw = Seeded::from_seed(seed);
        for _ in 0..100 {
            let length = below(&mut draw, 12)?.saturating_add(1);
            let name = word(&mut draw, &alphabet, length)?;
            let repro = test_repro(None, "p::it", &name);
            let safe = name
                .chars()
                .all(|found| found.is_ascii_alphanumeric() || SAFE_PUNCTUATION.contains(found));
            let wanted = if safe {
                format!("cargo nextest run --locked -p p -E 'test(={name})'")
            } else {
                String::new()
            };
            assert_eq!(repro, wanted, "seed {seed}: the repro of {name:?}");
            let quotes = repro.matches('\'').count();
            assert!(
                matches!(quotes, 0 | 2),
                "seed {seed}: the quotes of {repro:?}"
            );
        }
    }
    Ok(())
}

/// Source: run 45424270372. Fool: saboteur. Bug caught: a cell that breaks the
/// table: a pipe, a backslash before a pipe, a newline, or a live mention. Every
/// row keeps its six unescaped pipes and its one line.
#[test]
fn sim_ci_receipt_no_cell_breaks_the_table() -> SimResult {
    let alphabet = ['a', '|', '\n', '@', '`', '\\', '\u{e9}', ' ', ':'];
    let repro_alphabet = ['a', '|', '\n', '`', ' ', '-'];
    for seed in 0..SEEDS {
        let mut draw = Seeded::from_seed(seed);
        let mut receipt = Receipt::new(50);
        let rows = below(&mut draw, 10)?.saturating_add(1);
        for _ in 0..rows {
            let length = below(&mut draw, 30)?;
            let job = word(&mut draw, &alphabet, length)?;
            let mut item = failure(&job, Some("f.rs"), Some(1));
            item.file = Some(word(&mut draw, &alphabet, length)?);
            item.test = Some(word(&mut draw, &alphabet, length)?);
            item.repro = word(&mut draw, &repro_alphabet, length)?;
            receipt.absorb(vec![item]);
        }
        let table = render::markdown(&receipt, 50);
        let body: Vec<&str> = table
            .lines()
            .filter(|text| text.starts_with("| "))
            .collect();
        let rows = receipt.failures.len().saturating_add(1);
        assert_eq!(body.len(), rows, "seed {seed}: a row each and the header");
        for text in body {
            let mut slashes = 0_usize;
            let mut bars = 0_usize;
            for found in text.chars() {
                if found == '|' && slashes.is_multiple_of(2) {
                    bars = bars.saturating_add(1);
                }
                slashes = match found {
                    '\\' => slashes.saturating_add(1),
                    _ => 0,
                };
            }
            let mention = text.contains("@ ") || text.contains("@a");
            assert_eq!(bars, 6, "seed {seed}: {text:?}");
            assert!(!mention, "seed {seed}: a live mention in {text:?}");
        }
    }
    Ok(())
}

/// Source: run 45430022570. Fool: wrong-order. Bug caught: a command line that
/// reads differently when its flags come in another order, or keeps an optional
/// flag it was not given.
#[test]
fn sim_ci_receipt_the_flags_mean_the_same_in_any_order() -> SimResult {
    for seed in 0..SEEDS {
        let mut draw = Seeded::from_seed(seed);
        let mut pairs = vec![("--logs", "l"), ("--out", "o")];
        let with_markdown = below(&mut draw, 2)? == 0;
        let with_max = below(&mut draw, 2)? == 0;
        if with_markdown {
            pairs.push(("--markdown", "m"));
        }
        if with_max {
            pairs.push(("--max", "7"));
        }
        for at in (1..pairs.len()).rev() {
            let other = below(&mut draw, at.saturating_add(1))?;
            pairs.swap(at, other);
        }
        let words: Vec<OsString> = pairs
            .iter()
            .flat_map(|pair| [pair.0, pair.1])
            .map(OsString::from)
            .collect();
        let args = parse_args(&words)?;
        let read = (
            args.logs.to_str(),
            args.out.to_str(),
            args.markdown.is_some(),
            args.max,
        );
        let max = if with_max { 7 } else { 50 };
        let wanted = (Some("l"), Some("o"), with_markdown, max);
        assert_eq!(read, wanted, "seed {seed}: paths, markdown and max");
    }
    Ok(())
}
