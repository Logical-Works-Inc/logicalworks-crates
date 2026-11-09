//! `recorded-instant`: a recorded clock, driven line by line from standard input.
//!
//! What a consumer's process does with a recorded timestamp: read the clock, write the
//! reading to a journal, die, and come back never reading before what it wrote. The
//! journal argument is a record journal of instants, so a restart crosses a real
//! process boundary. With a seed the clock reads no real time, so two runs of one
//! script print the same bytes on any host.

use std::fmt::Display;
use std::io::{BufRead as _, Write as _};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use lgwks_bot::kernel::clock::{ClockError, EpochClock, RecordedInstant};
use lgwks_bot::kernel::journal::{JournalError, RecordEntry, RecordJournal, RecordReplay};

/// The usage text, for a first-timer and for a misuse.
const USAGE: &str = "usage: recorded-instant <journal> [seed]
with no seed the clock is live and never reads before the journal's last record;
with a seed it reads no real clock and only an advance moves it.
reads one command per line and prints one JSON line for each:
  now              print this moment
  record           write this moment to the journal, durably
  last             print the journal's last record
  since <nanos>    how long after that Unix-epoch nanosecond this moment is
  advance <nanos>  move a seeded or restored clock forward
  restore          stand a caller-advanced clock at the last record
  elapsed          how far this clock's own timeline has run";

/// Why this tool stopped rather than answering a command.
enum CliError {
    /// A command could not be read.
    Usage(String),
    /// The system clock could not anchor a live clock.
    Clock(ClockError),
    /// The journal refused.
    Journal(JournalError),
    /// Standard input or output failed.
    Io(std::io::Error),
}

/// The clock, the journal it records to, and the last record the journal holds.
struct Session {
    /// The clock under test.
    clock: EpochClock,
    /// The instants on disk.
    journal: RecordJournal<RecordedInstant>,
    /// The record a restart may not read before, once there is one.
    last: Option<RecordedInstant>,
}

/// One number from a command's arguments.
fn nanos(word: Option<&str>) -> Result<u64, CliError> {
    match word.map(str::parse) {
        Some(Ok(count)) => Ok(count),
        Some(Err(cause)) => {
            let cause: std::num::ParseIntError = cause;
            let refusal = CliError::Usage(format!("{word:?} is not a number: {cause}"));
            Err(refusal)
        }
        None => Err(CliError::Usage("that command takes a number".to_owned())),
    }
}

/// The last record in `journal`, read by replaying it.
fn last_recorded(
    journal: &RecordJournal<RecordedInstant>,
) -> Result<Option<RecordedInstant>, JournalError> {
    let replay: RecordReplay<RecordedInstant> = journal.replay()?;
    let mut last = None;
    for entry in replay {
        let entry: RecordEntry<RecordedInstant> = entry?;
        last = Some(*entry.record());
    }
    Ok(last)
}

/// The clock this process starts on.
fn opened(seed: Option<u64>, last: Option<RecordedInstant>) -> Result<EpochClock, ClockError> {
    match (seed, last) {
        (Some(drawn), _) => Ok(EpochClock::seeded(drawn)),
        (None, Some(floor)) => EpochClock::system_after(floor),
        (None, None) => EpochClock::system(),
    }
}

/// The stable name of a clock refusal, for a caller that branches on it.
const fn kind(refusal: &ClockError) -> &'static str {
    match *refusal {
        ClockError::NotVirtual => "not_virtual",
        ClockError::OutOfRange { .. } => "out_of_range",
        ClockError::BeforeEpoch => "before_epoch",
        _ => "refused",
    }
}

/// The fields one instant answers with.
fn fields(instant: RecordedInstant) -> String {
    format!("\"nanos\":{},\"text\":\"{instant}\"", instant.unix_nanos())
}

/// A refusal this tool chose, as one line.
fn refused(name: &str, why: impl Display) -> String {
    format!(
        "{{\"ok\":false,\"refused\":\"{name}\",\"why\":{:?}}}",
        why.to_string()
    )
}

