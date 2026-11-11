//! `churn-slot`: the nextest wrapper that holds one of a few machine-wide slots for
//! as long as one test process runs (#272).
//!
//! One macOS kernel serialises file-system namespace work across processes, and
//! the thirteen jobs of a run put every test of several suites on it at once: the
//! full `lgwks_bot` suite takes 40 s alone with its temporary files on a RAM disk
//! and about 300 s beside the other jobs. Throughput of that work falls past about
//! four concurrent processes, and a nextest test group limits one job's processes,
//! not the machine's. This binary limits the machine's: it takes an advisory lock on
//! one of `LWC_CHURN_SLOTS` files under `LWC_CHURN_DIR`, runs the test binary
//! nextest names with the arguments nextest gave it, and gives the slot back when
//! the test exits, however it exits (the operating system drops the lock with the
//! process). A test that finds every slot taken waits on one chosen by its process
//! id, so waiters spread over the slots.
//!
//! It is a wrapper binary and not a shell script because macOS removes `DYLD_*`
//! variables when a protected binary such as `/bin/sh` starts a child, and a test
//! binary that loads the toolchain's `libstd` dylib (a proc-macro crate's tests)
//! then fails to start. The slot is a convenience, so a slot that cannot be taken
//! (the directory cannot be made, the lock is unsupported) runs the test uncapped
//! and says so on standard error: a test that does not run is a test that silently
//! passes.
//!
//! A test whose `TMPDIR` is the macOS default (`/var/folders/...`, or none at all)
//! runs on one shared RAM volume instead, made once per boot under a lock and kept,
//! and the slots then default to the host's cores. The default directory is on the
//! physical disk, where each `sync_all` is an `F_FULLFSYNC` that queues behind every
//! other: the full `lgwks_bot` suite took 671 s there on 2026-11-11 against 144 s
//! with its files on a RAM disk at six slots. Off that queue the six slots only
//! idle the cores. A caller that names its own `TMPDIR` (each CI job names a RAM
//! disk of its own) keeps it and keeps the six slots, and `LWC_CHURN_TMP=inherit`
//! keeps the default directory.
//!
//! Usage: `churn-slot <program> [argument...]`. The exit status is the program's;
//! a program ended by a signal exits `128 + signal`.

#[cfg(target_os = "macos")]
#[path = "shared/ram_disk.rs"]
mod ram_disk;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};

/// The slots a machine offers when `LWC_CHURN_SLOTS` names none.
const DEFAULT_SLOTS: NonZeroUsize = match NonZeroUsize::new(6) {
    Some(count) => count,
    None => NonZeroUsize::MIN,
};

/// The most slots a caller may name: a larger number is a typo, and every slot is
/// a file this directory keeps.
const MAX_SLOTS: usize = 64;

/// The directory holding the slot files when `LWC_CHURN_DIR` names none. Absolute
/// and outside `TMPDIR` on purpose: every job of the host sets its own `TMPDIR`
/// (a RAM disk of its own), and the slots are shared by all of them.
const DEFAULT_DIR: &str = "/tmp/lwc-churn";

/// The variable naming how many slots there are.
const SLOTS_VAR: &str = "LWC_CHURN_SLOTS";

/// The variable naming the directory that holds the slot files.
const DIR_VAR: &str = "LWC_CHURN_DIR";

/// The variable that keeps a test on the default temporary directory when it is
/// `inherit`.
const TMP_VAR: &str = "LWC_CHURN_TMP";

/// The prefix of the per-user temporary directory macOS hands a process that
/// names none: on the physical disk.
const DEFAULT_TMP_PREFIX: &str = "/var/folders/";

/// The shared volume's name under `/Volumes`.
#[cfg(target_os = "macos")]
const SHARED_VOLUME: &str = "lgwks-test-tmp";

/// The shared volume's ceiling: 2 GiB of 512-byte sectors, taken only as written.
/// A full `lgwks_bot` suite leaves about 5 MiB behind.
#[cfg(target_os = "macos")]
const SHARED_SECTORS: u64 = 4_194_304;

/// Where a test's temporary files go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Temp {
    /// The caller's own `TMPDIR`, untouched.
    Inherited,
    /// The shared RAM volume.
    Shared,
}

