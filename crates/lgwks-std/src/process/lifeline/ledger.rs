//! The lease ledger: what an owner and its lifeline both died holding is
//! released at the next start of any process that holds (#1160, ADR 0090).
//!
//! The lifeline releases when its owner ends, but a lifeline SIGKILLed while
//! its owner lives, then the owner, leaves no process that knows the holds. So
//! every hold is also a line in a record file, one per owner, written before
//! the hold returns. The owner holds the file's kernel lock (`flock`) for its
//! whole life, and its lifeline holds the same lock through the open file it
//! takes as its stdout, so the lock is free exactly when neither lives. No pid
//! is read to decide it, so a reused pid never makes a live owner look dead.
//!
//! The first hold of every process sweeps the ledger before it writes its own
//! record: each record whose lock it takes belongs to a dead owner and its
//! dead lifeline. Groups are reaped by [`ProcessIdentity`], so a group whose
//! leader's pid was reused is left alone. A device is detached only on the
//! boot it was held on, only while it is still the image it was held as, and
//! only when no live record holds it. Release programs run last. Then the
//! record is removed. A lifeline that releases removes its own record, so no
//! release runs twice.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use crate::process::{ProcessIdentity, identify_process, kill_process_group, reap_orphaned_group};

/// The variable that names the ledger directory, over every default.
pub(super) const DIR_VAR: &str = "LGWKS_LEASE_DIR";

/// The variable a lifeline reads its own record's path from, to remove it
/// once it has released.
pub(super) const PATH_VAR: &str = "LGWKS_LEASE_PATH";

/// The suffix of a record file.
const SUFFIX: &str = ".lease";

/// The first line of every record.
const HEADER: &str = "lgwks-lease 1";

/// The most records one sweep reads; the rest wait for the next start.
const MAX_RECORDS: usize = 4096;

/// The most bytes read from one record; a longer one was not written here.
const MAX_RECORD_BYTES: u64 = 4 << 20;

/// The longest a release program or `hdiutil` call runs before its group is
/// killed.
const RELEASE_BOUND: Duration = Duration::from_secs(10);

/// Lines a record carries before it is rewritten with only its open leases.
pub(super) const COMPACT_AT: usize = 4096;

/// Name a failure nothing else sees, at debug level.
fn note(error: &dyn std::fmt::Display, what: &str) {
    #[cfg(feature = "trace")]
    crate::trace::debug!(%error, what, "ledger: a step was refused");
    #[cfg(not(feature = "trace"))]
    let _unreported = (error, what);
}

/// What one hold asks the ledger to release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Lease {
    /// A process group, by its leader's identity.
    Group(ProcessIdentity),
    /// A disk image, by device and the image path it was attached from.
    Device {
        /// `/dev/diskN`.
        device: String,
        /// The image path `hdiutil info` gave when the hold was taken.
        image: Option<String>,
    },
    /// `<program> <argument>`.
    Run(String, String),
}

impl Lease {
    /// The record line for `id`.
    pub(super) fn line(&self, id: u64) -> String {
        match *self {
            Self::Group(ref leader) => format!("+ {id} g {leader}\n"),
            Self::Device {
                ref device,
                image: Some(ref image),
            } => format!("+ {id} d {device} {image}\n"),
            Self::Device {
                ref device,
                image: None,
            } => format!("+ {id} d {device} -\n"),
            Self::Run(ref program, ref argument) => format!("+ {id} r {program},{argument}\n"),
        }
    }

    /// The lease a record line of `kind` carries in `value`. A value this
    /// module would not have written is named and read as no lease.
    fn parse(kind: &str, value: &str) -> Option<Self> {
        match (kind, value.split_once(' '), value.split_once(',')) {
            ("g", _, _) => match value.parse::<ProcessIdentity>() {
                Ok(leader) => Some(Self::Group(leader)),
                Err(error) => {
                    note(&error, "a record's group line");
                    None
                }
            },
            ("d", Some((device, image)), _) => Some(Self::Device {
                device: device.to_owned(),
                image: (image != "-").then(|| image.to_owned()),
            }),
            ("r", _, Some((program, argument))) if Path::new(program).is_absolute() => {
                let checked =
                    super::checked(Some(program), "release program").and_then(|program| {
                        super::checked(Some(argument), "release argument")
                            .map(|argument| Self::Run(program, argument))
                    });
                match checked {
                    Ok(lease) => Some(lease),
                    Err(error) => {
                        note(&error, "a record's release line");
                        None
                    }
                }
            }
            _ => {
                note(&kind, "a record line of no lease this ledger writes");
                None
            }
        }
    }
}

