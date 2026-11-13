//! Fools against the authority rule both journals share: a successor the device
//! refuses is an error, never an absent one (#910 defect 2, INV-BOT-7).
//!
//! Each seed writes a history, seals it into a successor that holds more, and
//! then breaks the successor the way a device does: a name that loops, a
//! generation directory that is a file, or (the control) a successor that is
//! really gone. The oracle is the acknowledged history. A refused open must
//! leave the sealed base byte for byte, and once the fault clears the successor
//! must still hold every record it acknowledged. Before the fix a looping name
//! read as absent: the record journal rolled its seal back and the file journal
//! opened the sealed base as live, two authorities over one history.
//!
//! Unix only: the faults are a symlink loop and `ENOTDIR`, which this crate's
//! Windows leg does not run its journal suites against (INV-CI-PLATFORM-1).

use std::path::{Path, PathBuf};

use super::{
    ContinuationPolicy, EffectEvent, EffectJournal as _, FileJournal, JournalError, RecordJournal,
    successor_path,
};
use crate::effect::{
    ActionDigest, ActionId, AttemptId, EffectIdentity, EffectKey, EffectKeyError, EnvironmentEpoch,
    EnvironmentId, FlowRevision, RunId,
};
use crate::sim_stream::{RECEIPT_START, Stream, TempDir, fold};

/// A test's outcome.
type Outcome = Result<(), Box<dyn std::error::Error>>;

/// The seeds every family replays.
const SEEDS: [u64; 4] = [0x5ACC_0001, 0x5ACC_0002, 0x5ACC_00FF, 0x5ACC_F00D];

/// How a seed breaks the successor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// The successor's name is a symlink to itself: every open is `ELOOP`.
    Loop,
    /// The successor's generation directory is a regular file: `ENOTDIR`.
    NotADirectory,
    /// The control: the successor is really gone, so the seal never committed.
    Gone,
}

/// The faults, in the order a seed draws from.
const FAULTS: [Fault; 3] = [Fault::Loop, Fault::NotADirectory, Fault::Gone];

/// The generation directory a successor sits in.
fn generation_of(next: &Path) -> std::io::Result<&Path> {
    next.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a successor sits in a directory",
        )
    })
}

/// Break `next` with `fault`; the returned path is where the real bytes went.
fn break_successor(next: &Path, fault: Fault) -> std::io::Result<PathBuf> {
    let generation = generation_of(next)?;
    let aside = generation.with_extension("aside");
    std::fs::rename(generation, &aside)?;
    match fault {
        Fault::Loop => {
            std::fs::create_dir(generation)?;
            std::os::unix::fs::symlink(next, next)?;
        }
        Fault::NotADirectory => std::fs::write(generation, b"not a directory")?,
        Fault::Gone => {}
    }
    Ok(aside)
}

/// Put the real successor back where `break_successor` took it from.
fn restore_successor(next: &Path, aside: &Path) -> std::io::Result<()> {
    let generation = generation_of(next)?;
    if generation.is_dir() {
        std::fs::remove_dir_all(generation)?;
    } else if generation.exists() {
        std::fs::remove_file(generation)?;
    }
    std::fs::rename(aside, generation)
}

/// A key for record `index` under `seed`: each seed is its own run, so no two
/// seeds' journals could be confused for one another.
fn key(seed: u64, index: u64) -> Result<EffectKey, EffectKeyError> {
    let hex = |salt: u64| format!("{seed:016x}{salt:016x}");
    let tagged = |salt: u64| format!("{}{}", hex(salt), hex(salt));
    let identity = EffectIdentity::new(
        RunId::from_hex(&hex(1))?,
        EnvironmentId::from_hex(&hex(2))?,
        FlowRevision::from_tagged("blake3_256", &tagged(3))?,
    );
    let action = ActionId::from_hex(&hex(index.saturating_add(16)))?;
    let digest = ActionDigest::from_tagged("blake3_256", &tagged(4))?;
    let attempt = AttemptId::from_decimal("1")?;
    let epoch = EnvironmentEpoch::from_decimal("1")?;
    Ok(identity.key(action, attempt, digest, epoch))
}

