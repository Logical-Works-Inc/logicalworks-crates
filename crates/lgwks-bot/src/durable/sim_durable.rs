//! Fools against the durable replace, each checked against a reference model.
//!
//! The model is a map from file name to the bytes last replaced under it. After
//! every walk the directory must hold exactly the model's names with exactly the
//! model's bytes: a scratch file left behind, two names sharing one scratch, or
//! an old document lost to a failed write is the bug.
//!
//! Sources: the POSIX `rename(2)` atomicity guarantee and its missing durability
//! one (the ext4 delayed-allocation zero-length-file reports, 2009), and the
//! write, `fsync`, rename, directory `fsync` order SQLite and PostgreSQL use.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use super::{SCRATCH_SUFFIX, parent_of, replace_durably, scratch_path, sync_directory};
use crate::sim_stream::{RECEIPT_START, Stream, TempDir, fold};

/// A test's outcome.
type Outcome = Result<(), Box<dyn std::error::Error>>;

/// The seeds every walk replays.
const SEEDS: [u64; 3] = [0xD0_0001, 0xD0_0002, 0xD0_FFFF];

/// Name stems the fools draw: extensions that collide under a naive
/// `with_extension`, a name that already ends in `.tmp`, unicode, dots.
const NAMES: [&str; 9] = [
    "x.json",
    "x.db",
    "x",
    "x.tmp",
    "x.json.tmp.json",
    ".hidden",
    "tenant.snapshot",
    "naïve résumé.json",
    "a.b.c",
];

/// The directory as the model sees it: every name and its bytes.
fn listing(dir: &Path) -> io::Result<BTreeMap<String, Vec<u8>>> {
    let mut held = BTreeMap::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        held.insert(name, std::fs::read(entry.path())?);
    }
    Ok(held)
}

/// A body of a drawn size: empty, one byte, under 4 KiB, or a megabyte, all
/// from one stream word, so the draw cannot fail.
fn body(stream: &mut Stream) -> Vec<u8> {
    let [shape, byte, small_low, small_high, ..] = stream.next().to_le_bytes();
    let length = match shape & 3 {
        0 => 0,
        1 => 1,
        2 => usize::from(u16::from_le_bytes([small_low, small_high & 0x0F])),
        _ => 1 << 20,
    };
    vec![byte; length]
}

/// Catches a scratch name shared by two documents, a scratch left behind, and a
/// replace that keeps an older body: after a seeded walk the directory is the
/// model, byte for byte, and the walk's receipt replays.
#[test]
fn sim_durable_double_clicker_and_neighbour_every_name_holds_its_last_body() -> Outcome {
    let mut receipts = Vec::new();
    for seed in SEEDS {
        let mut twice = Vec::new();
        for _ in 0..2 {
            let scratch = TempDir::new("durable-walk")?;
            let mut stream = Stream::new(seed);
            let mut model = BTreeMap::new();
            let mut receipt = RECEIPT_START;
            for _ in 0..24 {
                let name = stream.choose(&NAMES)?;
                let bytes = body(&mut stream);
                replace_durably(&scratch.path().join(name), &bytes)?;
                receipt = fold(receipt, name.as_bytes());
                receipt = fold(receipt, &bytes);
                model.insert(name.to_owned(), bytes);
            }
            let held = listing(scratch.path())?;
            assert_eq!(held, model, "seed {seed:#x}: the directory is the model");
            twice.push(receipt);
        }
        assert_eq!(twice.first(), twice.get(1), "seed {seed:#x} replays");
        receipts.push(twice.first().copied());
    }
    assert_ne!(receipts.first(), receipts.get(1), "two seeds walked alike");
    Ok(())
}

/// Catches a crash that loses the old document: a scratch left by a replace
/// that died is overwritten, and a replace that cannot write its scratch leaves
/// the old document whole and the error the device's.
#[test]
fn sim_durable_pulled_plug_a_failed_replace_keeps_the_old_document() -> Outcome {
    let scratch = TempDir::new("durable-plug")?;
    let mut stream = Stream::new(SEEDS[0]);
    for name in NAMES {
        let path = scratch.path().join(name);
        let old = body(&mut stream);
        replace_durably(&path, &old)?;
        // A replace that died after its scratch write, before the rename.
        let leftover = scratch_path(&path)?;
        std::fs::write(&leftover, b"half a document")?;
        let new = body(&mut stream);
        replace_durably(&path, &new)?;
        assert_eq!(std::fs::read(&path)?, new, "{name}: the new document");
        assert!(!leftover.exists(), "{name}: the stale scratch is consumed");
        // A scratch name that cannot be a file: the write fails before the rename.
        std::fs::create_dir(&leftover)?;
        let refused = replace_durably(&path, b"never lands");
        assert!(
            refused.is_err(),
            "{name}: a replace that cannot write is refused"
        );
        assert_eq!(
            std::fs::read(&path)?,
            new,
            "{name}: the old document stays whole"
        );
        std::fs::remove_dir(&leftover)?;
    }
    Ok(())
}

/// Catches a path that names no file reaching the device, and a bare relative
/// name syncing the empty path: the first is `InvalidInput` with nothing
/// written, the second syncs the current directory.
#[test]
fn sim_durable_fat_finger_a_path_that_names_no_file_is_refused() -> Outcome {
    for path in ["", "/", "..", "a/.."] {
        let refused = replace_durably(Path::new(path), b"x");
        assert_eq!(
            refused.as_ref().err().map(io::Error::kind),
            Some(io::ErrorKind::InvalidInput),
            "{path:?} names no file: {refused:?}"
        );
    }
    assert_eq!(parent_of(Path::new("x.json")), Path::new("."));
    assert_eq!(parent_of(Path::new("dir/x.json")), Path::new("dir"));
    let scratch = scratch_path(Path::new("x.json"))?;
    assert_eq!(scratch, PathBuf::from(format!("x.json{SCRATCH_SUFFIX}")));
    let nested = scratch_path(Path::new("d/x.db"))?;
    assert_eq!(nested, PathBuf::from(format!("d/x.db{SCRATCH_SUFFIX}")));
    sync_directory(Path::new("."))?;
    Ok(())
}

/// The saboteur, after the others: a missing directory is the device's error
/// and creates nothing, so no fool's walk can leave a partial state behind.
#[test]
fn sim_durable_saboteur_a_missing_directory_writes_nothing() -> Outcome {
    let scratch = TempDir::new("durable-saboteur")?;
    let missing = scratch.path().join("absent").join("x.json");
    let refused = replace_durably(&missing, b"x");
    assert_eq!(
        refused.as_ref().err().map(io::Error::kind),
        Some(io::ErrorKind::NotFound),
        "the device's own answer: {refused:?}"
    );
    assert!(listing(scratch.path())?.is_empty(), "nothing was created");
    Ok(())
}
