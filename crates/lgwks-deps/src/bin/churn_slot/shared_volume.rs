//! The shared RAM volume's lifetime: held while any wrapper runs, detached once
//! it has sat idle for [`IDLE`] (#1160, ADR 0087 refining ADR 0026).
//!
//! A RAM disk keeps every page it was ever written, so a volume kept "until the
//! host restarts" held up to 2 GiB of a 24 GB Mac between runs. Each wrapper
//! now holds the volume through an exclusively locked file in
//! `volume-holders/`, taken under `volume.lock`; the operating system drops the
//! lock with the process, so a killed wrapper's file is stale, not held. The
//! last wrapper to leave stamps `volume-idle` and schedules `churn-slot
//! --release-shared` for [`IDLE`] later; a wrapper that dies with no code run
//! has its lifeline run it at once. The release checks once: it detaches the
//! volume only when, under `volume.lock`, no holder is live and the stamp is
//! that old, and else schedules the next check for the moment it will be. One
//! release runs at a time, under `volume-release.lock`.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long the volume sits with no wrapper before it is detached.
pub(super) const IDLE: Duration = Duration::from_secs(60);

/// The argument that runs `churn-slot` as the release. Only macOS has the
/// shared volume, so only macOS reads it.
#[cfg(target_os = "macos")]
pub(super) const RELEASE_FLAG: &str = "--release-shared";

/// The directory of holder files, under the slot directory.
const HOLDERS: &str = "volume-holders";

/// The file whose modification time is the last moment a holder left.
const IDLE_STAMP: &str = "volume-idle";

/// The lock one release holds while it runs.
const RELEASE_LOCK: &str = "volume-release.lock";

/// The most holder files one count reads; past it the count is refused.
const MAX_HOLDERS: usize = 4_096;

/// `name` under `dir`, opened for writing and created when absent.
fn open(dir: &Path, name: &str) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(name))
}

/// `volume.lock`, held exclusively until the returned file closes.
fn lock_volume(dir: &Path) -> io::Result<File> {
    let file = open(dir, "volume.lock")?;
    file.lock()?;
    Ok(file)
}

/// One wrapper's hold on the shared volume.
#[derive(Debug)]
pub(super) struct Holder {
    /// The locked holder file.
    file: File,
    /// Its path, removed when the wrapper leaves.
    path: PathBuf,
    /// The slot directory.
    dir: PathBuf,
}

/// Take a hold under `dir`, under `volume.lock`, so no release can find the
/// volume unheld between this and the wrapper's use of it.
///
/// # Errors
///
/// A directory, lock or entropy step the host refused.
pub(super) fn hold(dir: &Path) -> io::Result<Holder> {
    let holders = dir.join(HOLDERS);
    fs::create_dir_all(&holders)?;
    let _volume = lock_volume(dir)?;
    let drawn = lgwks_std::random::bytes::<8>().map_err(io::Error::other)?;
    let path = holders.join(format!("{:016x}.lock", u64::from_le_bytes(drawn)));
    let file = File::create_new(&path)?;
    file.lock()?;
    Ok(Holder {
        file,
        path,
        dir: dir.to_path_buf(),
    })
}

impl Holder {
    /// Leave: under `volume.lock`, remove this hold and stamp the moment.
    /// Returns whether no live holder is left, so the caller starts a release.
    ///
    /// # Errors
    ///
    /// A lock, removal, stamp or count the host refused.
    pub(super) fn leave(self) -> io::Result<bool> {
        let _volume = lock_volume(&self.dir)?;
        fs::remove_file(&self.path)?;
        self.file.unlock()?;
        drop(self.file);
        fs::write(self.dir.join(IDLE_STAMP), b"")?;
        Ok(live_holders(&self.dir)? == 0)
    }
}

/// How many holders under `dir` are live, removing each stale one: a file
/// whose lock is free belonged to a wrapper that has ended.
///
/// # Errors
///
/// An unreadable directory or entry, a refused probe, or more than
/// [`MAX_HOLDERS`] files.
pub(super) fn live_holders(dir: &Path) -> io::Result<usize> {
    let holders = dir.join(HOLDERS);
    let entries = match fs::read_dir(&holders) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            lgwks_std::trace::warn!(%error, dir = %holders.display(), "the volume's holders cannot be read");
            return Err(error);
        }
    };
    let mut live = 0_usize;
    for (seen, entry) in entries.enumerate() {
        if seen >= MAX_HOLDERS {
            lgwks_std::trace::warn!(dir = %holders.display(), MAX_HOLDERS, "the volume's holders are past their bound");
            return Err(io::Error::new(
                io::ErrorKind::QuotaExceeded,
                format!("{} holds more than {MAX_HOLDERS} files", holders.display()),
            ));
        }
        let path = entry?.path();
        let probe = OpenOptions::new().write(true).open(&path)?;
        match probe.try_lock() {
            Ok(()) => {
                drop(probe);
                fs::remove_file(&path)?;
            }
            Err(TryLockError::WouldBlock) => live = live.saturating_add(1),
            Err(TryLockError::Error(error)) => {
                lgwks_std::trace::warn!(%error, holder = %path.display(), "a holder's lock cannot be probed");
                return Err(error);
            }
        }
    }
    Ok(live)
}