/// A record read back: the boot it was written on and its open leases.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Folded {
    /// The boot token of the owner's host when it wrote the record.
    pub(super) boot: Option<String>,
    /// Each open lease by id.
    pub(super) leases: BTreeMap<u64, Lease>,
}

/// Fold a record's text. A last line with no newline is torn and ignored, and
/// a line this module never writes is skipped, never guessed at.
pub(super) fn fold(text: &str) -> Folded {
    let mut folded = Folded::default();
    let whole = text.rfind('\n').and_then(|end| text.get(..end));
    for line in whole.into_iter().flat_map(str::lines) {
        if let Some(boot) = line.strip_prefix("boot ") {
            folded.boot = Some(boot.to_owned());
            continue;
        }
        let mut words = line.splitn(4, ' ');
        let (Some(op), Some(Ok(id))) = (words.next(), words.next().map(str::parse::<u64>)) else {
            continue;
        };
        match (op, words.next(), words.next()) {
            ("+", Some(kind), Some(value)) => {
                if let Some(lease) = Lease::parse(kind, value) {
                    folded.leases.insert(id, lease);
                }
            }
            ("-", None, None) => {
                folded.leases.remove(&id);
            }
            _ => {}
        }
    }
    folded
}

/// The directories a ledger may live in, first choice first: `LGWKS_LEASE_DIR`
/// alone when set, else the user state dir and then a per-user temp dir.
pub(super) fn dirs() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os(DIR_VAR).filter(|dir| !dir.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    let mut dirs = Vec::with_capacity(2);
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| Path::new(&home).join(".local/state"))
        });
    if let Some(state) = state {
        dirs.push(state.join("lgwks/leases"));
    }
    let uid = rustix::process::getuid().as_raw();
    dirs.push(std::env::temp_dir().join(format!("lgwks-leases-{uid}")));
    dirs
}

/// This host's boot token: the start token of pid 1, which no later boot
/// repeats. `None` when it cannot be read; a device is then never detached.
fn boot() -> Option<String> {
    match identify_process(1) {
        Ok(found) => found.map(|init| init.started().to_owned()),
        Err(error) => {
            note(&error, "the boot token");
            None
        }
    }
}

/// Make what `file` holds survive this process and a kernel crash:
/// `fsync(2)`. Not `File::sync_data`, which on macOS issues `F_FULLFSYNC`
/// and also empties the drive's own cache, 3.2 ms a write on this Mac
/// (`sim_lifeline_record_rewrite_*`, 4,097 writes in 13.1 s), on every
/// supervised start and stop. `fsync` takes the same writes in 0.149 s.
fn durable(file: &File) -> io::Result<()> {
    rustix::fs::fsync(file).map_err(io::Error::from)
}

/// An `io::Error` of `kind` saying `why`, logged where it is made.
fn refuse(kind: io::ErrorKind, why: String) -> io::Error {
    let refusal = io::Error::new(kind, why);
    #[cfg(feature = "trace")]
    crate::trace::debug!(%refusal, "ledger: returning an error to the caller");
    refusal
}

/// A record file created, locked and headed, at a name no other owner holds.
fn create(dir: &Path, boot: Option<&str>) -> io::Result<(File, PathBuf)> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let nanos = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since) => since.as_nanos(),
        Err(before) => before.duration().as_nanos(),
    };
    for attempt in 0..16_u32 {
        let pid = std::process::id();
        let path = dir.join(format!("{pid}-{nanos}-{attempt}{SUFFIX}"));
        let opened = OpenOptions::new()
            .append(true)
            .create_new(true)
            .mode(0o600)
            .open(&path);
        let mut file = match opened {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                #[cfg(feature = "trace")]
                crate::trace::debug!(%error, path = %path.display(), "ledger: a record could not be created");
                return Err(error);
            }
        };
        if let Err(error) = file.try_lock() {
            #[cfg(feature = "trace")]
            crate::trace::debug!(%error, path = %path.display(), "ledger: a fresh record could not be locked");
            let shown = path.display();
            return Err(refuse(
                io::ErrorKind::WouldBlock,
                format!("ledger: a fresh record {shown} could not be locked: {error}"),
            ));
        }
        let head = match boot {
            Some(boot) => format!("{HEADER}\nboot {boot}\n"),
            None => format!("{HEADER}\n"),
        };
        file.write_all(head.as_bytes())?;
        durable(&file)?;
        return Ok((file, path));
    }
    Err(refuse(
        io::ErrorKind::AlreadyExists,
        format!("ledger: no free record name in {}", dir.display()),
    ))
}

