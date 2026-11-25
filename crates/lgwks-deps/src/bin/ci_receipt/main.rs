//! `ci-receipt`: failed GitHub Actions job logs in, one compact and deterministic
//! JSON failure receipt out, so an agent reproduces a CI failure locally without
//! reading a raw log.
//!
//! ```text
//! ci-receipt --logs DIR --out FILE [--markdown FILE] [--max N]
//! ```
//!
//! `DIR` holds one `<job name>.log` per failed job, each line in the raw Actions
//! API form or the `gh run view --log` form. Each log yields failures of three
//! kinds, in this order of preference: `test` (a nextest result line and its
//! output), `compile` (a rustc or clippy diagnostic with a place) and `step` (the
//! failed step, only when nothing more specific was found), each with the command
//! that runs it again. A log that names none is one `step` row saying so, as
//! every log read is a failed job's. The receipt keeps the first `--max` (default 50, at most
//! 100) in the order job, file, line, test, and sets `truncated` when more were
//! found. Exit status 0 when the receipt is written, even with no failure found;
//! 2 on bad arguments, an unreadable directory or log, or a file it cannot write.
//!
//! Bounds (each reached by a test): 512 logs a directory, 32 MiB read from the end
//! of a log, 4,096 characters a line, 2,048 failures a log kind, 400 characters a
//! message, 100 failures a receipt. Past a log bound the receipt says `truncated`.
//!
//! Signals, through `lgwks_std::trace`: `ci-receipt refused` (error, one per
//! refused run, the typed error as a field), `a failure was dropped past the
//! per-log bound` (warn, one per drop, the `reader` named; any is a receipt that
//! is missing a failure), `wrote a failure receipt` (info, the
//! failures kept and `truncated`).

mod compile;
mod error;
mod extract;
mod failure;
mod line;
mod logs;
mod nextest;
mod render;
mod steps;

#[cfg(test)]
#[path = "../shared/fixture.rs"]
mod fixture;

#[cfg(test)]
#[path = "tests.rs"]
mod ci_receipt_tests;

#[cfg(test)]
mod sim_ci_receipt;

#[cfg(test)]
mod sim_ci_receipt_out;

#[cfg(test)]
mod sim_draw;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use error::ReceiptError;
use failure::{DEFAULT_MAX, Failure, Kind, MAX_CEILING, Receipt};

/// What the command line asked for.
#[derive(Debug)]
struct Args {
    /// The directory of logs.
    logs: PathBuf,
    /// The receipt file.
    out: PathBuf,
    /// The markdown file, when asked for.
    markdown: Option<PathBuf>,
    /// The most failures to keep.
    max: usize,
}

/// What a finished run wrote.
#[derive(Debug)]
struct Written {
    /// The failures kept.
    failures: usize,
    /// Whether the logs held more.
    truncated: bool,
    /// The receipt file.
    out: PathBuf,
}

/// Takes the value after `flag` into `slot`: refused when the flag came before,
/// when nothing follows, or when what follows is another flag.
fn take<'argument>(
    slot: &mut Option<OsString>,
    flag: &'static str,
    remaining: &mut impl Iterator<Item = &'argument OsString>,
) -> Result<(), ReceiptError> {
    if slot.is_some() {
        lgwks_std::trace::debug!(flag, "a flag was given twice");
        return Err(ReceiptError::DuplicateFlag(flag));
    }
    match remaining.next() {
        Some(value) if !value.to_str().is_some_and(|text| text.starts_with("--")) => {
            *slot = Some(value.clone());
            Ok(())
        }
        Some(_) | None => Err(ReceiptError::MissingValue(flag)),
    }
}

/// `value` as a `--max`: a whole number from 1 to the ceiling.
fn parse_max(value: &OsStr) -> Result<usize, ReceiptError> {
    let text = value.to_string_lossy();
    let refused = || ReceiptError::BadMax(text.clone().into_owned());
    let found = text.parse::<usize>().map_err(|error| {
        lgwks_std::trace::debug!(%error, "--max is not a number");
        refused()
    })?;
    if (1..=MAX_CEILING).contains(&found) {
        Ok(found)
    } else {
        Err(refused())
    }
}

/// Reads the command line.
fn parse_args(arguments: &[OsString]) -> Result<Args, ReceiptError> {
    let (mut logs, mut out, mut markdown, mut max) = (None, None, None, None);
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        let (slot, flag) = match argument.to_str() {
            Some("--logs") => (&mut logs, "--logs"),
            Some("--out") => (&mut out, "--out"),
            Some("--markdown") => (&mut markdown, "--markdown"),
            Some("--max") => (&mut max, "--max"),
            Some(_) | None => {
                let word = argument.to_string_lossy().into_owned();
                lgwks_std::trace::debug!(%word, "an argument is no flag ci-receipt takes");
                return Err(ReceiptError::UnknownArgument(word));
            }
        };
        take(slot, flag, &mut remaining)?;
    }
    let logs = logs.ok_or(ReceiptError::MissingFlag("--logs"))?;
    let out = out.ok_or(ReceiptError::MissingFlag("--out"))?;
    let max = match max {
        Some(value) => parse_max(&value)?,
        None => DEFAULT_MAX,
    };
    Ok(Args {
        logs: PathBuf::from(logs),
        out: PathBuf::from(out),
        markdown: markdown.map(PathBuf::from),
        max,
    })
}