/// Why a seeded walk could not finish: each arm names the layer that refused.
#[derive(Debug)]
enum WalkError {
    /// The journal under test refused.
    Journal(JournalError),
    /// The scratch device refused.
    Io(std::io::Error),
    /// The draw stream refused an empty range.
    Draw(String),
    /// A continuing journal declined to continue.
    NotContinued,
}

impl From<JournalError> for WalkError {
    fn from(cause: JournalError) -> Self {
        Self::Journal(cause)
    }
}

impl From<std::io::Error> for WalkError {
    fn from(cause: std::io::Error) -> Self {
        Self::Io(cause)
    }
}

impl From<String> for WalkError {
    fn from(cause: String) -> Self {
        Self::Draw(cause)
    }
}

impl std::fmt::Display for WalkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Journal(ref cause) => write!(f, "the journal refused: {cause}"),
            Self::Io(ref cause) => write!(f, "the scratch device refused: {cause}"),
            Self::Draw(ref cause) => write!(f, "the draw refused: {cause}"),
            Self::NotContinued => f.write_str("a continuing journal did not continue"),
        }
    }
}

impl std::error::Error for WalkError {}

/// The low byte of `index`: every index here is far below 256.
fn low_byte(index: usize) -> u8 {
    let [low, ..] = index.to_le_bytes();
    low
}

/// Every record `journal` replays, in order.
fn records(journal: &RecordJournal<Vec<u8>>) -> Result<Vec<Vec<u8>>, JournalError> {
    let mut held = Vec::new();
    for entry in journal.replay()? {
        held.push(entry?.record().clone());
    }
    Ok(held)
}

/// Whether `answer` is the device's refusal rather than an open.
fn is_storage<T>(answer: &Result<T, JournalError>) -> bool {
    matches!(*answer, Err(JournalError::Storage(_)))
}

/// Catches a record journal that reads a refused successor as absent and rolls
/// back an acknowledged seal: the open is refused, the base is untouched, and
/// once the fault clears the successor still holds every acknowledged record.
#[test]
fn sim_successor_pulled_plug_a_refused_record_successor_is_not_absent() -> Outcome {
    let mut receipts = Vec::new();
    for seed in SEEDS {
        for fault in FAULTS {
            let first = record_walk(seed, fault)?;
            assert_eq!(
                first,
                record_walk(seed, fault)?,
                "seed {seed:#x} {fault:?} replays"
            );
            receipts.push(first);
        }
    }
    assert_ne!(
        receipts.first(),
        receipts.get(FAULTS.len()),
        "two seeds walked alike"
    );
    Ok(())
}