/// This process's own record: locked for its whole life, appended one line per
/// hold and release.
pub(super) struct Record {
    /// The locked file, opened to append.
    file: File,
    /// Where it lives.
    path: PathBuf,
    /// The boot it was written on.
    boot: Option<String>,
    /// Each open lease, for a rewrite.
    leases: BTreeMap<u64, Lease>,
    /// Lines written since the last rewrite.
    lines: usize,
}

impl Record {
    /// Sweep every ledger directory, then open this process's record in the
    /// first one that takes it.
    pub(super) fn open() -> io::Result<Self> {
        let boot = boot();
        let dirs = dirs();
        for dir in &dirs {
            let swept = sweep(dir, boot.as_deref(), &Native);
            #[cfg(feature = "trace")]
            if swept.records > 0 {
                crate::trace::info!(dir = %dir.display(), records = swept.records, released = swept.released, "ledger: released what dead owners held");
            }
            #[cfg(not(feature = "trace"))]
            let _unreported = swept;
        }
        let mut refused = Vec::with_capacity(dirs.len());
        for dir in &dirs {
            match create(dir, boot.as_deref()) {
                Ok((file, path)) => {
                    return Ok(Self {
                        file,
                        path,
                        boot,
                        leases: BTreeMap::new(),
                        lines: 0,
                    });
                }
                Err(error) => refused.push(format!("{}: {error}", dir.display())),
            }
        }
        Err(refuse(
            io::ErrorKind::PermissionDenied,
            format!(
                "ledger: no lease record could be written ({}); set {DIR_VAR} to a writable directory",
                refused.join("; ")
            ),
        ))
    }

    /// Where the record lives.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// A second handle on the record for a lifeline's stdout: the same open
    /// file, so the lifeline holds the same lock.
    pub(super) fn share(&self) -> io::Result<Stdio> {
        Ok(Stdio::from(self.file.try_clone()?))
    }

    /// Write `text` durably. Returns whether the record was rewritten, which
    /// moves it to a new file a running lifeline does not hold.
    fn write(&mut self, text: &str) -> io::Result<bool> {
        self.file.write_all(text.as_bytes())?;
        durable(&self.file)?;
        self.lines = self.lines.saturating_add(1);
        if self.lines < COMPACT_AT || self.lines < self.leases.len().saturating_mul(2) {
            return Ok(false);
        }
        self.rewrite()?;
        Ok(true)
    }

    /// Write only the open leases to a fresh locked file and rename it over
    /// the record, so a crash mid-rewrite leaves the old record whole.
    fn rewrite(&mut self) -> io::Result<()> {
        let Some(dir) = self.path.parent() else {
            #[cfg(feature = "trace")]
            crate::trace::debug!(path = %self.path.display(), "ledger: a record has no directory to rewrite in");
            let shown = self.path.display();
            return Err(refuse(
                io::ErrorKind::InvalidInput,
                format!("ledger: record {shown} has no directory"),
            ));
        };
        let (mut fresh, staged) = create(dir, self.boot.as_deref())?;
        let body: String = self
            .leases
            .iter()
            .map(|(&id, lease)| lease.line(id))
            .collect();
        fresh.write_all(body.as_bytes())?;
        durable(&fresh)?;
        fs::rename(&staged, &self.path)?;
        self.file = fresh;
        self.lines = 0;
        Ok(())
    }

    /// Record lease `id` before its hold returns. Returns whether the record
    /// moved, so the lifeline must be started again on the new one.
    pub(super) fn add(&mut self, id: u64, lease: Lease) -> io::Result<bool> {
        let line = lease.line(id);
        self.leases.insert(id, lease);
        let written = self.write(&line);
        if written.is_err() {
            self.leases.remove(&id);
        }
        written
    }

