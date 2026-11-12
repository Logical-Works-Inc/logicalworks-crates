//! `command-journal`: a durable write as an app command, its request held until it lands.
//!
//! ```text
//! command-journal <directory> [--stall-after-write KEY]
//! ```
//!
//! Each tenant writes its own journal file, under a bearer key `key-{tenant}`:
//!
//! ```text
//! POST /t/{tenant}/write {"key":"k","value":7}   written 7, or replayed 7 on a retry
//! POST /t/{tenant}/count                         one unkeyed write, for a load client
//! GET  /t/{tenant}/count                         how many records the tenant holds
//! ```
//!
//! The first stdout line is `listening ADDR`, and stdin closing stops it. The process
//! can die between a write and its reply. The client then sees its request fail, the
//! write stands, and a retry with the same key answers `replayed` with what landed.
//! `--stall-after-write KEY` prints `wrote KEY` at exactly that point and holds, so a
//! test can kill it there.
//!
//! Writers commit in groups. Each writer queues its record, and one of them leads:
//! it takes the queue, syncs it as one append and wakes the rest. The queue is bounded
//! by the command threads, since each queued record holds one.

use std::collections::BTreeMap;
use std::io::{BufRead as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

use lgwks_bot::Cap;
use lgwks_bot::app::{
    App, CommandLimits, Effect, EffectResult, Method, OnInput, Principal, Provider, ReplyTag,
    RequestHeaders,
};
use lgwks_bot::journal::{JournalError, RecordJournal};
use lgwks_bot::rt::edge::EdgeStop;

/// The usage text, for a first-timer and for a misuse.
const USAGE: &str = "usage: command-journal <directory> [--stall-after-write KEY]";

/// The most tenants one process holds open; each costs a storage thread and two files.
const MAX_TENANTS: usize = 64;

/// The longest idempotency key taken.
const MAX_KEY_BYTES: usize = 256;

/// Threads that run writes; a group commit takes whatever queued while one synced.
const COMMAND_THREADS: usize = 32;

/// Writes one tenant holds, queued and running, before it hears 503.
const PER_TENANT: usize = 256;

/// How long a write may hold its request before the client hears 504 and retries.
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// The journal's command limits. The host holds every tenant's whole share, so a
/// tenant at its ceiling never fills the queue the other tenants wait in.
fn limits() -> CommandLimits {
    CommandLimits::new(
        COMMAND_THREADS,
        MAX_TENANTS * PER_TENANT,
        PER_TENANT,
        DEADLINE,
    )
}

/// What a client hears when a write may not have landed.
const RETRY: &[u8] = b"the write may not have landed; retry it with the same key";

/// One write: its idempotency key, empty when it has none, and its value.
type Entry = (String, u64);

/// One keyed write as posted.
#[derive(lgwks_std::json::Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
struct Write {
    /// The key a retry repeats.
    key: String,
    /// What it writes.
    value: u64,
}

/// What one tenant's writers share.
#[derive(Default)]
struct Pending {
    /// Records no leader has taken, each with where its outcome goes.
    queue: Vec<(Entry, Arc<OnceLock<bool>>)>,
    /// Keys a writer has queued and no outcome settled, so a retry waits on the first.
    in_flight: BTreeMap<String, Arc<OnceLock<bool>>>,
    /// Every key that landed and its value. The journal's own ceiling bounds it.
    landed: BTreeMap<String, u64>,
    /// How many records landed.
    count: u64,
    /// Whether a leader is appending now.
    leading: bool,
}

/// One tenant's journal, and the writers waiting on it.
struct Book {
    /// The writers' shared state, never held across an append.
    pending: Mutex<Pending>,
    /// Rung when a leader finishes.
    bell: Condvar,
    /// The journal, touched only by the leader.
    journal: Mutex<RecordJournal<Entry>>,
}

impl Book {
    /// Open `path` and rebuild what landed from it.
    fn open(path: &Path) -> Result<Self, JournalError> {
        let journal = RecordJournal::<Entry>::open(path)?;
        let mut pending = Pending::default();
        for entry in journal.replay()? {
            let held = entry?;
            let (ref key, value) = *held.record();
            pending.count = pending.count.saturating_add(1);
            if !key.is_empty() {
                pending.landed.insert(key.clone(), value);
            }
        }
        Ok(Self {
            pending: Mutex::new(pending),
            bell: Condvar::new(),
            journal: Mutex::new(journal),
        })
    }

    /// Append `queue` as one sync, and whether it landed.
    fn append(&self, queue: &[(Entry, Arc<OnceLock<bool>>)]) -> bool {
        let records: Vec<Entry> = queue.iter().map(|queued| queued.0.clone()).collect();
        let Ok(mut journal) = self.journal.lock() else {
            lgwks_std::trace::warn!("command-journal: the journal lock is poisoned");
            return false;
        };
        let tail = journal.tail();
        match journal.compare_and_append_all(tail, &records) {
            Ok(_) => true,
            Err(error) => {
                lgwks_std::trace::warn!(%error, "command-journal: an append was refused");
                false
            }
        }
    }

    /// Lead one group: take the queue, sync it, settle every outcome, step down.
    fn lead<'book>(
        &'book self,
        mut pending: MutexGuard<'book, Pending>,
        stall: &Stall,
    ) -> Option<MutexGuard<'book, Pending>> {
        pending.leading = true;
        let queue = std::mem::take(&mut pending.queue);
        drop(pending);
        let landed = self.append(&queue);
        if landed {
            stall.hold(&queue);
        }
        let Ok(mut settled) = self.pending.lock() else {
            lgwks_std::trace::warn!("command-journal: the pending lock is poisoned");
            return None;
        };
        for ((key, value), outcome) in queue {
            if landed {
                settled.count = settled.count.saturating_add(1);
            }
            if !key.is_empty() {
                settled.in_flight.remove(&key);
                if landed {
                    settled.landed.insert(key, value);
                }
            }
            if outcome.set(landed).is_err() {
                lgwks_std::trace::warn!("command-journal: an outcome was settled twice");
            }
        }
        settled.leading = false;
        self.bell.notify_all();
        Some(settled)
    }

    /// Write `value` under `key`, once, and say what happened.
    fn write(&self, key: &str, value: u64, stall: &Stall) -> Option<Vec<u8>> {
        let mut pending = self.pending.lock().ok()?;
        if let Some(stored) = pending.landed.get(key) {
            return Some(format!("replayed {stored}").into_bytes());
        }
        let waiting = pending.in_flight.get(key).cloned();
        let retry = waiting.is_some();
        let outcome = match waiting {
            Some(first) => first,
            None => {
                let mine = Arc::new(OnceLock::new());
                if !key.is_empty() {
                    pending.in_flight.insert(key.to_owned(), Arc::clone(&mine));
                }
                pending
                    .queue
                    .push(((key.to_owned(), value), Arc::clone(&mine)));
                mine
            }
        };
        let landed = loop {
            if let Some(&landed) = outcome.get() {
                break landed;
            }
            pending = if pending.leading {
                self.bell.wait(pending).ok()?
            } else {
                self.lead(pending, stall)?
            };
        };
        if !landed {
            return None;
        }
        Some(if retry {
            let stored = pending.landed.get(key).copied()?;
            format!("replayed {stored}").into_bytes()
        } else {
            format!("written {value}").into_bytes()
        })
    }

    /// How many records this tenant holds.
    fn count(&self) -> Option<Vec<u8>> {
        let pending = self.pending.lock().ok()?;
        Some(pending.count.to_string().into_bytes())
    }
}

