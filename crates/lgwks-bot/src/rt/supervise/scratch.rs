//! A private temp directory for every supervised child, removed with its group.
//!
//! A child writes temp files where `TMPDIR` points. A child killed by a
//! deadline, a cancel or a dropped task never removes them, and a shared temp
//! dir fills until the disk is full (2026-11-23: 110,965 leaked files, 86 GiB,
//! on the estate's build Mac). So each child gets its own directory as
//! `TMPDIR`, `TMP` and `TEMP`, and the process-group guard that owns the child
//! removes it after the group is killed: what a child leaves in temp ends with
//! the child, on every path (INV-BOT-TEMP-1).
//!
//! - **Crash-safe.** The directories live under one root per supervising
//!   process, `lgwks-tmp-<pid>-<n>`, whose `.owner` file that process holds an
//!   exclusive lock on for its lifetime. The kernel drops the lock when the
//!   process dies, `SIGKILL` included, so the first root a supervisor opens
//!   removes every sibling root whose lock it can take. A reused pid never
//!   keeps a dead root alive: the lock decides, not the pid.
//! - **The caller's choice wins.** A spec that sets `TMPDIR`, or clears the
//!   environment (whose contract is exactly what the caller names), gets
//!   nothing injected.
//! - **Writers that ignore `TMPDIR`.** macOS `xcrun`, behind the `/usr/bin`
//!   shims for `cc`, `git` and `clang`, writes its cache in the Darwin user
//!   temp dir whatever `TMPDIR` says, through `xcrun_db-XXXXXXXX`; one cut off
//!   (killed, or past a file-size limit) never removes it. While a group runs,
//!   and when it ends, such files idle past [`ABANDONED_AFTER`] are removed, at
//!   most once per [`SWEEP_EVERY`] per process.
//!
//! Not claimed: a child that writes to a fixed path such as `/tmp/x`, or sets
//! its own `TMPDIR` before it forks, writes where it says.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// The prefix of one supervising process's root of child directories.
const ROOT_PREFIX: &str = "lgwks-tmp-";
/// The file whose exclusive lock marks a root's owner as alive.
const OWNER: &str = ".owner";
/// A file a platform writer abandoned is idle this long: `xcrun` writes and
/// renames its cache within milliseconds.
const ABANDONED_AFTER: Duration = Duration::from_secs(10);
/// The shortest time between two sweeps by one process. A cut-off writer
/// leaked about 1,500 files every few seconds on 2026-11-23, so the bound on
/// what can pile up is this interval plus [`ABANDONED_AFTER`].
const SWEEP_EVERY: Duration = Duration::from_secs(2);
/// The most entries one directory read takes, so a pathological dir bounds it.
const SWEEP_MAX: usize = 1_000_000;
/// Temp writers that ignore `TMPDIR`: a name prefix and the length of the
/// alphanumeric `mkstemp` suffix after it.
const ABANDONED: &[(&str, usize)] = &[("xcrun_db-", 8)];

/// One child's private temp directory, removed when it drops.
#[derive(Debug)]
pub(super) struct Scratch {
    /// The directory the child sees as `TMPDIR`, `TMP` and `TEMP`.
    dir: PathBuf,
}

impl Scratch {
    /// A new directory for one child, or `None` when the caller owns its temp.
    ///
    /// # Errors
    ///
    /// The [`io::Error`] from creating the root or the directory: a child is
    /// never started without the temp scope it was promised.
    pub(super) fn for_spawn(caller_owns_temp: bool) -> io::Result<Option<Self>> {
        if caller_owns_temp {
            return Ok(None);
        }
        let root = process_root(&std::env::temp_dir())?;
        let dir = root.join(NEXT_CHILD.fetch_add(1, Ordering::Relaxed).to_string());
        fs::create_dir(&dir)?;
        Ok(Some(Self { dir }))
    }

    /// The child's directory, under every name a tool reads its temp from.
    pub(super) fn env(&self) -> [(&'static str, &Path); 3] {
        [
            ("TMPDIR", &self.dir),
            ("TMP", &self.dir),
            ("TEMP", &self.dir),
        ]
    }
}

impl Drop for Scratch {
    /// Remove the child's directory, then sweep what writers that ignore
    /// `TMPDIR` abandoned. Best effort: a cleanup never fails a run.
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.dir)
            && error.kind() != io::ErrorKind::NotFound
        {
            lgwks_std::trace::debug!(%error, dir = %self.dir.display(), "scratch: the child's temp dir was not removed");
        }
        sweep_if_due();
    }
}

