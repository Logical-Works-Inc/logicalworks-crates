//! The processes of the Restate row, owned: a child is killed and reaped on every
//! exit path, and the worker's notices are read by one thread this module joins.
//!
//! The worker says two things on stdout, one line each: `parked <key>` when a run
//! reaches its approval and `lost <key>` when a run ended without finishing. The
//! driver waits on those lines instead of polling the server, so an approval's
//! delay is counted from the moment the run parked, as the lgwks row counts it.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, ChildStdin, Command};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

use crate::error::BenchError;
use crate::rival_error::RivalError;
use crate::rival_wire::{PARKED_WORD, parse_key};
use crate::stats::parse_ps;

/// The word that opens the notice of a run that ended without finishing.
const LOST_WORD: &str = "lost";

/// The most bytes of one notice line the reader takes.
const NOTICE_BYTES: u64 = 256;

/// How one run came to be waited for no longer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// It reached its approval.
    Parked,
    /// It ended without finishing.
    Lost,
}

/// What the worker has said so far.
#[derive(Debug, Default)]
pub(crate) struct NoticeBook {
    /// Keys of runs parked at their approval.
    parked: HashSet<String>,
    /// Keys of runs that ended lost.
    lost: HashSet<String>,
    /// Why no more notices are coming, once that is so.
    ended: Option<&'static str>,
}

impl NoticeBook {
    /// Take in one notice line. A line that is not a notice about a canonical key
    /// ends the book: the stream is not trusted past it.
    pub(crate) fn apply(&mut self, line: &str) {
        let parsed = line
            .split_once(' ')
            .filter(|(_, key)| parse_key(key).is_some());
        match parsed {
            Some((PARKED_WORD, key)) => {
                self.parked.insert(key.to_owned());
            }
            Some((LOST_WORD, key)) => {
                self.lost.insert(key.to_owned());
            }
            _ => {
                lgwks_std::trace::debug!(line, "a notice does not read");
                self.ended = Some("a notice line does not read");
            }
        }
    }

    /// Mark the stream over for `reason`, unless it already ended for another.
    pub(crate) fn end(&mut self, reason: &'static str) {
        self.ended.get_or_insert(reason);
    }

    /// Take the news of `key`, if there is any. A lost run wins over a parked
    /// one: a run that was lost cannot be waiting at its approval.
    pub(crate) fn take(&mut self, key: &str) -> Option<Outcome> {
        if self.lost.remove(key) {
            self.parked.remove(key);
            return Some(Outcome::Lost);
        }
        self.parked.remove(key).then_some(Outcome::Parked)
    }

    /// Whether there is news of `key`.
    fn has(&self, key: &str) -> bool {
        self.parked.contains(key) || self.lost.contains(key)
    }
}

/// The book, and the signal that says it changed.
#[derive(Debug, Default)]
pub(crate) struct Notices {
    /// What the worker said.
    book: Mutex<NoticeBook>,
    /// Raised on every change.
    changed: Condvar,
}

impl Notices {
    /// The book, under its lock.
    fn lock(&self) -> Result<MutexGuard<'_, NoticeBook>, RivalError> {
        self.book.lock().map_err(|poisoned| {
            lgwks_std::trace::debug!(error = %poisoned, "the notice book's lock was poisoned");
            RivalError::Notices {
                reason: "the notice book's lock was poisoned",
            }
        })
    }

    /// Record one line, or the end of the stream when `line` is `None`.
    fn record(&self, line: Option<&str>, reason: &'static str) -> Result<(), RivalError> {
        let mut book = self.lock()?;
        match line {
            Some(text) => book.apply(text),
            None => book.end(reason),
        }
        drop(book);
        self.changed.notify_all();
        Ok(())
    }

    /// Wait up to `bound` for news of `key`, and take it.
    ///
    /// # Errors
    ///
    /// [`RivalError::WaitTimeout`] when nothing came in time and
    /// [`RivalError::Notices`] when the stream ended with the key unheard.
    pub(crate) fn wait_for(&self, key: &str, bound: Duration) -> Result<Outcome, RivalError> {
        let book = self.lock()?;
        let (mut book, _timeout) = self
            .changed
            .wait_timeout_while(book, bound, |book| !book.has(key) && book.ended.is_none())
            .map_err(|poisoned| {
                lgwks_std::trace::debug!(error = %poisoned, "the notice book's lock was poisoned");
                RivalError::Notices {
                    reason: "the notice book's lock was poisoned",
                }
            })?;
        if let Some(found) = book.take(key) {
            return Ok(found);
        }
        if let Some(reason) = book.ended {
            lgwks_std::trace::debug!(key, reason, "the notices ended before this run's");
            return Err(RivalError::Notices { reason });
        }
        lgwks_std::trace::debug!(key, "a run neither parked nor ended in time");
        Err(RivalError::WaitTimeout {
            key: key.to_owned(),
        })
    }
}