impl Session {
    /// Write this moment to the journal.
    fn record(&mut self, at: RecordedInstant) -> Result<String, CliError> {
        let tail = self.journal.tail();
        let ack = self
            .journal
            .compare_and_append(tail, &at)
            .map_err(CliError::Journal)?;
        self.last = Some(at);
        Ok(format!(
            "{{\"ok\":true,{},\"sequence\":{}}}",
            fields(at),
            ack.position().sequence()
        ))
    }

    /// Answer one command.
    fn apply(&mut self, line: &str) -> Result<String, CliError> {
        let mut words = line.split_whitespace();
        let command = words.next();
        let first = words.next();
        let at: RecordedInstant = self.clock.now();
        Ok(match command {
            Some("now") => format!("{{\"ok\":true,{}}}", fields(at)),
            Some("record") => self.record(at)?,
            Some("last") => match self.last {
                Some(held) => format!("{{\"ok\":true,{}}}", fields(held)),
                None => "{\"ok\":true,\"nanos\":null}".to_owned(),
            },
            Some("since") => {
                let then: RecordedInstant = RecordedInstant::from_unix_nanos(nanos(first)?);
                let gap: Duration = at.saturating_duration_since(then);
                format!("{{\"ok\":true,\"since\":{}}}", gap.as_nanos())
            }
            Some("advance") => match self.clock.advance(Duration::from_nanos(nanos(first)?)) {
                Ok(landed) => format!("{{\"ok\":true,{}}}", fields(landed)),
                Err(refusal) => refused(kind(&refusal), refusal),
            },
            Some("restore") => match self.last {
                Some(held) => {
                    self.clock = EpochClock::restored(held);
                    format!("{{\"ok\":true,{}}}", fields(held))
                }
                None => refused("nothing_recorded", "the journal holds no record"),
            },
            Some("elapsed") => {
                let ran: Duration = self.clock.clock().now();
                format!("{{\"ok\":true,\"elapsed\":{}}}", ran.as_nanos())
            }
            Some(other) => refused("usage", format!("no command named {other:?}")),
            None => refused("usage", "an empty line names no command"),
        })
    }
}

/// Run every line of standard input against one clock and one journal.
fn run(journal: &Path, seed: Option<u64>) -> Result<(), CliError> {
    let journal: RecordJournal<RecordedInstant> =
        RecordJournal::open(journal).map_err(CliError::Journal)?;
    let last = last_recorded(&journal).map_err(CliError::Journal)?;
    let mut session = Session {
        clock: opened(seed, last).map_err(CliError::Clock)?,
        journal,
        last,
    };
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(CliError::Io)?;
        let reply = match session.apply(&line) {
            Err(CliError::Usage(why)) => refused("usage", why),
            answered => answered?,
        };
        writeln!(out, "{reply}").map_err(CliError::Io)?;
        out.flush().map_err(CliError::Io)?;
    }
    Ok(())
}

/// The exit status a stopped run ends on, after saying why to standard error.
fn ended(stopped: &CliError) -> ExitCode {
    let (status, why) = match *stopped {
        CliError::Usage(ref why) => (64, format!("{why}\n{USAGE}")),
        CliError::Clock(ref cause) => (2, format!("the clock refused: {cause}")),
        CliError::Journal(ref cause) => (2, format!("the journal refused: {cause}")),
        CliError::Io(ref cause) => (1, format!("input or output failed: {cause}")),
    };
    let mut errors = std::io::stderr().lock();
    if let Err(closed) = writeln!(errors, "{why}") {
        lgwks_std::trace::debug!(%closed, %why, "recorded-instant: nowhere to say why");
    }
    ExitCode::from(status)
}

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let (Some(journal), seed, None) = (arguments.next(), arguments.next(), arguments.next()) else {
        return ended(&CliError::Usage("name one journal".to_owned()));
    };
    let seed = match seed.map(|word| nanos(word.to_str())) {
        None => None,
        Some(Ok(drawn)) => Some(drawn),
        Some(Err(refusal)) => return ended(&refusal),
    };
    match run(Path::new(&journal), seed) {
        Ok(()) => ExitCode::SUCCESS,
        Err(stopped) => ended(&stopped),
    }
}