/// Sweep the abandoned-writer files now when [`SWEEP_EVERY`] has passed.
///
/// Synchronous and bounded by [`SWEEP_MAX`] entries per directory; the driver
/// calls [`sweep_while_running`], which moves it to the blocking pool.
pub(super) fn sweep_if_due() {
    if !sweep_due(Instant::now()) {
        return;
    }
    let now = SystemTime::now();
    let swept: usize = sweep_dirs()
        .iter()
        .map(|dir| sweep_abandoned(dir, now))
        .sum();
    // Every due sweep reports its count, zero included: at most one line per
    // interval, and a quiet zero is how an operator sees the sweep is alive.
    lgwks_std::trace::debug!(swept, "scratch: abandoned temp files removed");
}

/// The sweep a running group's driver makes from its exit poll: due at most
/// once per [`SWEEP_EVERY`], run on the blocking pool, awaited, never detached.
/// A refused pool sheds this sweep; the next poll tries again.
pub(super) async fn sweep_while_running() {
    if !sweep_pending(Instant::now()) {
        return;
    }
    match lgwks_std::task::try_spawn_blocking(sweep_if_due) {
        // The pool's handle yields the job's own value; a panicking job is
        // the pool's to report, and this sweep returns nothing.
        Ok(job) => job.await,
        Err(refused) => {
            lgwks_std::trace::debug!(error = ?refused, "scratch: the blocking pool refused the running sweep");
        }
    }
}

/// The next child directory's number within this process's root.
static NEXT_CHILD: AtomicU64 = AtomicU64::new(0);
/// This process's root and the locked owner file that keeps it alive.
static ROOT: OnceLock<(PathBuf, File)> = OnceLock::new();
/// When this process last swept: milliseconds since [`SWEEP_BASE`], plus one.
static LAST_SWEEP: AtomicU64 = AtomicU64::new(0);
/// The instant sweep times are counted from.
static SWEEP_BASE: OnceLock<Instant> = OnceLock::new();
/// The Darwin user temp dir, read once.
static USER_TEMP: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Where abandoned writers leave files: the Darwin user temp dir `xcrun` uses,
/// and this process's own temp dir, once each.
fn sweep_dirs() -> Vec<PathBuf> {
    let own = std::env::temp_dir();
    let mut dirs = vec![own.clone()];
    if let Some(ref user) = *USER_TEMP.get_or_init(darwin_user_temp) {
        let same = match (user.canonicalize(), own.canonicalize()) {
            (Ok(darwin), Ok(this)) => darwin == this,
            _ => false,
        };
        if !same {
            dirs.push(user.clone());
        }
    }
    dirs
}

/// `getconf DARWIN_USER_TEMP_DIR`: the directory `xcrun` writes to, which
/// `TMPDIR` does not move. `None` off macOS or when it cannot be read.
fn darwin_user_temp() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let output = match std::process::Command::new("/usr/bin/getconf")
        .arg("DARWIN_USER_TEMP_DIR")
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            lgwks_std::trace::debug!(status = ?output.status, "scratch: getconf refused DARWIN_USER_TEMP_DIR");
            return None;
        }
        Err(error) => {
            lgwks_std::trace::debug!(%error, "scratch: getconf could not run");
            return None;
        }
    };
    let text = match String::from_utf8(output.stdout) {
        Ok(text) => text,
        Err(error) => {
            lgwks_std::trace::debug!(%error, "scratch: DARWIN_USER_TEMP_DIR is not UTF-8");
            return None;
        }
    };
    let dir = text.trim();
    if dir.is_empty() {
        None
    } else {
        Some(PathBuf::from(dir))
    }
}

/// This process's root under `parent`, opened once; the first open removes the
/// roots of owners that died.
fn process_root(parent: &Path) -> io::Result<PathBuf> {
    if let Some(entry) = ROOT.get() {
        return Ok(entry.0.clone());
    }
    // Two first spawns may race here. Each opens a root; one wins the set, and
    // the loser removes its own empty root and uses the winner's. No lock: a
    // failed open is retried by the next spawn rather than cached.
    let (root, owner) = open_root(parent)?;
    let swept = sweep_dead_roots(parent, &root);
    lgwks_std::trace::debug!(swept, root = %root.display(), "scratch: dead owners' roots removed");
    if let Err((lost, owner)) = ROOT.set((root, owner)) {
        drop(owner);
        if let Err(error) = fs::remove_dir_all(&lost) {
            lgwks_std::trace::debug!(%error, root = %lost.display(), "scratch: a raced root was not removed; the next supervisor sweeps it");
        }
    }
    if let Some(entry) = ROOT.get() {
        return Ok(entry.0.clone());
    }
    let error = io::Error::other("lgwks_bot: the temp root was set and is not readable");
    lgwks_std::trace::debug!(%error, "scratch: no root after its set");
    Err(error)
}

