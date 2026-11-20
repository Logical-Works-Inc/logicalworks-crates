//! Command-bar sims: what the suggestion list offers is exactly what the
//! parser accepts, whatever the capitalisation the user typed.

use drishti::native_host::command_bar::{CommandSuggestions, IntentParseError, IntentPipeline};

use crate::motion_kit::{describe, ensure};
use crate::rng::SimRng;

/// The four commands and the intent kind each one parses to.
const COMMANDS: [(&str, &str); 4] = [
    ("find", "intent.find"),
    ("pin", "intent.pin"),
    ("unpin", "intent.unpin"),
    ("help", "intent.help"),
];

/// A seeded capitalisation of `word`: each letter upper or lower case.
fn mixed_case(random: &mut SimRng, word: &str) -> String {
    word.chars()
        .map(|letter| {
            if random.below(2) == 0 {
                letter.to_ascii_uppercase()
            } else {
                letter
            }
        })
        .collect()
}

/// Bug caught: a suggestion list that completes `/FI` to `/find` while the
/// parser answers `/FIND query` with an unknown-command refusal, so the bar
/// offers a command it then rejects.
#[test]
fn sim_command_bar_parse_and_suggestions_agree_on_case() -> Result<(), String> {
    for seed in 0..300_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(400_000));
        for (name, kind) in COMMANDS {
            let typed = format!("/{}", mixed_case(&mut random, name));
            let offered = CommandSuggestions::suggestions(&typed);
            ensure(
                offered == [format!("/{name}")],
                &format!("seed {seed}: {typed} offered {offered:?}"),
            )?;
            let width = random.below_len(12);
            let tail = random.text(width);
            let input = format!("{typed} {tail}");
            let intent = IntentPipeline::parse(&input).map_err(describe)?;
            ensure(
                intent.kind() == kind && intent.text() == tail.trim_end(),
                &format!(
                    "seed {seed}: {input:?} parsed to ({}, {:?})",
                    intent.kind(),
                    intent.text()
                ),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a case-insensitive parser that widens the vocabulary: near
/// misses (an extra letter, a missing letter, a space after the slash) and
/// unknown words stay refused whatever their capitalisation, and every
/// suggestion of every typed prefix is accepted by the parser as offered.
#[test]
fn sim_command_bar_case_folding_does_not_widen_the_vocabulary() -> Result<(), String> {
    for seed in 0..300_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(410_000));
        let name = COMMANDS
            .get(random.below_len(COMMANDS.len()))
            .ok_or("commands must not be empty")?
            .0;
        let near = match random.below(4) {
            0 => format!("{name}x"),
            1 => name.chars().take(name.len().saturating_sub(1)).collect(),
            2 => format!(" {name}"),
            _ => String::from("exec"),
        };
        let typed = format!("/{}", mixed_case(&mut random, &near));
        ensure(
            IntentPipeline::parse(&typed) == Err(IntentParseError::UnknownCommand),
            &format!("seed {seed}: {typed:?} must stay an unknown command"),
        )?;
        let cut = random.below_len(name.len().saturating_add(1));
        let typed_prefix = name
            .get(..cut)
            .ok_or("the cut is not a character boundary")?;
        let prefix = format!("/{}", mixed_case(&mut random, typed_prefix));
        for offered in CommandSuggestions::suggestions(&prefix) {
            let shouted = mixed_case(&mut random, &offered);
            ensure(
                IntentPipeline::parse(&shouted).is_ok(),
                &format!("seed {seed}: {prefix:?} offered {offered:?} but {shouted:?} is refused"),
            )?;
        }
    }
    Ok(())
}