/// The temporary names one receipt path may take: `.<name>.<n>.tmp`, `n` below
/// this.
const TEMP_NAMES: usize = 64;

/// The first temporary beside `path` that no writer holds, created exclusively.
///
/// # Errors
///
/// The create's own error, or `AlreadyExists` when all [`TEMP_NAMES`] are held.
fn claim_temp(path: &Path, name: &str) -> io::Result<(PathBuf, fs::File)> {
    for number in 0..TEMP_NAMES {
        let temp = path.with_file_name(format!(".{name}.{number}.tmp"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                lgwks_std::trace::debug!(%error, temp = %temp.display(), "a temporary receipt could not be created");
                return Err(error);
            }
        }
    }
    lgwks_std::trace::debug!(path = %path.display(), "every temporary name is held");
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "every temporary name beside the receipt is held",
    ))
}

/// Writes `text` to `path` through a file beside it and a rename, so a reader
/// sees the old file or the whole new one, never half of it.
fn write_atomic(path: &Path, text: &str) -> Result<(), ReceiptError> {
    let failed = |source: io::Error| ReceiptError::Write {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(failed)?;
    }
    let Some(name) = path.file_name() else {
        lgwks_std::trace::debug!(path = %path.display(), "an output path names no file");
        return Err(failed(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the path names no file",
        )));
    };
    // An exclusive create, not a process id or a random name: the OS reuses a
    // pid, and `lgwks_std::id` does not build for wasm (run 45522616196). A
    // name another writer holds is skipped and never opened.
    let (temp, mut file) = claim_temp(path, &name.to_string_lossy()).map_err(failed)?;
    let wrote = file.write_all(text.as_bytes());
    drop(file);
    if let Err(source) = wrote {
        lgwks_std::trace::debug!(%source, temp = %temp.display(), "a temporary receipt could not be written");
        if let Err(error) = fs::remove_file(&temp) {
            lgwks_std::trace::debug!(%error, "a temporary receipt could not be removed");
        }
        return Err(failed(source));
    }
    let Err(source) = fs::rename(&temp, path) else {
        return Ok(());
    };
    lgwks_std::trace::debug!(%source, path = %path.display(), "a receipt could not be moved into place");
    if let Err(error) = fs::remove_file(&temp) {
        lgwks_std::trace::debug!(%error, "a temporary receipt could not be removed");
    }
    Err(failed(source))
}

/// The row of a failed job whose log names no failure: only failed jobs' logs are
/// read, so a log with nothing in it is a job cancelled at its time limit or
/// killed before it printed one (run 45467652939, `bot-it-surface, slice:3/4`).
fn quiet(job: &str) -> Failure {
    Failure {
        job: job.to_owned(),
        kind: Kind::Step,
        test: None,
        binary: None,
        file: None,
        line: None,
        code: None,
        seed: None,
        message: QUIET.to_owned(),
        repro: String::new(),
        step: String::new(),
    }
}

/// What a quiet job's row says.
const QUIET: &str =
    "the job failed and no line of the log names the failure: cancelled at its time limit?";

/// Runs with `arguments`: lists the logs, reads them one at a time into the
/// receipt, and writes the receipt and the table.
fn execute(arguments: &[OsString]) -> Result<Written, ReceiptError> {
    let args = parse_args(arguments)?;
    let files = logs::list(&args.logs)?;
    let mut receipt = Receipt::new(args.max);
    for file in &files {
        let log = logs::read(&file.path)?;
        let found = extract::extract(&file.job, &log.text);
        receipt.shed = receipt.shed || log.clipped || found.shed;
        let mut failures = found.failures;
        if failures.is_empty() {
            failures.push(quiet(&file.job));
        }
        receipt.absorb(failures);
    }
    write_atomic(&args.out, &render::json(&receipt)?)?;
    if let Some(path) = args.markdown.as_deref() {
        write_atomic(path, &render::markdown(&receipt, args.max))?;
    }
    lgwks_std::trace::info!(
        failures = receipt.failures.len(),
        truncated = receipt.truncated(),
        "wrote a failure receipt"
    );
    Ok(Written {
        failures: receipt.failures.len(),
        truncated: receipt.truncated(),
        out: args.out,
    })
}

/// Writes a line, logging a failure to write.
fn say(stream: &mut dyn Write, text: &str) {
    if let Err(error) = writeln!(stream, "{text}") {
        lgwks_std::trace::debug!(%error, "a line could not be written");
    }
}

fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    match execute(&arguments) {
        Ok(written) => {
            let cut = if written.truncated {
                " (truncated)"
            } else {
                ""
            };
            say(
                &mut io::stdout(),
                &format!(
                    "ci-receipt: {} failures in {}{cut}",
                    written.failures,
                    written.out.display()
                ),
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            lgwks_std::trace::error!(%error, "ci-receipt refused");
            say(&mut io::stderr(), &format!("ci-receipt: {error}"));
            ExitCode::from(2)
        }
    }
}