impl Temp {
    /// The choice a caller's environment makes: the shared volume exactly when
    /// `TMPDIR` is unset, empty or the macOS default, and the caller did not ask
    /// to inherit it.
    fn from_env(tmpdir: Option<&OsStr>, choice: Option<&OsStr>) -> Self {
        if choice.is_some_and(|said| said == "inherit") {
            return Self::Inherited;
        }
        match tmpdir.map(|dir| dir.to_string_lossy()) {
            None => Self::Shared,
            Some(dir) if dir.is_empty() || dir.starts_with(DEFAULT_TMP_PREFIX) => Self::Shared,
            Some(_) => Self::Inherited,
        }
    }
}

/// The exit status of a program that could not be started, as a shell reports one.
const UNRUNNABLE: u8 = 127;

/// The exit status of a call with no program to run.
const USAGE: u8 = 2;

/// Why a slot could not be taken.
#[derive(Debug)]
enum SlotError {
    /// The slot directory could not be made.
    Directory {
        /// The directory.
        path: PathBuf,
        /// What the operating system said.
        cause: io::Error,
    },
    /// A slot file could not be opened or locked.
    Slot {
        /// The slot file.
        path: PathBuf,
        /// What the operating system said.
        cause: io::Error,
    },
    /// The shared RAM volume could not be attached or given its directory.
    Volume {
        /// The volume or the directory on it.
        path: PathBuf,
        /// What the operating system said.
        cause: io::Error,
    },
}

impl fmt::Display for SlotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Directory {
                ref path,
                ref cause,
            } => write!(formatter, "cannot make {}: {cause}", path.display()),
            Self::Slot {
                ref path,
                ref cause,
            } => write!(formatter, "cannot lock {}: {cause}", path.display()),
            Self::Volume {
                ref path,
                ref cause,
            } => write!(formatter, "no shared volume at {}: {cause}", path.display()),
        }
    }
}

impl std::error::Error for SlotError {}

/// Logs `error` at the site that produced it, then hands it back as the `Err`.
fn fail<T>(error: SlotError, site: &'static str) -> Result<T, SlotError> {
    lgwks_std::trace::debug!(error = ?error, site, "returning an error to the caller");
    Err(error)
}

/// The machine's slots: how many, and where their files are.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Slots {
    /// The directory holding the slot files.
    dir: PathBuf,
    /// How many slots there are.
    count: NonZeroUsize,
}

/// A slot held: the lock lives as long as this file does.
#[derive(Debug)]
struct Held {
    /// The slot's file; closing it releases the lock.
    file: File,
}

/// The slot directory a caller's environment names: the default when it is unset
/// or empty.
fn dir_from_env(dir: Option<OsString>) -> PathBuf {
    match dir {
        Some(text) if !text.is_empty() => PathBuf::from(text),
        _ => PathBuf::from(DEFAULT_DIR),
    }
}

/// The slots a test on the shared RAM volume defaults to: one per core the host
/// offers, at most [`MAX_SLOTS`].
fn cores() -> NonZeroUsize {
    match std::thread::available_parallelism() {
        Ok(found) => match NonZeroUsize::new(found.get().min(MAX_SLOTS)) {
            Some(count) => count,
            None => DEFAULT_SLOTS,
        },
        Err(error) => {
            lgwks_std::trace::debug!(%error, "cores: the host names no parallelism");
            DEFAULT_SLOTS
        }
    }
}

impl Slots {
    /// The slots a caller's environment names. A count that is unset, not a
    /// number, zero or past [`MAX_SLOTS`] is `fallback`.
    fn from_env(slots: Option<OsString>, dir: PathBuf, fallback: NonZeroUsize) -> Self {
        let named = slots
            .as_deref()
            .and_then(|text| text.to_str())
            .map(|text| text.trim().parse::<usize>());
        let count = match named {
            Some(Ok(number)) if number <= MAX_SLOTS => match NonZeroUsize::new(number) {
                Some(count) => count,
                None => fallback,
            },
            _ => fallback,
        };
        Self { dir, count }
    }

    /// The file of slot `index`.
    fn path_of(&self, index: usize) -> PathBuf {
        self.dir.join(format!("slot-{index}"))
    }