    /// Record that lease `id` was let go.
    pub(super) fn remove(&mut self, id: u64) -> io::Result<bool> {
        if self.leases.remove(&id).is_none() {
            return Ok(false);
        }
        self.write(&format!("- {id}\n"))
    }
}

/// What a sweep did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Swept {
    /// Dead owners' records removed.
    pub(super) records: usize,
    /// Leases released.
    pub(super) released: usize,
}

/// What a sweep does to the host: the kernel and its tools, or a sim's model.
pub(super) trait Host {
    /// Reap the group `leader` led, if `leader` still holds its pid.
    fn reap(&self, leader: &ProcessIdentity);
    /// The image path `device` is attached from now, if any.
    fn image(&self, device: &str) -> Option<String>;
    /// Detach `device`.
    fn detach(&self, device: &str);
    /// Run `program argument`.
    fn run(&self, program: &str, argument: &str);
}

/// The host this process runs on.
pub(super) struct Native;

impl Host for Native {
    fn reap(&self, leader: &ProcessIdentity) {
        if let Err(error) = reap_orphaned_group(leader) {
            note(&error, "a dead owner's group reap");
        }
    }

    fn image(&self, device: &str) -> Option<String> {
        image_of(device)
    }

    fn detach(&self, device: &str) {
        if bounded("/usr/bin/hdiutil", &["detach", device, "-force"]).is_none() {
            note(&device, "a dead owner's device detach");
        }
    }

    fn run(&self, program: &str, argument: &str) {
        if bounded(program, &[argument]).is_none() {
            note(&program, "a dead owner's release program");
        }
    }
}

/// Run `program args` in its own group with its stdout read, its whole group
/// killed past [`RELEASE_BOUND`]. `None` when it did not end with status 0.
fn bounded(program: &str, args: &[&str]) -> Option<String> {
    bounded_within(program, args, RELEASE_BOUND)
}

/// [`bounded`] under a `bound` of the caller's, so a test reaches it in
/// milliseconds.
pub(super) fn bounded_within(program: &str, args: &[&str], bound: Duration) -> Option<String> {
    let spawned = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            note(&error, "a release start");
            return None;
        }
    };
    let pgid = match i32::try_from(child.id()) {
        Ok(pgid) => Some(pgid),
        Err(error) => {
            note(&error, "a release pid past pid_t");
            None
        }
    };
    let stdout = child.stdout.take();
    // One slot: the waiter sends exactly one exit, so the send never blocks.
    let (exited, exit) = std::sync::mpsc::sync_channel::<io::Result<std::process::ExitStatus>>(1);
    std::thread::scope(|scope| {
        let reader = scope.spawn(move || {
            let mut text = String::new();
            let read = match stdout {
                Some(out) => out.take(MAX_RECORD_BYTES).read_to_string(&mut text),
                None => Ok(0),
            };
            read.map(|_| text)
        });
        let waiter = scope.spawn(move || {
            if let Err(error) = exited.send(child.wait()) {
                note(&error, "a release exit nobody read");
            }
        });
        let success = match exit.recv_timeout(bound) {
            Ok(Ok(status)) => status.success(),
            Ok(Err(error)) => {
                note(&error, "a release wait");
                false
            }
            Err(error) => {
                note(&error, "a release past its bound");
                if let Some(pgid) = pgid
                    && let Err(error) = kill_process_group(pgid)
                {
                    note(&error, "a release group kill");
                }
                false
            }
        };
        // The kill ends the child, so the waiter and the reader end with it.
        match (success, waiter.join(), reader.join()) {
            (true, Ok(()), Ok(Ok(text))) => Some(text),
            _ => None,
        }
    })
}

/// The image path `device` is attached from, as `hdiutil info` prints it.
/// `None` off macOS or when no image owns the device.
pub(super) fn image_of(device: &str) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let info = bounded("/usr/bin/hdiutil", &["info"])?;
    image_in(&info, device)
}

/// [`image_of`] over `hdiutil info`'s text: blocks split by `=` rules, each
/// naming its `image-path` and, one per line, the devices it attached.
pub(super) fn image_in(info: &str, device: &str) -> Option<String> {
    for block in info.split("\n====") {
        let mut image = None;
        let mut holds = false;
        for line in block.lines() {
            if let Some((key, value)) = line.split_once(':')
                && key.trim() == "image-path"
            {
                image = Some(value.trim().to_owned());
            }
            holds |= line.split_whitespace().next() == Some(device);
        }
        if holds {
            return image.filter(|image| !image.is_empty());
        }
    }
    None
}

