//! The command line: every flag, every bound, one typed refusal each.
//!
//! The hostile reader is whoever types the command: a flag that is not here, a
//! number that is not a number, a number past a bound and a flag with no value
//! are each refused with their own variant before any work starts.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::error::BenchError;

/// The name the command line is reported under.
pub(crate) const PROGRAM: &str = "lgwks-bench-durable";

/// Runs when `--runs` is not given.
pub(crate) const DEFAULT_RUNS: u32 = 1_000;

/// The most runs one measurement takes: the latency samples, the fault plan and
/// the run store all grow with this, so a larger ask is refused, not clamped.
pub(crate) const MAX_RUNS: u32 = 100_000;

/// The most runs one recovery round parks: the host holds at most 10 000
/// undecided approvals per tenant, and the two tenants split the runs evenly.
pub(crate) const MAX_RECOVER_RUNS: u32 = 10_000;

/// Recovery rounds when `--recover-rounds` is not given.
pub(crate) const DEFAULT_ROUNDS: u32 = 5;

/// The most recovery rounds: each is a child process and a reopened store.
pub(crate) const MAX_ROUNDS: u32 = 100;

/// The seed when `--seed` is not given.
pub(crate) const DEFAULT_SEED: u64 = 1;

/// What the binary was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Time the flow: throughput, latency, retries, peak RSS.
    Flow,
    /// Kill a child mid-flight and time the recovery.
    Recover,
    /// The child half of [`Mode::Recover`]: park runs, say so, wait to be killed.
    ParkChild,
}

/// An engine the report can carry a row for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    /// The `lgwks_bot` durable workflow engine.
    Lgwks,
    /// Restate, the rival.
    Restate,
}

impl Row {
    /// The name `--rows` spells it with.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Lgwks => "lgwks",
            Self::Restate => "restate",
        }
    }
}

/// The server name a `--restate-server` that is not given means: found on `PATH`.
pub(crate) const DEFAULT_RESTATE_SERVER: &str = "restate-server";

/// The most rows one command measures: each engine once.
const MAX_ROWS: usize = 2;

/// The row `--rows` spells `name`, or `None` for a name no row has.
fn row_named(name: &str) -> Option<Row> {
    match name {
        "lgwks" => Some(Row::Lgwks),
        "restate" => Some(Row::Restate),
        _ => None,
    }
}

/// The rows a `--rows` value names, each at most once, in the order typed.
fn rows_of(text: &str) -> Result<Vec<Row>, BenchError> {
    let mut rows = Vec::new();
    for name in text.split(',') {
        let fresh = row_named(name).filter(|row| !rows.contains(row) && rows.len() < MAX_ROWS);
        let Some(row) = fresh else {
            lgwks_std::trace::debug!(value = text, "--rows is not a list of known, distinct rows");
            return Err(BenchError::UnknownArgument {
                argument: format!("--rows {text}"),
            });
        };
        rows.push(row);
    }
    Ok(rows)
}

/// The parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Args {
    /// How many runs.
    pub(crate) runs: u32,
    /// The directory to work under, when given.
    pub(crate) work_dir: Option<PathBuf>,
    /// The seed every draw comes from.
    pub(crate) seed: u64,
    /// One JSON document on stdout instead of text.
    pub(crate) json: bool,
    /// What to do.
    pub(crate) mode: Mode,
    /// Recovery rounds.
    pub(crate) rounds: u32,
    /// The engines to put in one table; empty is the lgwks row alone, as a
    /// report of its own.
    pub(crate) rows: Vec<Row>,
    /// The Restate server: a name found on `PATH`, or a path.
    pub(crate) restate_server: String,
    /// The table as Markdown instead of text.
    pub(crate) markdown: bool,
    /// The command line, as the report prints it.
    pub(crate) command: String,
}

/// The value a flag takes, or the refusal that it has none.
fn value<'a>(flag: &'static str, next: Option<&'a String>) -> Result<&'a str, BenchError> {
    next.map(String::as_str)
        .ok_or(BenchError::MissingValue { flag })
}

/// The whole number a flag takes.
fn number(flag: &'static str, next: Option<&String>) -> Result<u64, BenchError> {
    let text = value(flag, next)?;
    // A leading plus is a second spelling of the same number: refuse it with the
    // parser's own invalid-digit error, so one number has one spelling.
    let parsed = if text.starts_with('+') {
        "+".parse::<u64>()
    } else {
        text.parse::<u64>()
    };
    parsed.map_err(|source| BenchError::MalformedArgument {
        flag,
        value: text.to_owned(),
        source,
    })
}

