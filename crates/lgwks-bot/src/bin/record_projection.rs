//! `record-projection`: a consumer's view of its own journal, rebuilt after any kill.
//!
//! The record is one number: the high half names a tenant and the low half is a
//! value. Each line of standard input is one command and each answer is one JSON
//! line, so a person or an agent can fill a journal, kill the process, and see the
//! view of what survived come back with the same digest.

use std::fmt;
use std::io::{BufRead as _, Write as _};
use std::path::Path;
use std::process::ExitCode;

use lgwks_bot::journal::{JournalError, RecordEntry, RecordJournal};
use lgwks_bot::{ProjectionError, RecordFold, RecordProjection};

/// The usage text, for a first-timer and for a misuse.
const USAGE: &str = "usage: record-projection <journal> <max-rows>
reads one command per line and prints one JSON line for each:
  fill <tenant> <count>   append count values for a tenant, durably
  poison <tenant>         append a record the fold refuses
  catch-up                fold what the journal gained
  show                    print the rows and their digest
  rebuild                 discard the view and fold the journal again";

/// A low half the fold refuses, for trying a poisoned view from the outside.
const POISON: u64 = 0xFFFF_FFFF;

/// A record the tally refuses.
#[derive(Debug)]
struct Refused(u64);

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "record {} carries the refused value", self.0)
    }
}

/// One tenant's row: how many records and their sum.
#[derive(Default, Debug)]
struct Row {
    /// Records folded for this tenant.
    count: u64,
    /// Their values added up.
    sum: u64,
}

/// The fold: each tenant's row, plus the records that named no tenant.
#[derive(Default)]
struct Tally {
    /// Records with no tenant, which are read and belong to no row.
    unrouted: u64,
}

impl RecordFold<u64> for Tally {
    type Key = u64;
    type Row = Row;
    type Error = Refused;

    fn route(&self, record: &u64) -> Option<u64> {
        let tenant = *record >> 32;
        (tenant != 0).then_some(tenant)
    }

    fn apply(&mut self, entry: &RecordEntry<u64>, row: Option<&mut Row>) -> Result<(), Refused> {
        let value = *entry.record() & POISON;
        match row {
            _ if value == POISON => Err(Refused(*entry.record())),
            Some(held) => {
                held.count = held.count.saturating_add(1);
                held.sum = held.sum.saturating_add(value);
                Ok(())
            }
            None => {
                self.unrouted = self.unrouted.saturating_add(1);
                Ok(())
            }
        }
    }
}

/// Why this tool stopped rather than answering a command.
enum Stopped {
    /// The journal refused a write or an open.
    Journal(JournalError),
    /// Standard input or output failed.
    Pipe(std::io::Error),
}

/// The view, or why the last rebuild left none.
type View = Result<RecordProjection<u64, Tally>, String>;

/// A journal and its view.
struct Session {
    /// The journal, the truth.
    journal: RecordJournal<u64>,
    /// The view of it.
    view: View,
    /// The rows a rebuilt view may hold.
    max_rows: usize,
}

/// A refusal as one JSON line.
fn refused<E: fmt::Display>(cause: &ProjectionError<E>) -> String {
    let kind = match *cause {
        ProjectionError::Fold(_) => "fold",
        ProjectionError::Journal(_) => "journal",
        ProjectionError::Diverged { .. } => "diverged",
        ProjectionError::RowLimit { .. } => "row_limit",
        ProjectionError::Poisoned => "poisoned",
        _ => "refused",
    };
    format!(
        "{{\"ok\":false,\"refused\":\"{kind}\",\"why\":{:?}}}",
        cause.to_string()
    )
}

/// The answer when a refused rebuild left no view.
fn no_view(why: &str) -> String {
    format!("{{\"ok\":false,\"refused\":\"no_view\",\"why\":{why:?}}}")
}

/// A sequence as JSON, where nothing applied yet is null.
fn applied_of(view: &RecordProjection<u64, Tally>) -> String {
    match view.applied() {
        Some(last) => last.to_string(),
        None => "null".to_owned(),
    }
}

/// A word of a command, as a number.
fn number(word: Option<&str>) -> Option<u64> {
    word.and_then(|text| text.parse().ok())
}

