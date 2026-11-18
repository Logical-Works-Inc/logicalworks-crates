//! The argument vector of `parse_budget`: each flag, its value and its refusal.

use super::{DEFAULT_BYTES, DEFAULT_ROUNDS, Options};

/// Parse the argument vector, or report why it is wrong.
pub(super) fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        grammar: None,
        kind: None,
        bytes: DEFAULT_BYTES,
        shape_bytes: DEFAULT_BYTES,
        rounds: DEFAULT_ROUNDS,
        tiers: Vec::new(),
        tier_bytes: 64 * 1024,
        threads: 8,
        list: false,
    };
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let applied = apply_flag(&mut options, flag, &mut rest);
        applied?;
    }
    validated(options)
}

/// Apply one flag, taking its value from `rest` when it has one.
///
/// Every arm answers a `Result` rather than returning early, so the one caller
/// propagates exactly one refusal per flag and a flag with no value, an
/// unparseable value and an unknown flag are three distinct messages.
fn apply_flag<'arg>(
    options: &mut Options,
    flag: &str,
    rest: &mut impl Iterator<Item = &'arg String>,
) -> Result<(), String> {
    let mut value = || rest.next().ok_or(format!("{flag} needs a value"));
    match flag {
        "--grammar" => value().map(|raw| options.grammar = Some(raw.clone())),
        "--shape" => value().map(|raw| options.kind = Some(raw.clone())),
        "--bytes" => value()
            .and_then(|raw| number(flag, raw))
            .map(|bytes| options.bytes = bytes),
        "--shape-bytes" => value()
            .and_then(|raw| number(flag, raw))
            .map(|bytes| options.shape_bytes = bytes),
        "--rounds" => value()
            .and_then(|raw| number::<usize>(flag, raw))
            .map(|rounds| options.rounds = rounds.max(1)),
        "--tier" => value()
            .and_then(|raw| number(flag, raw))
            .map(|level| options.tiers.push(level)),
        "--tier-bytes" => value()
            .and_then(|raw| number(flag, raw))
            .map(|bytes| options.tier_bytes = bytes),
        "--threads" => value()
            .and_then(|raw| number::<usize>(flag, raw))
            .map(|threads| options.threads = threads.max(1)),
        "--list-grammars" => {
            options.list = true;
            Ok(())
        }
        other => Err(format!("unknown argument `{other}`")),
    }
}

/// Parse one flag's numeric value, naming the flag in the refusal.
fn number<T>(flag: &str, raw: &str) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    raw.parse().map_err(|error| format!("{flag}: {error}"))
}

/// Refuse a byte budget of zero, and a concurrency level of zero.
///
/// A zero-byte source measures nothing and a tier of zero parses admits no
/// worker, so both would print a row of throughput over no input; the tier row's
/// percentiles are then percentiles of nothing.
fn validated(options: Options) -> Result<Options, String> {
    if options.bytes == 0 {
        Err("--bytes must be at least one".to_owned())
    } else if options.shape_bytes == 0 {
        Err("--shape-bytes must be at least one".to_owned())
    } else if options.tiers.contains(&0) {
        Err("--tier must be at least one".to_owned())
    } else {
        Ok(options)
    }
}