/// `found` as a `u32` inside `min..=max`, or the refusal naming the bounds.
fn within(flag: &'static str, found: u64, min: u32, max: u32) -> Result<u32, BenchError> {
    let refusal = || BenchError::OutOfRange {
        flag,
        value: found,
        min: u64::from(min),
        max: u64::from(max),
    };
    if !(u64::from(min)..=u64::from(max)).contains(&found) {
        lgwks_std::trace::debug!(
            flag,
            found,
            min,
            max,
            "a flag's number is outside its bounds"
        );
        return Err(refusal());
    }
    u32::try_from(found).map_err(|source| {
        lgwks_std::trace::debug!(flag, found, %source, "a flag's number does not fit a u32");
        refusal()
    })
}

/// Parse the arguments after the program name.
///
/// # Errors
///
/// [`BenchError::NotUnicode`], [`BenchError::UnknownArgument`],
/// [`BenchError::MissingValue`], [`BenchError::MalformedArgument`] and
/// [`BenchError::OutOfRange`], each naming the flag; and
/// [`BenchError::MissingValue`] for a recovery child given no `--work-dir`.
pub(crate) fn parse(items: impl IntoIterator<Item = OsString>) -> Result<Args, BenchError> {
    let mut words = Vec::new();
    for item in items {
        let word = item.into_string().map_err(|rejected| {
            lgwks_std::trace::debug!(argument = ?rejected, "an argument is not valid UTF-8");
            BenchError::NotUnicode
        })?;
        words.push(word);
    }
    let command = std::iter::once(PROGRAM)
        .chain(words.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    let (mut runs, mut rounds) = (u64::from(DEFAULT_RUNS), u64::from(DEFAULT_ROUNDS));
    let (mut work_dir, mut seed) = (None, DEFAULT_SEED);
    let (mut json, mut recover, mut child, mut markdown) = (false, false, false, false);
    let (mut rows, mut restate_server) = (Vec::new(), DEFAULT_RESTATE_SERVER.to_owned());
    let mut cursor = words.iter();
    while let Some(word) = cursor.next() {
        match word.as_str() {
            "--runs" => {
                let found = number("--runs", cursor.next())?;
                runs = found;
            }
            "--recover-rounds" => {
                let found = number("--recover-rounds", cursor.next())?;
                rounds = found;
            }
            "--seed" => {
                let found = number("--seed", cursor.next())?;
                seed = found;
            }
            "--work-dir" => {
                let found = value("--work-dir", cursor.next())?;
                work_dir = Some(PathBuf::from(found));
            }
            "--rows" => rows = rows_of(value("--rows", cursor.next())?)?,
            "--restate-server" => {
                value("--restate-server", cursor.next())?.clone_into(&mut restate_server);
            }
            "--markdown" => markdown = true,
            "--json" => json = true,
            "--recover" => recover = true,
            "--park-child" => child = true,
            other => {
                lgwks_std::trace::debug!(argument = other, "no flag has this name");
                return Err(BenchError::UnknownArgument {
                    argument: other.to_owned(),
                });
            }
        }
    }
    let mode = if child {
        Mode::ParkChild
    } else if recover {
        Mode::Recover
    } else {
        Mode::Flow
    };
    if mode == Mode::ParkChild && work_dir.is_none() {
        lgwks_std::trace::debug!("the recovery child was given no --work-dir");
        return Err(BenchError::MissingValue { flag: "--work-dir" });
    }
    if (markdown && json) || (markdown && rows.is_empty()) || (!rows.is_empty() && child) {
        lgwks_std::trace::debug!("--markdown wants --rows and excludes --json; --rows excludes --park-child");
        return Err(BenchError::UnknownArgument {
            argument: "--markdown or --rows used with a flag it excludes".to_owned(),
        });
    }
    let most = if mode == Mode::Flow {
        MAX_RUNS
    } else {
        MAX_RECOVER_RUNS
    };
    let runs = within("--runs", runs, 1, most)?;
    let rounds = within("--recover-rounds", rounds, 1, MAX_ROUNDS)?;
    Ok(Args {
        runs,
        rounds,
        rows,
        restate_server,
        markdown,
        work_dir,
        seed,
        json,
        mode,
        command,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lgwks_std::seeded::Seeded;

    /// A test's result.
    type TestResult = Result<(), String>;

    /// The arguments as the OS would hand them.
    fn argv(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    /// The tokens a hostile typist draws from: real flags, junk, edge numbers.
    const VOCABULARY: [&str; 18] = [
        "--runs",
        "--seed",
        "--work-dir",
        "--json",
        "--recover",
        "--recover-rounds",
        "--park-child",
        "",
        "abc",
        "-1",
        "0",
        "1",
        "40",
        "100000",
        "100001",
        "18446744073709551616",
        "--runs=5",
        "\u{1F980}\n",
    ];

    /// The defaults are the ones the issue names.
    #[test]
    fn no_arguments_give_the_default_workload() -> TestResult {
        let args = parse(argv(&[])).map_err(|error| error.line())?;
        assert_eq!(args.runs, DEFAULT_RUNS);
        assert_eq!(args.seed, 1);
        assert_eq!(args.rounds, DEFAULT_ROUNDS);
        assert_eq!(args.mode, Mode::Flow);
        assert!(!args.json);
        Ok(())
    }

    /// A number that is not a number names its flag and keeps the parser's cause.
    #[test]
    fn a_malformed_number_is_a_typed_refusal() {
        let refused = parse(argv(&["--runs", "abc"]));
        assert!(
            matches!(
                refused,
                Err(BenchError::MalformedArgument { flag: "--runs", .. })
            ),
            "got {refused:?}"
        );
    }

    /// A value one past `u64` is malformed, not wrapped.
    #[test]
    fn a_number_past_u64_is_malformed_not_wrapped() {
        let refused = parse(argv(&["--seed", "18446744073709551616"]));
        assert!(matches!(
            refused,
            Err(BenchError::MalformedArgument { flag: "--seed", .. })
        ));
    }

    /// The two ends of `--runs` are refused, not clamped.
    #[test]
    fn runs_outside_its_bounds_is_refused() {
        for text in ["0", "100001"] {
            let refused = parse(argv(&["--runs", text]));
            assert!(
                matches!(refused, Err(BenchError::OutOfRange { flag: "--runs", .. })),
                "{text}: {refused:?}"
            );
        }
        assert!(parse(argv(&["--runs", "100000"])).is_ok());
    }

    /// Recovery parks approvals, which a tenant holds at most 10 000 of.
    #[test]
    fn recovery_runs_are_bounded_tighter_than_flow_runs() {
        let refused = parse(argv(&["--recover", "--runs", "10001"]));
        assert!(
            matches!(refused, Err(BenchError::OutOfRange { .. })),
            "{refused:?}"
        );
    }

    /// A flag that wants a value and is last has no value.
    #[test]
    fn a_trailing_flag_has_no_value() {
        let refused = parse(argv(&["--work-dir"]));
        assert!(matches!(
            refused,
            Err(BenchError::MissingValue { flag: "--work-dir" })
        ));
    }

    /// The child half cannot run without the directory the parent chose.
    #[test]
    fn the_park_child_needs_its_work_dir() {
        let refused = parse(argv(&["--park-child"]));
        assert!(matches!(
            refused,
            Err(BenchError::MissingValue { flag: "--work-dir" })
        ));
    }

    /// A flag spelled `--flag=value` is not a flag here.
    #[test]
    fn an_equals_spelling_is_an_unknown_argument() {
        let refused = parse(argv(&["--runs=5"]));
        assert!(matches!(refused, Err(BenchError::UnknownArgument { .. })));
    }

    /// The fool at the keyboard: thousands of seeded junk command lines never
    /// panic, and every one either parses inside its bounds or is refused with
    /// a usage-class line that is one line long.
    #[test]
    fn sim_hostile_command_lines_parse_in_bounds_or_refuse_typed() -> TestResult {
        for seed in 1_u64..=4 {
            let mut stream = Seeded::from_seed(seed);
            for _ in 0..2_500 {
                let length = stream.index(7).map_err(|error| error.to_string())?;
                let mut words = Vec::new();
                for _ in 0..length {
                    let at = stream
                        .index(VOCABULARY.len())
                        .map_err(|error| error.to_string())?;
                    words.push(*VOCABULARY.get(at).ok_or("the draw left the vocabulary")?);
                }
                match parse(argv(&words)) {
                    Ok(args) => {
                        let most = if args.mode == Mode::Flow {
                            MAX_RUNS
                        } else {
                            MAX_RECOVER_RUNS
                        };
                        assert!((1..=most).contains(&args.runs), "{words:?} -> {args:?}");
                        assert!((1..=MAX_ROUNDS).contains(&args.rounds), "{words:?}");
                        assert!(args.mode != Mode::ParkChild || args.work_dir.is_some());
                    }
                    Err(error) => {
                        assert_eq!(error.exit_code(), 2, "{words:?} -> {}", error.line());
                        assert!(!error.line().contains('\n'), "{words:?}");
                    }
                }
            }
        }
        Ok(())
    }
}
