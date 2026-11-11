//! The unit tests of `churn-slot`: the slot table, the environment it reads and the
//! shared volume's attach, each against the wrapper's own private items.

use super::*;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Why a test could not run its checks: the precondition that broke, by kind.
enum Failed {
    /// The file system refused a step the test set up.
    Io(io::Error),
    /// The slot table refused where the test expected it to serve.
    Slot(SlotError),
    /// A precondition the test states in words, such as a slot it expected free.
    Said(String),
}

impl fmt::Debug for Failed {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Io(ref error) => write!(out, "the file system refused: {error}"),
            Self::Slot(ref error) => write!(out, "the slot table refused: {error:?}"),
            Self::Said(ref said) => write!(out, "{said}"),
        }
    }
}

impl From<io::Error> for Failed {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<SlotError> for Failed {
    fn from(error: SlotError) -> Self {
        Self::Slot(error)
    }
}

impl From<&'static str> for Failed {
    fn from(said: &'static str) -> Self {
        Self::Said(said.to_owned())
    }
}

impl From<String> for Failed {
    fn from(said: String) -> Self {
        Self::Said(said)
    }
}

/// A directory name of this test's own under the system temporary directory,
/// distinguished by a random draw so two runs on one machine never share one.
fn scratch(name: &str) -> Result<PathBuf, Failed> {
    let drawn = lgwks_std::random::bytes::<8>().map_err(|error| error.to_string())?;
    let distinguisher = u64::from_le_bytes(drawn);
    Ok(std::env::temp_dir().join(format!("churn-slot-{name}-{distinguisher:016x}")))
}

/// Removes what a test made; a directory that is already gone is fine.
fn clear(dir: &Path) {
    match fs::remove_dir_all(dir) {
        Ok(()) | Err(_) => {}
    }
}

fn slots_in(dir: &Path, count: usize) -> Result<Slots, Failed> {
    let count = NonZeroUsize::new(count).ok_or("a slot count of zero")?;
    Ok(Slots {
        dir: dir.to_path_buf(),
        count,
    })
}

#[test]
fn the_environment_names_the_slots_and_a_bad_value_is_the_fallback() -> Result<(), Failed> {
    let named = Slots::from_env(
        Some("3".into()),
        dir_from_env(Some("/somewhere".into())),
        DEFAULT_SLOTS,
    );
    assert_eq!(named.count.get(), 3);
    assert_eq!(named.dir, PathBuf::from("/somewhere"));
    let wide = NonZeroUsize::new(15).ok_or("fifteen is not zero")?;
    for bad in ["0", "65", "-1", "many", "", "  "] {
        for fallback in [DEFAULT_SLOTS, wide] {
            let chosen = Slots::from_env(Some(bad.into()), dir_from_env(Some("".into())), fallback);
            assert_eq!(chosen.count, fallback, "{bad:?} is not a slot count");
            assert_eq!(chosen.dir, PathBuf::from(DEFAULT_DIR));
        }
    }
    assert_eq!(Slots::from_env(None, dir_from_env(None), wide).count, wide);
    assert_eq!(
        Slots::from_env(Some(" 4 ".into()), dir_from_env(None), wide)
            .count
            .get(),
        4
    );
    assert_eq!(
        Slots::from_env(Some(MAX_SLOTS.to_string().into()), dir_from_env(None), wide)
            .count
            .get(),
        MAX_SLOTS
    );
    assert!((1..=MAX_SLOTS).contains(&cores().get()));
    Ok(())
}

/// The macOS default and an unnamed directory go to the shared volume; a
/// directory the caller named (a CI job's own RAM disk, Linux's `/dev/shm`)
/// and an explicit `inherit` stay where they are.
#[test]
fn only_the_default_temporary_directory_moves_to_the_shared_volume() {
    let default = OsStr::new("/var/folders/_x/pjjf0dh11vq8/T/");
    for (tmpdir, choice, expected) in [
        (None, None, Temp::Shared),
        (Some(OsStr::new("")), None, Temp::Shared),
        (Some(default), None, Temp::Shared),
        (Some(default), Some(OsStr::new("shared")), Temp::Shared),
        (Some(default), Some(OsStr::new("inherit")), Temp::Inherited),
        (None, Some(OsStr::new("inherit")), Temp::Inherited),
        (
            Some(OsStr::new("/Volumes/ci-ram-r3/")),
            None,
            Temp::Inherited,
        ),
        (
            Some(OsStr::new("/dev/shm/lgwks-gate-1")),
            None,
            Temp::Inherited,
        ),
        (Some(OsStr::new("/var/foldersX/")), None, Temp::Inherited),
        (
            Some(OsStr::new("/private/var/folders/x/T/")),
            None,
            Temp::Inherited,
        ),
    ] {
        assert_eq!(
            Temp::from_env(tmpdir, choice),
            expected,
            "TMPDIR {tmpdir:?} with {TMP_VAR} {choice:?}"
        );
    }
}