    /// Opens slot `index`, making its file when it is absent. The file is kept
    /// between runs and never removed: a lock on a file another process then
    /// unlinks and recreates would be a lock on a name nobody else holds.
    fn open(&self, index: usize) -> Result<File, SlotError> {
        let path = self.path_of(index);
        match OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
        {
            Ok(file) => Ok(file),
            Err(cause) => fail(SlotError::Slot { path, cause }, "Slots::open"),
        }
    }

    /// Takes a free slot without waiting, trying each from `start`, or `None` when
    /// every slot is held.
    fn try_take(&self, start: usize) -> Result<Option<Held>, SlotError> {
        for step in 0..self.count.get() {
            let index = start.wrapping_add(step) % self.count;
            let file = self.open(index)?;
            match file.try_lock() {
                Ok(()) => return Ok(Some(Held { file })),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(cause)) => {
                    let path = self.path_of(index);
                    return fail(SlotError::Slot { path, cause }, "Slots::try_take");
                }
            }
        }
        Ok(None)
    }

    /// Takes a slot, waiting on the one `start` names when every slot is held.
    fn take(&self, start: usize) -> Result<Held, SlotError> {
        if let Err(cause) = fs::create_dir_all(&self.dir) {
            let path = self.dir.clone();
            return fail(SlotError::Directory { path, cause }, "Slots::take");
        }
        if let Some(held) = self.try_take(start)? {
            return Ok(held);
        }
        let index = start % self.count;
        let file = self.open(index)?;
        match file.lock() {
            Ok(()) => Ok(Held { file }),
            Err(cause) => {
                let path = self.path_of(index);
                fail(SlotError::Slot { path, cause }, "Slots::take")
            }
        }
    }
}

impl Drop for Held {
    /// Gives the slot back. Closing the file would too; releasing it by name
    /// keeps the intent in one place, and a refusal changes nothing because the
    /// operating system drops the lock when the process ends.
    fn drop(&mut self) {
        match self.file.unlock() {
            Ok(()) | Err(_) => {}
        }
    }
}

impl Held {
    /// The slot's file, so a test can see that the lock is on a real file.
    #[cfg(test)]
    fn file(&self) -> &File {
        &self.file
    }
}

/// The exit status a wrapper reports for `status`: the program's own, or
/// `128 + signal` for one a signal ended, as a shell reports it.
fn exit_status_of(status: ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        return low_byte(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return low_byte(signal.saturating_add(128));
        }
    }
    1
}

/// The low byte of `value`, which is all an exit status carries.
fn low_byte(value: i32) -> u8 {
    let [low, _, _, _] = value.to_le_bytes();
    low
}

/// Writes one line to standard error through a locked handle. A refusal to write
/// is not worth failing a test for, so it is dropped.
fn say(line: &str) {
    let mut handle = io::stderr().lock();
    match writeln!(handle, "churn-slot: {line}") {
        Ok(()) | Err(_) => {}
    }
}

/// The slot a taker tries first, and waits on when every slot is held: a random
/// one, so waiters spread over the slots. A process id would do the same but is
/// reused by the operating system, which is why the estate draws from
/// `lgwks_std::random`. A source that refuses starts every taker at the first
/// slot, which is still correct, only less spread. A WASI build has no entropy
/// module (`lgwks_std::random` refuses wasm) and no host to share slots with, so
/// it starts at the first slot.
#[cfg(target_family = "wasm")]
fn start_slot() -> usize {
    0
}

/// The slot a taker tries first: see the WASI variant above.
#[cfg(not(target_family = "wasm"))]
fn start_slot() -> usize {
    match lgwks_std::random::bytes::<8>() {
        Ok(bytes) => match usize::try_from(u64::from_le_bytes(bytes)) {
            Ok(start) => start,
            Err(error) => {
                lgwks_std::trace::debug!(error = ?error, "start_slot: the draw does not fit a usize");
                0
            }
        },
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "start_slot: no entropy for a start slot");
            0
        }
    }
}