/// How a release ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Released {
    /// Another release holds the release lock and decides.
    Busy,
    /// A holder is live; it starts the next release when it leaves.
    Held,
    /// A holder left less than [`IDLE`] ago: check again after this long.
    Pending(Duration),
    /// The volume was idle for the bound and was detached.
    Detached,
    /// There was no volume to detach.
    Absent,
}

/// What a release reads and does: the clock and the detach, so a test drives
/// it without a RAM disk or a minute.
pub(super) trait ReleaseHost {
    /// Now.
    fn now(&mut self) -> SystemTime;
    /// Detach the volume mounted at `root`.
    fn detach(&mut self, root: &Path);
}

/// How long ago `stamp` was last written, as of `now`: zero when it was
/// written after `now`, and the whole bound when no holder ever left.
fn age(stamp: &Path, now: SystemTime) -> io::Result<Duration> {
    let written = match fs::metadata(stamp) {
        Ok(meta) => meta.modified()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(IDLE),
        Err(error) => {
            lgwks_std::trace::warn!(%error, stamp = %stamp.display(), "the idle stamp cannot be read");
            return Err(error);
        }
    };
    match now.duration_since(written) {
        Ok(age) => Ok(age),
        Err(ahead) => {
            lgwks_std::trace::debug!(ahead = ?ahead.duration(), "the idle stamp is ahead of the clock");
            Ok(Duration::ZERO)
        }
    }
}

/// Check once whether the volume mounted at `root`, holders under `dir`, has
/// sat idle for [`IDLE`], and detach it if so. No thread waits here: a
/// [`Released::Pending`] answer is the caller's to check again.
///
/// # Errors
///
/// A lock, count or stamp read the host refused: the volume is then kept.
pub(super) fn release(
    dir: &Path,
    root: &Path,
    host: &mut impl ReleaseHost,
) -> io::Result<Released> {
    let release = open(dir, RELEASE_LOCK)?;
    match release.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(Released::Busy),
        Err(TryLockError::Error(error)) => {
            lgwks_std::trace::warn!(%error, "the release lock cannot be taken");
            return Err(error);
        }
    }
    let _volume = lock_volume(dir)?;
    if live_holders(dir)? > 0 {
        return Ok(Released::Held);
    }
    let idle = age(&dir.join(IDLE_STAMP), host.now())?;
    if idle < IDLE {
        return Ok(Released::Pending(IDLE.saturating_sub(idle)));
    }
    if !root.is_dir() {
        return Ok(Released::Absent);
    }
    host.detach(root);
    Ok(Released::Detached)
}

/// A wrapper's tenure on the shared volume: its holder file, and the lifeline
/// hold that starts the release if the wrapper dies with no code run.
#[cfg(target_os = "macos")]
pub(super) struct Tenure {
    /// The holder file, locked while the wrapper runs.
    holder: Holder,
    /// The release this wrapper's lifeline runs at its death.
    lifeline: Option<lgwks_std::process::LifelineHold>,
}

#[cfg(target_os = "macos")]
impl Tenure {
    /// Hold the shared volume under `dir`; a refusal runs the test unheld and
    /// says so, as a slot that cannot be taken does.
    pub(super) fn take(dir: &Path) -> Option<Self> {
        let holder = match hold(dir) {
            Ok(holder) => holder,
            Err(error) => {
                super::say(&format!("the shared volume is not held: {error}"));
                return None;
            }
        };
        let lifeline = match std::env::current_exe()
            .and_then(|exe| lgwks_std::process::hold_run(&exe, Path::new(RELEASE_FLAG)))
        {
            Ok(hold) => Some(hold),
            Err(error) => {
                lgwks_std::trace::debug!(%error, "no lifeline release for the shared volume; a normal exit still starts one");
                None
            }
        };
        Some(Self { holder, lifeline })
    }