/// A volume that is there is reused and never attached again; one that is
/// not is attached once however many tests start together, and every one of
/// them gets the same directory.
#[test]
fn takers_starting_at_once_attach_the_volume_once() -> Result<(), Failed> {
    const TAKERS: usize = 64;
    let base = scratch("volume")?;
    let root = base.join("volume");
    let lock = base.join("slots").join("volume.lock");
    let attaches = AtomicUsize::new(0);
    let answers = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..TAKERS {
            let (root, lock, attaches, answers) = (&root, &lock, &attaches, &answers);
            scope.spawn(move || {
                let made = ensure_temp(root, lock, || {
                    attaches.fetch_add(1, Ordering::SeqCst);
                    fs::create_dir_all(root)
                });
                if let Ok(mut seen) = answers.lock() {
                    seen.push(made.map_err(|error| error.to_string()));
                }
            });
        }
    });
    let seen = answers.into_inner().map_err(|error| error.to_string())?;
    assert_eq!(seen.len(), TAKERS);
    let expected = root.join("tmp");
    assert!(
        seen.iter().all(|made| made.as_ref() == Ok(&expected)),
        "{seen:?}"
    );
    assert_eq!(
        attaches.load(Ordering::SeqCst),
        1,
        "one attach for {TAKERS} takers"
    );
    let again = ensure_temp(&root, &lock, || Err(io::Error::other("attached twice")))?;
    assert_eq!(again, expected);
    clear(&base);
    Ok(())
}

/// An attach that fails is the typed volume error, so the wrapper keeps the
/// caller's directory and says why, rather than running a test nowhere.
#[test]
fn a_volume_that_cannot_be_attached_is_a_volume_error() -> Result<(), Failed> {
    let base = scratch("refused")?;
    let root = base.join("volume");
    let refused = ensure_temp(&root, &base.join("volume.lock"), || {
        Err(io::Error::other("hdiutil attached no ram disk"))
    });
    assert!(
        matches!(refused, Err(SlotError::Volume { ref path, .. }) if *path == root),
        "{refused:?}"
    );
    assert!(!root.join("tmp").exists(), "a refused attach makes nothing");
    clear(&base);
    Ok(())
}

#[test]
fn every_slot_is_held_once_and_a_further_try_finds_none() -> Result<(), Failed> {
    let dir = scratch("full")?;
    let slots = slots_in(&dir, 3)?;
    fs::create_dir_all(&dir)?;
    let mut held = Vec::new();
    for _ in 0..3 {
        held.push(slots.try_take(0)?.ok_or("a slot was free")?);
    }
    assert!(
        slots.try_take(0)?.is_none(),
        "a fourth taker must find every slot held"
    );
    assert!(held.iter().all(|slot| slot.file().metadata().is_ok()));
    clear(&dir);
    Ok(())
}

#[test]
fn a_dropped_slot_is_free_for_the_next_taker() -> Result<(), Failed> {
    let dir = scratch("drop")?;
    let slots = slots_in(&dir, 1)?;
    fs::create_dir_all(&dir)?;
    let first = slots.try_take(0)?.ok_or("the only slot was free")?;
    assert!(slots.try_take(0)?.is_none());
    drop(first);
    assert!(
        slots.try_take(0)?.is_some(),
        "releasing the slot must free it"
    );
    clear(&dir);
    Ok(())
}

#[test]
fn takers_on_threads_never_exceed_the_slots() -> Result<(), Failed> {
    const SLOTS: usize = 3;
    const TAKERS: usize = 16;
    let dir = scratch("threads")?;
    let slots = slots_in(&dir, SLOTS)?;
    let inside = AtomicUsize::new(0);
    let widest = AtomicUsize::new(0);
    let failures = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for taker in 0..TAKERS {
            let (slots, inside, widest, failures) = (&slots, &inside, &widest, &failures);
            scope.spawn(move || match slots.take(taker) {
                Ok(held) => {
                    let now = inside.fetch_add(1, Ordering::SeqCst).saturating_add(1);
                    widest.fetch_max(now, Ordering::SeqCst);
                    for _ in 0..50 {
                        std::thread::yield_now();
                    }
                    inside.fetch_sub(1, Ordering::SeqCst);
                    drop(held);
                }
                Err(_) => {
                    failures.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
    });
    assert_eq!(failures.load(Ordering::SeqCst), 0);
    let widest = widest.load(Ordering::SeqCst);
    assert!(
        (1..=SLOTS).contains(&widest),
        "{widest} takers held a slot at once, with {SLOTS} slots"
    );
    clear(&dir);
    Ok(())
}

#[test]
fn a_directory_that_is_a_file_is_an_error_the_wrapper_survives() -> Result<(), Failed> {
    let dir = scratch("blocked")?;
    fs::write(&dir, b"not a directory")?;
    let slots = slots_in(&dir, 2)?;
    assert!(matches!(slots.take(0), Err(SlotError::Directory { .. })));
    fs::remove_file(&dir)?;
    Ok(())
}

#[test]
fn a_program_that_exits_reports_its_own_status() -> Result<(), Failed> {
    for code in [0_i32, 1, 7, 101] {
        let status = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("exit {code}"))
            .status()?;
        assert_eq!(i32::from(exit_status_of(status)), code);
    }
    Ok(())
}