impl Session {
    /// Fold the whole journal into a new view.
    fn rebuild(&mut self) -> String {
        match RecordProjection::rebuild(&self.journal, Tally::default(), self.max_rows) {
            Ok(view) => {
                let line = format!("{{\"ok\":true,\"applied\":{}}}", applied_of(&view));
                self.view = Ok(view);
                line
            }
            Err(cause) => {
                let line = refused(&cause);
                self.view = Err(cause.to_string());
                line
            }
        }
    }

    /// The rows and their digest.
    fn show(&self) -> String {
        let view: &RecordProjection<u64, Tally> = match self.view {
            Ok(ref view) => view,
            Err(ref why) => return no_view(why),
        };
        let rendered: Vec<String> = view
            .rows()
            .map(|(tenant, row)| format!("{tenant}:{}:{}", row.count, row.sum))
            .collect();
        let text = format!("{}|{}", rendered.join(","), view.fold().unrouted);
        let first = view.row(&1).is_some();
        format!(
            "{{\"ok\":true,\"rows\":{},\"tenant_one\":{first},\"applied\":{},\"digest\":\"{}\"}}",
            rendered.len(),
            applied_of(view),
            lgwks_std::hash::blake3(text.as_bytes()).to_hex()
        )
    }

    /// Fold what the journal gained.
    fn catch_up(&mut self) -> String {
        let view: &mut RecordProjection<u64, Tally> = match self.view {
            Ok(ref mut view) => view,
            Err(ref why) => return no_view(why),
        };
        match view.catch_up(&self.journal) {
            Ok(folded) => format!("{{\"ok\":true,\"folded\":{folded}}}"),
            Err(cause) => refused(&cause),
        }
    }

    /// Answer one command.
    fn apply(&mut self, line: &str) -> Result<String, JournalError> {
        let mut words = line.split_whitespace();
        let command = words.next();
        let (first, second) = (number(words.next()), number(words.next()));
        Ok(match (command, first, second) {
            (Some("fill"), Some(tenant), Some(count)) => {
                let batch: Vec<u64> = (0..count)
                    .map(|value| (tenant << 32) | (value & 0xFFFF))
                    .collect();
                self.journal
                    .compare_and_append_all(self.journal.tail(), &batch)?;
                format!("{{\"ok\":true,\"filled\":{count}}}")
            }
            (Some("poison"), Some(tenant), None) => {
                let record = (tenant << 32) | POISON;
                self.journal
                    .compare_and_append(self.journal.tail(), &record)?;
                "{\"ok\":true,\"poisoned\":true}".to_owned()
            }
            (Some("catch-up"), None, None) => self.catch_up(),
            (Some("show"), None, None) => self.show(),
            (Some("rebuild"), None, None) => self.rebuild(),
            _ => format!("{{\"ok\":false,\"refused\":\"usage\",\"why\":{line:?}}}"),
        })
    }
}

/// Run every line of standard input against one journal.
fn run(journal: &Path, max_rows: usize) -> Result<(), Stopped> {
    let opened = RecordJournal::open(journal).map_err(Stopped::Journal)?;
    let view = RecordProjection::rebuild(&opened, Tally::default(), max_rows)
        .map_err(|cause| cause.to_string());
    let mut session = Session {
        journal: opened,
        view,
        max_rows,
    };
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let command = line.map_err(Stopped::Pipe)?;
        let reply = session.apply(&command).map_err(Stopped::Journal)?;
        writeln!(out, "{reply}")
            .and_then(|()| out.flush())
            .map_err(Stopped::Pipe)?;
    }
    Ok(())
}

/// Say why this process is ending, and record a closed pipe rather than panic on it.
fn say(why: &str) {
    let mut handle = std::io::stderr().lock();
    if let Err(cause) = writeln!(handle, "{why}") {
        lgwks_std::trace::debug!(error = ?cause, "standard error is closed");
    }
}

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let journal = arguments.next();
    let rows = arguments.next();
    let limit = rows
        .as_deref()
        .and_then(std::ffi::OsStr::to_str)
        .and_then(|text| text.parse::<usize>().ok());
    let (Some(journal), Some(max_rows)) = (journal, limit) else {
        say(USAGE);
        return ExitCode::from(64);
    };
    match run(Path::new(&journal), max_rows) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Stopped::Journal(cause)) => {
            say(&format!("the journal refused: {cause}"));
            ExitCode::from(2)
        }
        Err(Stopped::Pipe(cause)) => {
            say(&format!("input or output failed: {cause}"));
            ExitCode::FAILURE
        }
    }
}
