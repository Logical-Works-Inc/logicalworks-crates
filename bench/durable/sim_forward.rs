//! Simulations of the bench target's argument filter: cargo adds `--bench`, the
//! durable binary must see everything else, in order, untouched.

use std::ffi::OsString;

use crate::forward::forwarded;
use crate::sim_kit::{Dice, TestResult};

/// Words as the OS would hand them.
fn argv(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

/// The fool who types the flag himself, and the fool who types near it: only the
/// first exact `--bench` is cargo's, and every near miss goes through.
#[test]
fn sim_forward_double_clicker_only_the_first_bench_flag_is_dropped() {
    let table: [(&[&str], &[&str]); 11] = [
        (&[], &[]),
        (&["--bench"], &[]),
        (&["--bench", "--rows", "lgwks,restate", "--recover"], &["--rows", "lgwks,restate", "--recover"]),
        (&["--rows", "lgwks", "--bench"], &["--rows", "lgwks"]),
        (&["--bench", "--bench"], &["--bench"]),
        (&["--bench", "--bench", "--bench"], &["--bench", "--bench"]),
        (&["--runs", "--bench"], &["--runs"]),
        (&["--benches", "--bench=1", "-bench", "--BENCH", "--bench ", "", "bench"], &["--benches", "--bench=1", "-bench", "--BENCH", "--bench ", "", "bench"]),
        (&["--rows", "\u{1F980}", "--bench", "\n"], &["--rows", "\u{1F980}", "\n"]),
        (&["--markdown", "--runs", "20"], &["--markdown", "--runs", "20"]),
        (&["--write-doc", "--bench", "d.md"], &["--write-doc", "d.md"]),
    ];
    for (given, want) in table {
        assert_eq!(forwarded(argv(given)), argv(want), "{given:?}");
    }
}

/// The monkey: seeded argument lists that mix real flags, near misses and `--bench`
/// at any place and any number of times. The filter removes exactly the element at
/// the first `--bench`, if there is one, and keeps every other element in order.
#[test]
fn sim_forward_monkey_generated_lines_lose_exactly_the_cargo_flag() -> TestResult {
    let vocabulary = [
        "--bench", "--bench", "--rows", "lgwks,restate", "--recover", "--markdown", "--runs", "20", "--benchx", "-b",
        "", "--write-doc", "doc.md", "\u{200f}--bench", "--bench\u{0}",
    ];
    let mut dice = Dice::new(78);
    let mut dropped = 0_u32;
    for round in 0..4_000 {
        let mut words = Vec::new();
        for _ in 0..dice.pick(9)? {
            words.push(*vocabulary.get(dice.pick(vocabulary.len())?).ok_or("the draw left the vocabulary")?);
        }
        let first = words.iter().position(|word| *word == "--bench");
        let want: Vec<&str> = words
            .iter()
            .enumerate()
            .filter(|(at, _)| Some(*at) != first)
            .map(|(_, word)| *word)
            .collect();
        let got = forwarded(argv(&words));
        assert_eq!(got, argv(&want), "round {round}: {words:?}");
        assert_eq!(got.len() + usize::from(first.is_some()), words.len(), "round {round}");
        dropped += u32::from(first.is_some());
    }
    assert!(dropped > 1_000, "the draws rarely held the flag: {dropped} of 4000");
    Ok(())
}