/// The point a test kills this process at: after one key's sync, before its reply.
struct Stall {
    /// The key to stop at, if any.
    key: Option<String>,
    /// Whether stdin closed, which lets a held writer go on.
    stopping: Mutex<bool>,
    /// Rung when stdin closes.
    bell: Condvar,
}

impl Stall {
    /// If `queue` holds the key, say so on stdout and wait for stdin to close.
    fn hold(&self, queue: &[(Entry, Arc<OnceLock<bool>>)]) {
        let Some(ref key) = self.key else {
            return;
        };
        if !queue.iter().any(|queued| queued.0.0 == *key) {
            return;
        }
        let mut out = std::io::stdout().lock();
        if let Err(cause) = writeln!(out, "wrote {key}").and_then(|()| out.flush()) {
            lgwks_std::trace::warn!(%cause, "command-journal: stdout is closed");
        }
        drop(out);
        let Ok(mut stopping) = self.stopping.lock() else {
            return;
        };
        while !*stopping {
            stopping = match self.bell.wait(stopping) {
                Ok(woke) => woke,
                Err(_) => return,
            };
        }
    }

    /// Let a held writer go on.
    fn release(&self) {
        if let Ok(mut stopping) = self.stopping.lock() {
            *stopping = true;
        }
        self.bell.notify_all();
    }
}

/// Every open tenant's book.
struct Ledger {
    /// Where each tenant's journal lives.
    directory: PathBuf,
    /// The books open now, at most [`MAX_TENANTS`].
    open: Mutex<BTreeMap<Box<str>, Arc<Book>>>,
    /// Where a test may stop a writer.
    stall: Arc<Stall>,
}

impl Ledger {
    /// `tenant`'s book, opened on first use. The app's own check on a tenant's name
    /// is what makes the name safe as a file name.
    fn book(&self, tenant: &str) -> Option<Arc<Book>> {
        let mut open = self.open.lock().ok()?;
        if let Some(found) = open.get(tenant) {
            return Some(Arc::clone(found));
        }
        if open.len() >= MAX_TENANTS {
            lgwks_std::trace::warn!(tenant, "command-journal: past the open tenants");
            return None;
        }
        let path = self.directory.join(format!("{tenant}.journal"));
        match Book::open(&path) {
            Ok(book) => {
                let book = Arc::new(book);
                open.insert(tenant.into(), Arc::clone(&book));
                Some(book)
            }
            Err(error) => {
                lgwks_std::trace::warn!(%error, tenant, "command-journal: a journal will not open");
                None
            }
        }
    }
}

