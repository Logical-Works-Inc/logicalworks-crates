//! Seeded sim of the shared volume's tenure and release against a model
//! (#1160, ADR 0087).
//!
//! Each seed drives one slot directory through a drawn run of the hostile
//! moves a test host makes: wrappers arrive, leave, or die with no code run (a
//! SIGKILL closes the lock and leaves the file), a release fires at a drawn
//! age of the idle stamp, another release already runs, the volume is gone.
//! The model is three facts (live holders, whether a holder ever left, and
//! whether the volume is there), and every release must answer what the model
//! says: never a detach while a wrapper holds the volume, never before the
//! idle bound, and a stale holder file swept by the next count. The real code
//! runs on a real file system and real `flock(2)` locks; only the clock is the
//! host's, read relative to the stamp's own time, so no wall clock decides.

use super::{
    HOLDERS, IDLE, IDLE_STAMP, MAX_HOLDERS, RELEASE_LOCK, ReleaseHost, Released, hold,
    live_holders, open, release,
};
use lgwks_std::seeded::Seeded;
use std::fs;
use std::io;
use std::num::TryFromIntError;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Why a sim could not run its checks, by kind.
enum SimError {
    /// The file system or a lock refused a step.
    Io(io::Error),
    /// A drawn count did not fit the trace's word.
    Number(TryFromIntError),
    /// A precondition the sim states in words.
    Said(&'static str),
}

/// Written by hand so a failed test names its cause in words.
impl std::fmt::Debug for SimError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Io(ref error) => write!(out, "the file system refused: {error}"),
            Self::Number(ref error) => write!(out, "a drawn count did not fit: {error}"),
            Self::Said(said) => write!(out, "{said}"),
        }
    }
}

impl From<io::Error> for SimError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<TryFromIntError> for SimError {
    fn from(error: TryFromIntError) -> Self {
        Self::Number(error)
    }
}

/// What a sim reports when a precondition did not hold.
type SimResult = Result<(), SimError>;

/// Seeds swept.
const SEEDS: u64 = 128;

/// Moves per seed.
const STEPS: usize = 40;

/// Most wrappers holding at once.
const MAX_LIVE: usize = 6;

/// A slot directory of this sim's own, removed when it drops.
struct Dir(PathBuf);

impl Dir {
    fn new(seed: u64) -> io::Result<Self> {
        let drawn = lgwks_std::random::bytes::<8>().map_err(io::Error::other)?;
        let unique = u64::from_le_bytes(drawn);
        let path = std::env::temp_dir().join(format!("sim-volume-{seed}-{unique:016x}"));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            lgwks_std::trace::debug!(%error, "a volume sim directory was not removed");
        }
    }
}

/// The release's host for one check: a clock at a set instant, and a detach
/// that counts and takes the volume away.
struct Host {
    now: SystemTime,
    detached: usize,
}

impl ReleaseHost for Host {
    fn now(&mut self) -> SystemTime {
        self.now
    }
    fn detach(&mut self, root: &Path) {
        self.detached = self.detached.saturating_add(1);
        if let Err(error) = fs::remove_dir(root) {
            lgwks_std::trace::debug!(%error, "the sim's volume was not removed at detach");
        }
    }
}

/// The volume's state as the model holds it.
#[derive(Default)]
struct Model {
    /// Wrappers holding now.
    live: usize,
    /// Whether any holder ever left, so the idle stamp exists.
    stamped: bool,
    /// Whether the volume is attached.
    attached: bool,
}

/// A drawn index below `bound`.
fn below(draw: &mut Seeded, bound: usize) -> io::Result<usize> {
    draw.index(bound).map_err(io::Error::other)
}

/// Fold `value` into the trace.
fn fold(trace: &mut u64, value: u64) {
    *trace = (*trace ^ value).wrapping_mul(0x0100_0000_01b3);
}