    /// Leave the volume, and schedule its release when this was the last
    /// holder. The lifeline hold goes last, so a kill between the two still
    /// runs a release.
    pub(super) fn leave(self) {
        let dir = self.holder.dir.clone();
        match self.holder.leave() {
            Ok(true) => schedule_release(&dir, IDLE, Due::Take),
            Ok(false) => {}
            Err(error) => super::say(&format!("the shared volume's hold was not let go: {error}")),
        }
        drop(self.lifeline);
    }
}

/// The `sh` that runs `churn-slot --release-shared` once `$2` seconds pass:
/// `$0` is the binary and `$1` the flag, both passed as arguments, never
/// spliced into the text.
#[cfg(target_os = "macos")]
const DELAYED: &str = r#"/bin/sleep "$2" && exec "$0" "$1""#;

/// The lock the one scheduled release holds, as its standard input, from the
/// moment it is scheduled until its check ends.
#[cfg(target_os = "macos")]
const DUE_LOCK: &str = "volume-release-due.lock";

/// Set on a scheduled release, whose standard input is the held [`DUE_LOCK`].
#[cfg(target_os = "macos")]
const SCHEDULED_VAR: &str = "LWC_VOLUME_RELEASE_SCHEDULED";

/// Who holds [`DUE_LOCK`] for the release about to be scheduled.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
enum Due {
    /// Nobody yet: take it, or find one already scheduled and add none.
    Take,
    /// This process, a scheduled release, through its standard input.
    Inherited,
}

/// Run the release after `after`, in a child of its own group that outlives
/// this process. The delay is that child's, so no thread here blocks. At most
/// one is scheduled at a time: the child holds [`DUE_LOCK`] through its
/// standard input, and a later leave that finds the lock held adds none, since
/// the scheduled check will see its newer stamp and schedule again.
#[cfg(target_os = "macos")]
fn schedule_release(dir: &Path, after: Duration, due: Due) {
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};
    let input = match due {
        Due::Inherited => Stdio::inherit(),
        Due::Take => {
            let file = match open(dir, DUE_LOCK) {
                Ok(file) => file,
                Err(error) => {
                    super::say(&format!(
                        "the shared volume's release was not scheduled: {error}"
                    ));
                    return;
                }
            };
            match file.try_lock() {
                Ok(()) => Stdio::from(file),
                Err(TryLockError::WouldBlock) => return,
                Err(TryLockError::Error(error)) => {
                    super::say(&format!(
                        "the shared volume's release was not scheduled: {error}"
                    ));
                    return;
                }
            }
        }
    };
    let seconds = after.as_secs().saturating_add(1).to_string();
    let started = std::env::current_exe().and_then(|exe| {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(DELAYED)
            .arg(exe)
            .arg(RELEASE_FLAG)
            .arg(seconds)
            .env(SCHEDULED_VAR, "1")
            .process_group(0)
            .stdin(input)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    });
    match started {
        // Not awaited: the release outlives this wrapper by design.
        Ok(release) => drop(release),
        Err(error) => super::say(&format!(
            "the shared volume's release did not start: {error}"
        )),
    }
}

/// The release's host: the real clock and `hdiutil detach`.
#[cfg(target_os = "macos")]
struct Detacher;

#[cfg(target_os = "macos")]
impl ReleaseHost for Detacher {
    fn now(&mut self) -> SystemTime {
        SystemTime::now()
    }
    fn detach(&mut self, root: &Path) {
        super::ram_disk::run_quietly(
            "hdiutil",
            &["detach", &root.display().to_string(), "-force"],
        );
    }
}

/// `churn-slot --release-shared`: detach the volume at `root` if it is idle,
/// else schedule the next check for the moment it will be.
#[cfg(target_os = "macos")]
pub(super) fn release_shared(dir: &Path, root: &Path) -> std::process::ExitCode {
    let scheduled = std::env::var_os(SCHEDULED_VAR).is_some();
    let due = || if scheduled { Due::Inherited } else { Due::Take };
    match release(dir, root, &mut Detacher) {
        Ok(released) => {
            match released {
                Released::Pending(after) => schedule_release(dir, after, due()),
                // The release running now may have found this one's lock held
                // and scheduled none, so the scheduled one checks again.
                Released::Busy if scheduled => schedule_release(dir, IDLE, Due::Inherited),
                Released::Busy | Released::Held | Released::Detached | Released::Absent => {}
            }
            lgwks_std::trace::debug!(?released, "the shared volume's release ended");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            super::say(&format!("the shared volume's release was refused: {error}"));
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "sim_shared_volume.rs"]
mod sim_shared_volume;