/// Makes sure `root` holds a `tmp` directory and returns it, calling `attach` to
/// make `root` when it is absent. The check repeats under an exclusive lock on
/// `lock`, so the tests of a run that start at once attach one volume between them.
fn ensure_temp(
    root: &Path,
    lock: &Path,
    attach: impl FnOnce() -> io::Result<()>,
) -> Result<PathBuf, SlotError> {
    let temp = root.join("tmp");
    if temp.is_dir() {
        return Ok(temp);
    }
    if let Some(parent) = lock.parent()
        && let Err(cause) = fs::create_dir_all(parent)
    {
        let path = parent.to_path_buf();
        return fail(SlotError::Directory { path, cause }, "ensure_temp");
    }
    let guard = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock)
    {
        Ok(file) => file,
        Err(cause) => {
            let path = lock.to_path_buf();
            return fail(SlotError::Slot { path, cause }, "ensure_temp");
        }
    };
    if let Err(cause) = guard.lock() {
        let path = lock.to_path_buf();
        return fail(SlotError::Slot { path, cause }, "ensure_temp");
    }
    if !root.is_dir()
        && let Err(cause) = attach()
    {
        let path = root.to_path_buf();
        return fail(SlotError::Volume { path, cause }, "ensure_temp");
    }
    if let Err(cause) = fs::create_dir_all(&temp) {
        return fail(SlotError::Volume { path: temp, cause }, "ensure_temp");
    }
    drop(Held { file: guard });
    Ok(temp)
}

/// The shared RAM volume's temporary directory, made when it is absent.
#[cfg(target_os = "macos")]
fn shared_temp(dir: &Path) -> Result<PathBuf, SlotError> {
    let root = Path::new("/Volumes").join(SHARED_VOLUME);
    ensure_temp(&root, &dir.join("volume.lock"), || {
        ram_disk::attach(SHARED_VOLUME, SHARED_SECTORS).map(drop)
    })
}

/// No shared volume off macOS: the wrapper runs there only through a caller's
/// own configuration, which keeps its `TMPDIR`.
#[cfg(not(target_os = "macos"))]
fn shared_temp(dir: &Path) -> Result<PathBuf, SlotError> {
    let cause = io::Error::new(
        io::ErrorKind::Unsupported,
        "a RAM volume is made on macOS only",
    );
    let path = dir.to_path_buf();
    fail(SlotError::Volume { path, cause }, "shared_temp")
}

/// The temporary directory a test runs with when the shared volume is wanted, or
/// `None` to keep the caller's.
fn temp_for(choice: Temp, dir: &Path) -> Option<PathBuf> {
    match choice {
        Temp::Inherited => None,
        Temp::Shared => match shared_temp(dir) {
            Ok(temp) => Some(temp),
            Err(error) => {
                say(&format!("keeping TMPDIR: {error}"));
                None
            }
        },
    }
}

/// Runs `program` under a slot, with `temp` as its `TMPDIR` when one is named, and
/// returns its exit status.
fn run(slots: &Slots, temp: Option<&Path>, program: &OsString, arguments: &[OsString]) -> ExitCode {
    let held = match slots.take(start_slot()) {
        Ok(held) => Some(held),
        Err(error) => {
            say(&format!("running uncapped: {error}"));
            None
        }
    };
    let mut command = Command::new(program);
    command.args(arguments);
    if let Some(dir) = temp {
        // A trailing separator, as the macOS default carries one.
        let mut named = dir.as_os_str().to_os_string();
        named.push("/");
        command.env("TMPDIR", named);
    }
    let status = command.status();
    drop(held);
    match status {
        Ok(status) => ExitCode::from(exit_status_of(status)),
        Err(error) => {
            say(&format!(
                "cannot start {}: {error}",
                Path::new(program).display()
            ));
            ExitCode::from(UNRUNNABLE)
        }
    }
}

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let Some(program) = arguments.next() else {
        say("usage: churn-slot <program> [argument...]");
        return ExitCode::from(USAGE);
    };
    let rest: Vec<OsString> = arguments.collect();
    let dir = dir_from_env(std::env::var_os(DIR_VAR));
    let choice = if cfg!(target_os = "macos") {
        Temp::from_env(
            std::env::var_os("TMPDIR").as_deref(),
            std::env::var_os(TMP_VAR).as_deref(),
        )
    } else {
        Temp::Inherited
    };
    let temp = temp_for(choice, &dir);
    let fallback = if temp.is_some() {
        cores()
    } else {
        DEFAULT_SLOTS
    };
    let slots = Slots::from_env(std::env::var_os(SLOTS_VAR), dir, fallback);
    run(&slots, temp.as_deref(), &program, &rest)
}

#[cfg(all(test, not(target_family = "wasm")))]
#[path = "churn_slot/slot_tests.rs"]
mod slot_tests;