/// How many holder files are on disk.
fn files(dir: &Path) -> io::Result<usize> {
    match fs::read_dir(dir.join("volume-holders")) {
        Ok(entries) => Ok(entries.count()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

/// What the model says one release answers.
fn expected(model: &Model, age: Duration, busy: bool) -> Released {
    if busy {
        Released::Busy
    } else if model.live > 0 {
        Released::Held
    } else if model.stamped && age < IDLE {
        Released::Pending(IDLE.saturating_sub(age))
    } else if model.attached {
        Released::Detached
    } else {
        Released::Absent
    }
}

/// One release at `age` past the stamp, checked against the model.
fn check_release(
    seed: u64,
    dir: &Path,
    root: &Path,
    model: &mut Model,
    age: Duration,
    busy: bool,
) -> Result<Released, SimError> {
    let base = if model.stamped {
        fs::metadata(dir.join(IDLE_STAMP))?.modified()?
    } else {
        SystemTime::UNIX_EPOCH
    };
    let mut host = Host {
        now: base
            .checked_add(age)
            .ok_or(SimError::Said("a clock past its range"))?,
        detached: 0,
    };
    let other = if busy {
        let lock = open(dir, RELEASE_LOCK)?;
        lock.lock()?;
        Some(lock)
    } else {
        None
    };
    let got = release(dir, root, &mut host)?;
    drop(other);
    let want = expected(model, age, busy);
    assert_eq!(got, want, "seed {seed}: live {}, age {age:?}", model.live);
    let detached = usize::from(want == Released::Detached);
    assert_eq!(
        host.detached, detached,
        "seed {seed}: a detach only when idle"
    );
    if want == Released::Detached {
        model.attached = false;
    }
    if !busy {
        assert_eq!(
            files(dir)?,
            model.live,
            "seed {seed}: every stale holder is swept"
        );
    }
    Ok(got)
}

/// One seed's run; answers its trace, the folded moves and detaches.
fn run(seed: u64) -> Result<u64, SimError> {
    let dir = Dir::new(seed)?;
    let root = dir.0.join("mount");
    let mut draw = Seeded::from_seed(seed);
    let mut model = Model::default();
    let mut holders = Vec::new();
    let mut trace = 0xcbf2_9ce4_8422_2325_u64;
    for _ in 0..STEPS {
        let step = below(&mut draw, 6)?;
        fold(&mut trace, u64::try_from(step)?);
        match step {
            0 if holders.len() < MAX_LIVE => {
                holders.push(hold(&dir.0)?);
                model.live = model.live.saturating_add(1);
            }
            1 | 2 if !holders.is_empty() => {
                let at = below(&mut draw, holders.len())?;
                let holder = holders.swap_remove(at);
                model.live = model.live.saturating_sub(1);
                if step == 1 {
                    let last = holder.leave()?;
                    model.stamped = true;
                    assert_eq!(
                        last,
                        model.live == 0,
                        "seed {seed}: the last to leave says so"
                    );
                } else {
                    // A SIGKILL: the lock goes with the process, the file stays.
                    drop(holder);
                }
            }
            3 | 4 => {
                // Ages below, at and past the bound, in whole seconds.
                let seconds = below(&mut draw, 121)?;
                let age = Duration::from_secs(u64::try_from(seconds)?);
                let busy = below(&mut draw, 8)? == 0;
                let got = check_release(seed, &dir.0, &root, &mut model, age, busy)?;
                fold(&mut trace, u64::from(got == Released::Detached));
            }
            _ => {
                if model.attached {
                    fs::remove_dir(&root)?;
                } else {
                    fs::create_dir(&root)?;
                }
                model.attached = !model.attached;
            }
        }
    }
    Ok(trace)
}

/// Names a saboteur drops into the holders directory: near misses of a holder
/// name, hidden, spaced and non-ASCII.
const JUNK: [&str; 6] = [
    "0000000000000000.lock",
    ".hidden",
    "with space.lock",
    "é.lock",
    "-dash",
    "no-extension",
];

/// A saboteur fills the holders directory with files no wrapper locks, and
/// sometimes a directory, beside the live holders. Model: an unlocked file is
/// a dead wrapper's, swept, and never counts; a directory cannot be probed,
/// so the release is refused and the volume kept (fail closed), never
/// detached under a holder it could not see.
#[test]
fn sim_shared_volume_junk_beside_the_holders_never_frees_a_held_volume() -> SimResult {
    for seed in 0..SEEDS {
        let dir = Dir::new(seed)?;
        let root = dir.0.join("mount");
        fs::create_dir(&root)?;
        let mut draw = Seeded::from_seed(seed);
        let live = below(&mut draw, 3)?;
        let holders = (0..live)
            .map(|_| hold(&dir.0))
            .collect::<io::Result<Vec<_>>>()?;
        let holders_dir = dir.0.join("volume-holders");
        fs::create_dir_all(&holders_dir)?;
        for _ in 0..below(&mut draw, 4)? {
            let name = JUNK
                .get(below(&mut draw, JUNK.len())?)
                .ok_or(SimError::Said("a drawn name past the list"))?;
            fs::write(holders_dir.join(name), b"not a holder")?;
        }
        let saboteur = below(&mut draw, 3)? == 0;
        if saboteur {
            fs::create_dir_all(holders_dir.join("a-directory.lock"))?;
        }
        let mut host = Host {
            now: SystemTime::UNIX_EPOCH,
            detached: 0,
        };
        match release(&dir.0, &root, &mut host) {
            Err(error) => assert!(saboteur, "seed {seed}: refused without a saboteur: {error}"),
            Ok(got) => {
                assert!(
                    !saboteur,
                    "seed {seed}: a directory it cannot probe passed as {got:?}"
                );
                let want = if live > 0 {
                    Released::Held
                } else {
                    Released::Detached
                };
                assert_eq!(got, want, "seed {seed}: {live} live beside junk");
                assert_eq!(
                    files(&dir.0)?,
                    live,
                    "seed {seed}: every junk file is swept"
                );
            }
        }
        let detached = usize::from(!saboteur && live == 0);
        assert_eq!(
            host.detached, detached,
            "seed {seed}: a detach only when unheld"
        );
        drop(holders);
    }
    Ok(())
}

/// #1160: the shared volume held 2 GiB of a 24 GB Mac for hours with no test
/// running. Over every drawn run the release detaches exactly when no wrapper
/// holds the volume and it has sat idle for the bound, and a seed replays.
/// The holders' bound, at its edge: exactly `MAX_HOLDERS` stale files are all
/// swept, and one more is refused as `QuotaExceeded` before any is read, so a
/// flood of files can never make a release walk an unbounded directory.
#[test]
fn sim_shared_volume_refuses_holders_past_their_bound() -> SimResult {
    for extra in [0_usize, 1] {
        let dir = Dir::new(u64::try_from(extra)?)?;
        let holders = dir.0.join(HOLDERS);
        fs::create_dir_all(&holders)?;
        for index in 0..MAX_HOLDERS.saturating_add(extra) {
            fs::write(holders.join(format!("stale-{index}")), b"")?;
        }
        match (extra, live_holders(&dir.0)) {
            (0, Ok(live)) => {
                assert_eq!(live, 0, "every stale holder at the bound is swept");
                assert_eq!(files(&holders)?, 0, "the swept holders are gone");
            }
            (1, Err(error)) => assert_eq!(
                error.kind(),
                io::ErrorKind::QuotaExceeded,
                "one holder past the bound is refused as a quota: {error}"
            ),
            (0, Err(_)) => return Err(SimError::Said("a directory at the bound was refused")),
            _ => return Err(SimError::Said("a directory past the bound was read")),
        }
    }
    Ok(())
}

#[test]
fn sim_shared_volume_detaches_only_when_unheld_and_idle() -> SimResult {
    for seed in 0..SEEDS {
        let first = run(seed)?;
        assert_eq!(first, run(seed)?, "seed {seed}: the run replays");
    }
    Ok(())
}
