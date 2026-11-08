//! `durable-stop`: a named stop and a consumer's cursor, through a real kill.
//!
//! A host keeps its work in one record journal and its paging position in a tenant
//! store. Each line of standard input is one command and each answer is one JSON
//! line, so a person or an agent can ask for a stop, kill the process, and see that
//! the next process refuses to start a step and remembers how far it read.

use std::io::{BufRead as _, Error as IoError, Write as _};
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, MutexGuard};

use lgwks_bot::journal::RecordJournal;
use lgwks_bot::rt::sync::CancellationToken;
use lgwks_bot::script::{CancelError, CancelRecord, CancelStage, JournaledCancel, Tenant};
use lgwks_bot::tenant_store::{TenantStore, TenantStoreError};
use lgwks_std::wire::rkyv::{Archive, Deserialize, Serialize};

/// The usage text, for a first-timer and for a misuse.
const USAGE: &str = "usage: durable-stop <journal> <store-dir> <tenant> <consumer>
reads one command per line and prints one JSON line for each:
  work <n>        record the host's own work beside the stops
  request         record that the stop was asked for, and fire it
  begin           record that the work is winding down
  finish          record that the work stopped
  status          print the stage, and whether a step may start
  advance <n>     move this consumer's cursor forward
  cursor          print this consumer's cursor";

/// A record of the host's: its own work, or a stop moving.
#[derive(Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv)]
enum HostRecord {
    /// A piece of the host's work.
    Work(u64),
    /// A stop moved.
    Stop(String, CancelStage),
}

impl CancelRecord for HostRecord {
    fn cancel(name: &str, stage: CancelStage) -> Self {
        Self::Stop(name.to_owned(), stage)
    }

    fn as_cancel(&self) -> Option<(&str, CancelStage)> {
        match *self {
            Self::Stop(ref name, stage) => Some((name, stage)),
            Self::Work(_) => None,
        }
    }
}

/// What one process holds.
struct Session {
    /// The journal, shared with the stop.
    journal: Arc<Mutex<RecordJournal<HostRecord>>>,
    /// The stop this host answers to.
    stop: JournaledCancel<HostRecord>,
    /// The cursor's home.
    store: TenantStore,
    /// Whose work this is.
    tenant: Tenant,
    /// Whose cursor this is.
    consumer: String,
}

/// A failed step as one JSON line, naming the kind a caller branches on.
fn stopped(kind: &str, why: &dyn std::fmt::Display) -> String {
    format!(
        "{{\"ok\":false,\"refused\":\"{kind}\",\"why\":{:?}}}",
        why.to_string()
    )
}

/// A stage moved, or why not.
fn moved(result: Result<CancelStage, CancelError>) -> String {
    match result {
        Ok(stage) => format!("{{\"ok\":true,\"stage\":\"{stage:?}\"}}"),
        Err(ref cause @ CancelError::OutOfOrder { .. }) => stopped("out_of_order", cause),
        Err(ref cause) => stopped("journal", cause),
    }
}

/// The journal, whoever panicked holding it.
fn held(journal: &Mutex<RecordJournal<HostRecord>>) -> MutexGuard<'_, RecordJournal<HostRecord>> {
    match journal.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl Session {
    /// Print the stage and try to start a step.
    fn status(&self) -> String {
        let stop: &JournaledCancel<HostRecord> = &self.stop;
        let stage = match stop.stage() {
            Some(stage) => format!("\"{stage:?}\""),
            None => "null".to_owned(),
        };
        let may_start = stop.scope(self.tenant.clone()).enter("step").is_ok();
        let fired = stop.token().is_cancelled();
        format!("{{\"ok\":true,\"stage\":{stage},\"fired\":{fired},\"may_start\":{may_start}}}")
    }

    /// Move the cursor forward.
    fn advance(&self, position: u64) -> String {
        let store: &TenantStore = &self.store;
        match store.advance_cursor(self.tenant.as_str(), &self.consumer, position) {
            Ok(at) => format!("{{\"ok\":true,\"cursor\":{at}}}"),
            Err(ref cause @ TenantStoreError::CursorBehind { .. }) => stopped("behind", cause),
            Err(ref cause) => stopped("store", cause),
        }
    }

    /// Print the cursor.
    fn cursor(&self) -> String {
        let store: &TenantStore = &self.store;
        match store.cursor(self.tenant.as_str(), &self.consumer) {
            Ok(Some(at)) => format!("{{\"ok\":true,\"cursor\":{at}}}"),
            Ok(None) => "{\"ok\":true,\"cursor\":null}".to_owned(),
            Err(ref cause) => stopped("store", cause),
        }
    }

    /// Record the host's own work.
    fn work(&self, value: u64) -> String {
        let mut journal = held(&self.journal);
        let tail = journal.tail();
        match journal.compare_and_append(tail, &HostRecord::Work(value)) {
            Ok(_) => "{\"ok\":true,\"worked\":true}".to_owned(),
            Err(ref cause) => stopped("journal", cause),
        }
    }

    /// Answer one command.
    fn apply(&self, line: &str) -> String {
        let mut words = line.split_whitespace();
        let command = words.next();
        let number = words.next().and_then(|word| word.parse::<u64>().ok());
        match (command, number) {
            (Some("work"), Some(value)) => self.work(value),
            (Some("request"), None) => moved(self.stop.request()),
            (Some("begin"), None) => moved(self.stop.begin()),
            (Some("finish"), None) => moved(self.stop.finish()),
            (Some("status"), None) => self.status(),
            (Some("advance"), Some(position)) => self.advance(position),
            (Some("cursor"), None) => self.cursor(),
            _ => stopped("usage", &line),
        }
    }
}

/// Answer every line of standard input, or say what ended the process.
fn serve(paths: &[String; 4]) -> Result<(), IoError> {
    let [ref journal, ref store, ref tenant, ref consumer] = *paths;
    let opened = RecordJournal::open(Path::new(journal)).map_err(IoError::other)?;
    let journal = Arc::new(Mutex::new(opened));
    let token = CancellationToken::new();
    let stop =
        JournaledCancel::restore(Arc::clone(&journal), "the-run", token).map_err(IoError::other)?;
    let session = Session {
        journal,
        stop,
        store: TenantStore::open(Path::new(store)).map_err(IoError::other)?,
        tenant: Tenant::new(tenant.as_str()).map_err(IoError::other)?,
        consumer: consumer.clone(),
    };
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let reply = session.apply(&line?);
        writeln!(out, "{reply}")?;
        out.flush()?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let words: Vec<String> = std::env::args().skip(1).collect();
    let (message, code) = match <[String; 4]>::try_from(words) {
        Ok(paths) => match serve(&paths) {
            Ok(()) => return ExitCode::SUCCESS,
            Err(cause) => (cause.to_string(), ExitCode::from(2)),
        },
        Err(_) => (USAGE.to_owned(), ExitCode::from(64)),
    };
    let mut standard_error = std::io::stderr().lock();
    let told = writeln!(standard_error, "{message}");
    lgwks_std::trace::debug!(?told, "durable-stop: ending on a refusal");
    code
}
