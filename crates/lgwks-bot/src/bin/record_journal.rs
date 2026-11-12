//! `record-journal`: drive a durable record journal from a shell.
//!
//! What a consumer's process does to a [`RecordJournal`], as commands an operator or
//! a test can kill partway. Every acknowledgment is printed and flushed the moment
//! the append returns, so a line on the screen is a record the disk holds and a
//! crash test can check that promise against whatever the kill left. Output is one
//! JSON object a line. Refusals go to standard error with exit code 2.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use lgwks_bot::journal::{
    ContinuationPolicy, ContinuationWatermark, DurabilityPromise, Durable, DurableAck,
    JournalError, RecordEntry, RecordJournal, RecordReplay, SealPause,
};

/// What each subcommand takes, for a first-timer who typed none of it.
const USAGE: &str = "usage: record-journal <command> <path> [args]
  append <path> <count> <size>        one flush per record, each ack printed
  append-batch <path> <count> <size>  one flush for all of them
  await <path> <count> <size>         the awaited door, one flush per record
  verify <path>                       follow continuations, replay all, report
  page <path> <after> <limit>         entries after a sequence
  continue <path> <note> [boundary]   seal with a checkpoint, optionally stopping at
                                      after_seal_write, after_successor_write,
                                      after_successor_sync or after_directory_sync
  bench <path> <count> <size>         append latency p50, p95, p99 in microseconds";

/// Why a command did not finish.
#[derive(Debug)]
enum CliError {
    /// The command line asked for nothing this tool does.
    Usage(String),
    /// The journal refused.
    Journal(JournalError),
    /// The journal would refuse one record of a batch, so none of the batch was written.
    Refused {
        /// Where the record sits in the batch, from zero.
        index: u64,
        /// What the journal said about it.
        refusal: JournalError,
    },
    /// The terminal or pipe refused.
    Output(std::io::Error),
}

impl core::fmt::Display for CliError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::Usage(ref what) => write!(f, "{what}\n{USAGE}"),
            Self::Journal(ref refusal) => write!(f, "journal: {refusal}"),
            Self::Refused { index, ref refusal } => write!(
                f,
                "journal: record {index} of the batch would be refused, so none was written: {refusal}"
            ),
            Self::Output(ref cause) => write!(f, "output: {cause}"),
        }
    }
}

impl From<JournalError> for CliError {
    fn from(refusal: JournalError) -> Self {
        Self::Journal(refusal)
    }
}

impl From<std::io::Error> for CliError {
    fn from(cause: std::io::Error) -> Self {
        Self::Output(cause)
    }
}

/// Print one line and put it on the pipe before anything else happens.
fn say(line: &str) -> Result<(), CliError> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}")?;
    out.flush()?;
    Ok(())
}

/// One acknowledgment as the line a crash test reads back.
fn say_ack(ack: DurableAck) -> Result<(), CliError> {
    let at = ack.position();
    say(&format!(
        "{{\"ack\":{},\"head\":\"{}\",\"promise\":\"{}\"}}",
        at.sequence(),
        at.head(),
        ack.promise()
    ))
}

/// One entry, reporting its record's size.
fn say_entry(entry: &RecordEntry<Vec<u8>>) -> Result<(), CliError> {
    let at = entry.position();
    let record: &Vec<u8> = entry.record();
    let archived = record.to_record().map_err(JournalError::Encoding)?;
    say(&format!(
        "{{\"entry\":{},\"head\":\"{}\",\"len\":{},\"archived\":{},\"checkpoint\":{}}}",
        at.sequence(),
        at.head(),
        record.len(),
        archived.len(),
        entry.is_checkpoint()
    ))
}

/// The record a count and a size name: deterministic, so two runs write one history.
fn record_at(index: u64, size: usize) -> Vec<u8> {
    let fill = index
        .to_be_bytes()
        .iter()
        .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
    vec![fill; size]
}

/// The next argument, or the refusal naming what was missing.
fn required(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, CliError> {
    args.next()
        .ok_or_else(|| CliError::Usage(format!("missing <{name}>")))
}

/// The next argument as a number.
fn number<T: core::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
    name: &str,
) -> Result<T, CliError> {
    let text = required(args, name)?;
    match text.parse() {
        Ok(value) => Ok(value),
        Err(_refused) => Err(CliError::Usage(format!(
            "<{name}> is not a number: {text:?}"
        ))),
    }
}

/// The live journal at `path`, wherever its continuations have taken it.
fn active(path: &Path) -> Result<RecordJournal<Vec<u8>>, CliError> {
    let journal: RecordJournal<Vec<u8>> =
        RecordJournal::open_active(path, ContinuationPolicy::declared())?;
    Ok(journal)
}

/// `append`: every record acknowledged on its own flush.
fn append(path: &Path, count: u64, size: usize) -> Result<(), CliError> {
    let mut journal = active(path)?;
    for index in 0..count {
        let ack = journal.compare_and_append(journal.tail(), &record_at(index, size))?;
        say_ack(ack)?;
    }
    Ok(())
}