/// A record whose lock this sweep took: its owner and its lifeline are dead.
struct Dead {
    /// The open, locked file, held until the record is removed.
    _file: File,
    /// Where it lives.
    path: PathBuf,
    /// What it still held.
    folded: Folded,
}

/// The text of the record at `path`, at most [`MAX_RECORD_BYTES`].
fn read(path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_RECORD_BYTES)
        .read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Whether the open `file` is still the one `path` names: a record rewritten
/// meanwhile was renamed over, and the old file is not the record.
pub(super) fn still_named(file: &File, path: &Path) -> bool {
    match (file.metadata(), fs::metadata(path)) {
        (Ok(open), Ok(named)) => open.ino() == named.ino() && open.dev() == named.dev(),
        _ => false,
    }
}

/// The record files in `dir`, sorted, at most [`MAX_RECORDS`].
fn records(dir: &Path) -> Vec<PathBuf> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            note(&error, "a ledger directory read");
            return Vec::new();
        }
    };
    let mut names: Vec<PathBuf> = entries
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry.path()),
            Err(error) => {
                note(&error, "a ledger entry read");
                None
            }
        })
        .filter(|path| path.to_string_lossy().ends_with(SUFFIX))
        .collect();
    names.sort();
    names.truncate(MAX_RECORDS);
    names
}

/// The record at `path` when its owner and lifeline are dead, locked; else
/// `None`, with every device a live record holds added to `live`.
fn claim(path: PathBuf, live: &mut BTreeSet<String>) -> Option<Dead> {
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) => {
            note(&error, "a record open");
            return None;
        }
    };
    if file.try_lock().is_err() || !still_named(&file, &path) {
        match read(&path) {
            Ok(text) => live.extend(fold(&text).leases.into_values().filter_map(
                |lease| match lease {
                    Lease::Device { device, .. } => Some(device),
                    Lease::Group(_) | Lease::Run(..) => None,
                },
            )),
            Err(error) => note(&error, "a live record read"),
        }
        return None;
    }
    // Read under the lock, so no write of the dead owner is still to come.
    match read(&path) {
        Ok(text) => Some(Dead {
            _file: file,
            path,
            folded: fold(&text),
        }),
        Err(error) => {
            note(&error, "a dead record read");
            None
        }
    }
}

/// Release what one dead record held: groups, then devices, then programs.
fn release(
    folded: &Folded,
    boot: Option<&str>,
    live: &BTreeSet<String>,
    host: &impl Host,
) -> usize {
    let same_boot = boot.is_some() && folded.boot.as_deref() == boot;
    let mut released = 0_usize;
    for lease in folded.leases.values() {
        if let Lease::Group(ref leader) = *lease {
            host.reap(leader);
            released = released.saturating_add(1);
        }
    }
    for lease in folded.leases.values() {
        if let Lease::Device {
            ref device,
            image: Some(ref image),
        } = *lease
            && same_boot
            && !live.contains(device)
            && host.image(device).as_deref() == Some(image.as_str())
        {
            host.detach(device);
            released = released.saturating_add(1);
        }
    }
    for lease in folded.leases.values() {
        if let Lease::Run(ref program, ref argument) = *lease {
            host.run(program, argument);
            released = released.saturating_add(1);
        }
    }
    released
}

/// Sweep `dir`: release what every dead owner's record still holds, then
/// remove the record. `boot` is this host's boot token. A record that cannot
/// be read is left for the next start and named.
pub(super) fn sweep(dir: &Path, boot: Option<&str>, host: &impl Host) -> Swept {
    let mut live = BTreeSet::new();
    let dead: Vec<Dead> = records(dir)
        .into_iter()
        .filter_map(|path| claim(path, &mut live))
        .collect();
    let mut swept = Swept::default();
    for record in dead {
        let released = release(&record.folded, boot, &live, host);
        swept.released = swept.released.saturating_add(released);
        match fs::remove_file(&record.path) {
            Ok(()) => swept.records = swept.records.saturating_add(1),
            Err(error) => note(&error, "a swept record's removal"),
        }
    }
    swept
}

#[cfg(test)]
mod sim_ledger;