impl Provider for Ledger {
    fn dispatch(&self, _: &str, _: &Effect) -> Option<Vec<u8>> {
        None
    }

    fn command(&self, tenant: &str, _: Option<&Principal>, body: &[u8]) -> Option<Vec<u8>> {
        let book = self.book(tenant)?;
        let text = std::str::from_utf8(body).ok()?;
        if text == COUNT {
            return book.count();
        }
        let (value, key) = text.split_once(' ')?;
        book.write(key, value.parse().ok()?, &self.stall)
    }
}

/// The command that reads a tenant's count.
const COUNT: &str = "count";

/// A command answered at the `landed` handler.
fn command(body: String) -> Vec<Effect> {
    vec![Effect::Command {
        body: body.into_bytes().into_boxed_slice(),
        reply_to: ReplyTag::new("landed"),
    }]
}

/// The app: three routes, each one command, answered at one handler.
fn journal_app(ledger: Ledger) -> App<()> {
    App::new("command-journal")
        .state::<()>()
        .grant(Cap::new("app.command"))
        .command_limits(limits())
        .provider(ledger)
        .authenticate(|tenant: &str, headers: &RequestHeaders<'_>| {
            if headers.bearer()? == format!("key-{tenant}") {
                Principal::new(tenant)
            } else {
                None
            }
        })
        .route_as(
            Method::POST,
            "/write",
            |_: &mut (), _: &Principal, write: Write| {
                if write.key.is_empty() || write.key.len() > MAX_KEY_BYTES {
                    let why = format!("a key is 1 to {MAX_KEY_BYTES} bytes");
                    return vec![Effect::reply(400, why.into_bytes())];
                }
                command(format!("{} {}", write.value, write.key))
            },
        )
        .route_as(
            Method::POST,
            "/count",
            |_: &mut (), _: &Principal, (): ()| command(String::from("1 ")),
        )
        .route_as(
            Method::GET,
            "/count",
            |_: &mut (), _: &Principal, (): ()| command(COUNT.to_owned()),
        )
        .on(
            ReplyTag::new("landed"),
            |_: &mut (), input: &OnInput| match *input.result() {
                EffectResult::Value(ref bytes) => vec![Effect::reply(200, bytes.to_vec())],
                EffectResult::Refused(_) => vec![Effect::reply(403, b"refused".to_vec())],
                _ => vec![Effect::reply(503, RETRY.to_vec())],
            },
        )
}

/// Block until stdin reaches its end.
fn until_stdin_closes() {
    let mut line = String::new();
    let mut input = std::io::stdin().lock();
    while matches!(input.read_line(&mut line), Ok(read) if read > 0) {
        line.clear();
    }
}

/// Why this server stopped.
#[derive(Debug)]
enum Stopped {
    /// The journal directory, the socket or standard output failed.
    Io(std::io::Error),
    /// The app was refused at start or stopped serving.
    App(lgwks_bot::app::AppError),
}

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Io(ref cause) => write!(f, "input or output failed: {cause}"),
            Self::App(ref cause) => write!(f, "the app stopped: {cause}"),
        }
    }
}

impl std::error::Error for Stopped {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Io(ref cause) => Some(cause),
            Self::App(ref cause) => Some(cause),
        }
    }
}

impl From<std::io::Error> for Stopped {
    fn from(cause: std::io::Error) -> Self {
        Self::Io(cause)
    }
}

impl From<lgwks_bot::app::AppError> for Stopped {
    fn from(cause: lgwks_bot::app::AppError) -> Self {
        Self::App(cause)
    }
}

/// Serve `directory` until stdin closes.
fn run(directory: PathBuf, stall_key: Option<String>) -> Result<(), Stopped> {
    std::fs::create_dir_all(&directory)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "listening {}", listener.local_addr()?)?;
    out.flush()?;
    drop(out);
    let stall = Arc::new(Stall {
        key: stall_key,
        stopping: Mutex::new(false),
        bell: Condvar::new(),
    });
    let ledger = Ledger {
        directory,
        open: Mutex::new(BTreeMap::new()),
        stall: Arc::clone(&stall),
    };
    let stop = EdgeStop::new();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            until_stdin_closes();
            stall.release();
            stop.stop();
        });
        journal_app(ledger).serve(listener, &stop)
    })?;
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let (directory, stall) = match *words.as_slice() {
        [directory] => (directory, None),
        [directory, "--stall-after-write", key] => (directory, Some(key.to_owned())),
        _ => {
            say(USAGE);
            return ExitCode::from(64);
        }
    };
    match run(PathBuf::from(directory), stall) {
        Ok(()) => ExitCode::SUCCESS,
        Err(cause) => {
            say(&format!("command-journal stopped: {cause}"));
            ExitCode::FAILURE
        }
    }
}