/// Create a fresh root under `parent`, named by 64 random bits (INV-BOT-116),
/// holding its owner lock. The lock, not the name, says whether its owner lives;
/// `create_dir` refuses a name that exists, so a collision is an error.
fn open_root(parent: &Path) -> io::Result<(PathBuf, File)> {
    let unique = lgwks_std::random::bytes::<8>().map_err(io::Error::other)?;
    let root = parent.join(format!("{ROOT_PREFIX}{:016x}", u64::from_be_bytes(unique)));
    fs::create_dir(&root)?;
    let owner = File::create(root.join(OWNER))?;
    owner.lock()?;
    Ok((root, owner))
}

/// Remove every root under `parent` but `keep` whose owner lock is free.
///
/// A root whose owner file cannot be opened is left alone: only a lock this
/// process took proves its owner gone. Returns how many roots were removed.
fn sweep_dead_roots(parent: &Path, keep: &Path) -> usize {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) => {
            lgwks_std::trace::debug!(%error, "scratch: the temp dir is unreadable, so no dead root is swept");
            return 0;
        }
    };
    let mut removed = 0_usize;
    for entry in entries.flatten().take(SWEEP_MAX) {
        let path = entry.path();
        let named = match entry.file_name().to_str() {
            Some(name) => name.starts_with(ROOT_PREFIX),
            None => false,
        };
        if !named || path == keep {
            continue;
        }
        let Ok(owner) = File::open(path.join(OWNER)) else {
            continue;
        };
        if owner.try_lock().is_ok() && fs::remove_dir_all(&path).is_ok() {
            removed = removed.saturating_add(1);
        }
    }
    removed
}

/// Milliseconds since [`SWEEP_BASE`] at `now`, plus one so zero means never.
fn sweep_clock(now: Instant) -> u64 {
    let base = *SWEEP_BASE.get_or_init(|| now);
    let elapsed = now.saturating_duration_since(base).as_millis();
    match u64::try_from(elapsed) {
        Ok(ms) => ms.saturating_add(1),
        Err(_) => u64::MAX,
    }
}

/// Whether `at` is [`SWEEP_EVERY`] or more past `last`, where `last == 0` is
/// never swept.
fn elapsed_enough(last: u64, at: u64) -> bool {
    last == 0 || u128::from(at.saturating_sub(last)) >= SWEEP_EVERY.as_millis()
}

/// Whether a sweep is due at `now`, without claiming it.
fn sweep_pending(now: Instant) -> bool {
    elapsed_enough(LAST_SWEEP.load(Ordering::Relaxed), sweep_clock(now))
}

/// Whether a sweep is due at `now`, claiming it when it is: one sweeper wins.
fn sweep_due(now: Instant) -> bool {
    let last = LAST_SWEEP.load(Ordering::Relaxed);
    let at = sweep_clock(now);
    elapsed_enough(last, at)
        && LAST_SWEEP
            .compare_exchange(last, at, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}

/// Whether `name`, a regular file last written at `modified`, is a temp file a
/// writer that ignores `TMPDIR` abandoned, judged at `now`.
fn abandoned(name: &str, regular: bool, modified: SystemTime, now: SystemTime) -> bool {
    let shaped = ABANDONED
        .iter()
        .any(|&(prefix, len)| match name.strip_prefix(prefix) {
            Some(rest) => {
                rest.len() == len && rest.bytes().all(|byte| byte.is_ascii_alphanumeric())
            }
            None => false,
        });
    let idle = match now.duration_since(modified) {
        Ok(idle) => idle >= ABANDONED_AFTER,
        // Written after `now`: a live write, or a clock step. Never abandoned.
        Err(ahead) => {
            lgwks_std::trace::debug!(ahead = ?ahead.duration(), name, "scratch: a temp file is newer than the sweep, so it stays");
            false
        }
    };
    shaped && regular && idle
}

/// Remove the abandoned temp files directly in `dir`, judged at `now`.
/// Returns how many were removed.
fn sweep_abandoned(dir: &Path, now: SystemTime) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0_usize;
    for entry in entries.flatten().take(SWEEP_MAX) {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if abandoned(&name, meta.is_file(), modified, now) && fs::remove_file(entry.path()).is_ok()
        {
            removed = removed.saturating_add(1);
        }
    }
    removed
}

#[cfg(test)]
#[path = "sim_scratch.rs"]
mod sim_scratch;