/// One seeded record-journal history sealed, broken by `fault`, opened, and
/// (unless the successor is gone) restored; the receipt folds what was held.
fn record_walk(seed: u64, fault: Fault) -> Result<u64, WalkError> {
    let dir = TempDir::new("successor-records")?;
    let path = dir.path().join("journal");
    let policy = ContinuationPolicy::declared();
    let mut stream = Stream::new(seed);
    let mut journal = RecordJournal::<Vec<u8>>::open_continuing(&path, policy)?;
    let mut before = Vec::new();
    for index in 0..stream.below(6)?.saturating_add(1) {
        let record = vec![low_byte(index); stream.below(64)?];
        journal.compare_and_append(journal.tail(), &record)?;
        before.push(record);
    }
    let checkpoint = vec![0xC0; stream.below(32)?];
    let mut successor = journal
        .continue_with(&checkpoint)?
        .ok_or(WalkError::NotContinued)?;
    let mut after = vec![checkpoint];
    for index in 0..stream.below(4)? {
        let record = vec![0xA0_u8.saturating_add(low_byte(index)); 9];
        successor.compare_and_append(successor.tail(), &record)?;
        after.push(record);
    }
    drop((journal, successor));
    let sealed = std::fs::read(&path)?;
    let next = successor_path(&path);
    let aside = break_successor(&next, fault)?;
    let opened = RecordJournal::<Vec<u8>>::open(&path);
    let held = if fault == Fault::Gone {
        let held = records(&opened?)?;
        assert_eq!(
            held, before,
            "seed {seed:#x}: a gone successor takes the seal back"
        );
        held
    } else {
        assert!(is_storage(&opened), "seed {seed:#x} {fault:?}: {opened:?}");
        drop(opened);
        assert_eq!(
            std::fs::read(&path)?,
            sealed,
            "seed {seed:#x}: the base is untouched"
        );
        restore_successor(&next, &aside)?;
        let refused = RecordJournal::<Vec<u8>>::open(&path);
        assert!(
            matches!(refused, Err(JournalError::Superseded { .. })),
            "seed {seed:#x}"
        );
        drop(refused);
        let held = records(&RecordJournal::<Vec<u8>>::open_active(&path, policy)?)?;
        assert_eq!(
            held, after,
            "seed {seed:#x}: the successor kept its history"
        );
        held
    };
    let mut receipt = fold(RECEIPT_START, format!("{fault:?}").as_bytes());
    for record in &held {
        receipt = fold(receipt, record);
    }
    Ok(receipt)
}

/// Catches a file journal that opens its sealed base as live when the device
/// refuses the successor: the open is refused, the base is untouched, and once
/// the fault clears the base is superseded and the successor holds its events.
#[test]
fn sim_successor_saboteur_a_refused_file_successor_is_not_absent() -> Outcome {
    for seed in SEEDS {
        let dir = TempDir::new("successor-file")?;
        let path = dir.path().join("run.jrnl");
        let mut stream = Stream::new(seed);
        let mut journal = FileJournal::open_continuing_with(&path, ContinuationPolicy::declared())?;
        let held_before = u64::try_from(stream.below(5)?)?.saturating_add(1);
        for index in 0..held_before {
            let event = EffectEvent::IntentAdmitted {
                key: key(seed, index)?,
            };
            journal.compare_and_append(journal.tail(), &event)?;
        }
        let mut successor = journal.continue_as_file()?.ok_or("it continues")?;
        let mut after = Vec::new();
        for index in 0..u64::try_from(stream.below(4)?)? {
            let event = EffectEvent::IntentAdmitted {
                key: key(seed, held_before.saturating_add(index))?,
            };
            successor.compare_and_append(successor.tail(), &event)?;
            after.push(event);
        }
        let successor_file = successor.path().to_path_buf();
        drop((journal, successor));
        let sealed = std::fs::read(&path)?;
        let next = successor_path(&path);
        assert_eq!(next, successor_file, "seed {seed:#x}: one successor name");
        for fault in [Fault::Loop, Fault::NotADirectory] {
            let aside = break_successor(&next, fault)?;
            let opened = FileJournal::open(&path);
            assert!(is_storage(&opened), "seed {seed:#x} {fault:?}: {opened:?}");
            drop(opened);
            assert_eq!(
                std::fs::read(&path)?,
                sealed,
                "seed {seed:#x}: the base is untouched"
            );
            restore_successor(&next, &aside)?;
        }
        let refused = FileJournal::open(&path);
        assert!(
            matches!(refused, Err(JournalError::Superseded { .. })),
            "seed {seed:#x}"
        );
        drop(refused);
        let live = FileJournal::open(&next)?;
        let own: Vec<EffectEvent> = live
            .committed()?
            .into_iter()
            .filter(|event| after.contains(event))
            .collect();
        assert_eq!(own, after, "seed {seed:#x}: the successor kept its events");
    }
    Ok(())
}