/// Read notices from `reader` into `notices` until it ends or a line does not read.
/// A line longer than [`NOTICE_BYTES`] does not read.
pub(crate) fn pump(reader: impl Read, notices: &Notices) -> Result<(), RivalError> {
    let mut lines = BufReader::new(reader);
    loop {
        let mut bytes = Vec::new();
        let read = lines
            .by_ref()
            .take(NOTICE_BYTES)
            .read_until(b'\n', &mut bytes)
            .map_err(|source| {
                lgwks_std::trace::debug!(%source, "reading the worker's stdout failed");
                RivalError::Notices {
                    reason: "reading the worker's stdout failed",
                }
            })?;
        if read == 0 {
            return notices.record(None, "the worker closed its stdout");
        }
        let whole = bytes.last() == Some(&b'\n');
        let text = std::str::from_utf8(&bytes).map(|text| text.trim_end_matches(['\n', '\r']));
        match text {
            Ok(line) if whole => notices.record(Some(line), "")?,
            _ => return notices.record(Some("-"), "a notice line does not read"),
        }
    }
}

/// A child process that is killed and reaped when this value goes.
pub(crate) struct Owned {
    /// The child, until it is reaped.
    child: Option<Child>,
    /// The write end of its stdin, held open so it knows its parent is alive.
    stdin: Option<ChildStdin>,
    /// What it is, for a log line.
    what: &'static str,
}

impl Owned {
    /// Start `command` as `what`.
    ///
    /// # Errors
    ///
    /// [`RivalError::Spawn`] when it cannot be started.
    pub(crate) fn spawn(what: &'static str, command: &mut Command) -> Result<Self, BenchError> {
        let mut child = command.spawn().map_err(|source| BenchError::Rival {
            source: RivalError::Spawn { what, source },
        })?;
        let stdin = child.stdin.take();
        Ok(Self {
            child: Some(child),
            stdin,
            what,
        })
    }

    /// The stdout of the child, once.
    pub(crate) fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.as_mut().and_then(|child| child.stdout.take())
    }

    /// The process id, while the child is alive.
    pub(crate) fn id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Send the kill signal and wait until the child is gone.
    ///
    /// # Errors
    ///
    /// [`BenchError::Io`] when the signal or the wait fails.
    pub(crate) fn kill_and_reap(&mut self) -> Result<(), BenchError> {
        drop(self.stdin.take());
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        child.kill().map_err(|source| BenchError::Io {
            context: "killing a child of the Restate row",
            source,
        })?;
        child.wait().map_err(|source| BenchError::Io {
            context: "waiting for a killed child of the Restate row",
            source,
        })?;
        Ok(())
    }
}

impl Drop for Owned {
    /// A child never outlives its owner. A failure is traced: nothing can be
    /// returned from a drop.
    fn drop(&mut self) {
        if let Err(error) = self.kill_and_reap() {
            lgwks_std::trace::warn!(%error, what = self.what, "a child was not reaped");
        }
    }
}

/// The resident set of process `pid`, in bytes, as `ps` reads it.
///
/// # Errors
///
/// [`BenchError::Io`] when `ps` cannot run and [`BenchError::MemoryReading`] when
/// it answers anything but a positive whole number of kibibytes.
pub(crate) fn resident_of(pid: u32) -> Result<u64, BenchError> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p"])
        .arg(pid.to_string())
        .output()
        .map_err(|source| BenchError::Io {
            context: "reading a resident set with ps",
            source,
        })?;
    let text = String::from_utf8(output.stdout).map_err(|source| {
        lgwks_std::trace::debug!(%source, "ps did not answer UTF-8");
        BenchError::MemoryReading {
            reason: "ps did not answer UTF-8",
        }
    })?;
    let bytes = parse_ps(&text)?;
    if bytes == 0 {
        lgwks_std::trace::debug!(pid, "ps answered a resident set of zero");
        return Err(BenchError::MemoryReading {
            reason: "ps answered a resident set of zero",
        });
    }
    Ok(bytes)
}