/// `append-batch`: every record on one flush, in a handle that stops at its ceilings.
///
/// Each record is asked about before the flush, as a group commit stages it, so a
/// refusal names the record the journal would not take and nothing is written.
fn append_batch(path: &Path, count: u64, size: usize) -> Result<(), CliError> {
    let mut journal: RecordJournal<Vec<u8>> = RecordJournal::open(path)?;
    let mut batch: Vec<Vec<u8>> = Vec::new();
    for index in 0..count {
        let record = record_at(index, size);
        if let Err(refusal) = journal.admits(&record) {
            let refused = Err(CliError::Refused { index, refusal });
            lgwks_std::trace::debug!(index, "record-journal: a staged record would be refused");
            return refused;
        }
        batch.push(record);
    }
    for ack in journal.compare_and_append_all(journal.tail(), &batch)? {
        say_ack(ack)?;
    }
    Ok(())
}

/// `await`: the door a task uses, driven to its answer.
fn await_appends(path: &Path, count: u64, size: usize) -> Result<(), CliError> {
    let mut journal: RecordJournal<Vec<u8>> =
        RecordJournal::open_continuing(path, ContinuationPolicy::declared())?;
    for index in 0..count {
        let tail = journal.tail();
        let record = record_at(index, size);
        let ack = lgwks_std::task::block_on(journal.compare_and_append_async(tail, &record))?;
        say_ack(ack)?;
    }
    Ok(())
}

/// `verify`: replay all of it and say what is there.
fn verify(path: &Path) -> Result<(), CliError> {
    let journal = active(path)?;
    let replay: RecordReplay<Vec<u8>> = journal.replay()?;
    let mut replayed = 0_u64;
    for entry in replay {
        entry?;
        replayed = replayed.saturating_add(1);
    }
    let mark: ContinuationWatermark = journal.watermark();
    let promise: DurabilityPromise = journal.durability();
    let tail = journal.tail();
    say(&format!(
        "{{\"committed\":{},\"replayed\":{replayed},\"first\":{},\"tail\":{},\"head\":\"{}\",\
         \"repaired\":{},\"promise\":\"{promise}\",\"due\":{}}}",
        journal.committed(),
        journal.first_sequence(),
        tail.sequence(),
        tail.head(),
        journal.torn_tail_repaired(),
        mark.is_due()
    ))
}

/// `page`: one window of a cursor's walk.
fn page(path: &Path, after: u64, limit: usize) -> Result<(), CliError> {
    let journal = active(path)?;
    for entry in journal.page(after, limit)? {
        say_entry(&entry)?;
    }
    Ok(())
}

/// `continue`: seal, optionally stopping at a named durable boundary.
fn continue_journal(path: &Path, note: &str, boundary: Option<String>) -> Result<(), CliError> {
    let mut journal = active(path)?;
    if let Some(name) = boundary {
        let found = SealPause::all()
            .into_iter()
            .find(|pause| pause.as_str() == name)
            .ok_or_else(|| CliError::Usage(format!("no boundary named {name:?}")))?;
        journal.arm_seal_pause(found);
    }
    let successor = journal
        .continue_with(&note.as_bytes().to_vec())?
        .ok_or_else(|| CliError::Usage("this journal does not continue".to_owned()))?;
    let tail = successor.tail();
    say(&format!(
        "{{\"sealed\":{},\"first\":{},\"head\":\"{}\"}}",
        tail.sequence(),
        successor.first_sequence(),
        tail.head()
    ))
}

/// The value at `percent` of a sorted sample, zero for no samples.
fn rank(sorted: &[u128], percent: usize) -> u128 {
    let last = sorted.len().saturating_sub(1);
    let index = last.saturating_mul(percent).saturating_div(100);
    match sorted.get(index) {
        Some(value) => *value,
        None => 0,
    }
}

/// `bench`: the whole append path, device included, on this machine.
fn bench(path: &Path, count: u64, size: usize) -> Result<(), CliError> {
    let mut journal = active(path)?;
    let mut micros = Vec::new();
    for index in 0..count {
        let record = record_at(index, size);
        let started = Instant::now();
        journal.compare_and_append(journal.tail(), &record)?;
        micros.push(started.elapsed().as_micros());
    }
    micros.sort_unstable();
    say(&format!(
        "{{\"samples\":{},\"p50_us\":{},\"p95_us\":{},\"p99_us\":{}}}",
        micros.len(),
        rank(&micros, 50),
        rank(&micros, 95),
        rank(&micros, 99)
    ))
}

/// Route a command line to its command.
fn run(mut args: impl Iterator<Item = String>) -> Result<(), CliError> {
    let command = required(&mut args, "command")?;
    let path = PathBuf::from(required(&mut args, "path")?);
    let (count, size) = match command.as_str() {
        "append" | "append-batch" | "await" | "bench" => {
            (number(&mut args, "count")?, number(&mut args, "size")?)
        }
        _ => (0, 0),
    };
    match command.as_str() {
        "append" => append(&path, count, size),
        "append-batch" => append_batch(&path, count, size),
        "await" => await_appends(&path, count, size),
        "bench" => bench(&path, count, size),
        "verify" => verify(&path),
        "page" => page(
            &path,
            number(&mut args, "after")?,
            number(&mut args, "limit")?,
        ),
        "continue" => {
            let note = required(&mut args, "note")?;
            continue_journal(&path, &note, args.next())
        }
        other => Err(CliError::Usage(format!("no command named {other:?}"))),
    }
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            lgwks_std::trace::debug!(error = ?failure, "record-journal: the command failed");
            let mut err = std::io::stderr().lock();
            if writeln!(err, "{failure}").is_err() {
                return ExitCode::FAILURE;
            }
            ExitCode::from(2)
        }
    }
}
