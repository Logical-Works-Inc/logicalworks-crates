//! Fools against the record journal, each with a reference model for an oracle.
//!
//! The model is a plain `Vec` of what was acknowledged: a reopen, a page or a
//! replay answering anything else is the bug. The chain head at the tail commits
//! to the whole history, so it is also each run's trace hash.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use lgwks_std::hash::Digest;

use super::super::continuation::{ContinuationPolicy, SealPause, successor_path};
use super::super::{CorruptionKind, JournalError, JournalLimitKind, MAX_JOURNAL_EVENTS};
use super::RecordJournal;

/// A test's outcome: its setup failures propagate instead of panicking.
pub(super) type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A consumer's record, shaped like the commands and events of a real one.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    lgwks_std::wire::Archive,
    lgwks_std::wire::Serialize,
    lgwks_std::wire::Deserialize,
)]
#[rkyv(crate = lgwks_std::wire::rkyv)]
pub(super) struct Fact {
    /// The run it is about.
    pub(super) run: u64,
    /// The consumer's own kind code.
    pub(super) kind: u8,
    /// The rest of the record.
    pub(super) note: Vec<u8>,
}

/// A record with `note` as its bytes.
pub(super) fn fact(run: u64, kind: u8, note: &[u8]) -> Fact {
    Fact {
        run,
        kind,
        note: note.to_vec(),
    }
}

/// One seed's whole randomness.
pub(super) struct Dice(u64);

impl Dice {
    /// Seeded dice; xorshift cannot leave zero, so zero is moved off it.
    pub(super) fn new(seed: u64) -> Self {
        Self(seed.saturating_mul(2).saturating_add(0x9E37_79B9))
    }

    /// The next value below `bound`.
    pub(super) fn below(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 << 13_u32;
        self.0 ^= self.0 >> 7_u32;
        self.0 ^= self.0 << 17_u32;
        let Some(value) = self.0.checked_rem(bound) else {
            return 0;
        };
        value
    }

    /// A record whose size and contents come from this seed.
    pub(super) fn record(&mut self, largest: u64) -> Outcome<Fact> {
        let length = usize::try_from(self.below(largest))?;
        let mut note = Vec::with_capacity(length);
        for _ in 0..length {
            note.push(u8::try_from(self.below(256))?);
        }
        Ok(Fact {
            run: self.below(8),
            kind: u8::try_from(self.below(8))?,
            note,
        })
    }
}

/// A directory of this test's own, gone when the test is.
pub(super) struct Scratch(PathBuf);

impl Scratch {
    /// A fresh directory named for this test, unique per call.
    pub(super) fn new(name: &str) -> Outcome<Self> {
        let unique = lgwks_std::id::Uuid::new_v4()?;
        let dir = std::env::temp_dir().join(format!("lgwks-record-sim-{name}-{unique}"));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    /// A file in this directory.
    pub(super) fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(cause) = std::fs::remove_dir_all(&self.0) {
            lgwks_std::trace::warn!(error = %cause, "sim_records: the scratch directory stayed");
        }
    }
}

/// Every record this journal replays, in order.
pub(super) fn held(journal: &RecordJournal<Fact>) -> Outcome<Vec<Fact>> {
    let mut records = Vec::new();
    for entry in journal.replay()? {
        records.push(entry?.record().clone());
    }
    Ok(records)
}

/// Run `body` twice per seed and require the same trace hash from both.
pub(super) fn replay(
    seeds: core::ops::Range<u64>,
    mut body: impl FnMut(u64) -> Outcome<Digest>,
) -> Outcome {
    for seed in seeds {
        let first = body(seed)?;
        let second = body(seed)?;
        assert_eq!(first, second, "seed {seed} replayed a different trace");
    }
    Ok(())
}

/// A journal of `count` seeded records, closed, with what it was acknowledged to hold.
fn seeded_file(path: &Path, seed: u64, count: u64, largest: u64) -> Outcome<(Vec<Fact>, Vec<u64>)> {
    let mut dice = Dice::new(seed);
    let mut journal = RecordJournal::<Fact>::open(path)?;
    let mut model = Vec::new();
    for _ in 0..count {
        let record = dice.record(largest)?;
        journal.compare_and_append(journal.tail(), &record)?;
        model.push(record);
    }
    Ok((model, journal.offsets.clone()))
}

/// Catches a repair that eats an acknowledged record, or keeps half of one: a cut
/// at every byte of the file leaves exactly the whole frames before it.
#[test]
fn sim_records_pulled_plug_every_cut_keeps_every_whole_frame() -> Outcome {
    let scratch = Scratch::new("cut")?;
    let source = scratch.file("source");
    let (model, offsets) = seeded_file(&source, 11, 6, 24)?;
    let bytes = std::fs::read(&source)?;
    let copy = scratch.file("copy");
    for cut in 0..=bytes.len() {
        std::fs::write(&copy, bytes.get(..cut).ok_or("cut past the end")?)?;
        let at = u64::try_from(cut)?;
        let whole = offsets.iter().filter(|offset| **offset < at).count();
        let boundary = at == u64::try_from(bytes.len())? || offsets.contains(&at);
        let survivors = if boundary {
            whole
        } else {
            whole.saturating_sub(1)
        };
        let journal = RecordJournal::<Fact>::open(&copy)?;
        assert_eq!(journal.torn_tail_repaired(), !boundary, "cut {cut}");
        assert_eq!(
            held(&journal)?,
            model.get(..survivors).ok_or("model")?,
            "cut {cut}"
        );
    }
    Ok(())
}

/// Catches a chain that trusts a stored byte: any one changed byte in a committed
/// file is refused, the refused file is left as it was, and nothing is trimmed.
#[test]
fn sim_records_weird_bytes_any_flipped_byte_is_refused_and_untouched() -> Outcome {
    let scratch = Scratch::new("flip")?;
    let source = scratch.file("source");
    seeded_file(&source, 12, 5, 16)?;
    let bytes = std::fs::read(&source)?;
    let copy = scratch.file("copy");
    let mut dice = Dice::new(12);
    for index in 0..bytes.len() {
        let mut flipped = bytes.clone();
        let target = flipped.get_mut(index).ok_or("flip past the end")?;
        *target ^= u8::try_from(dice.below(255))?.saturating_add(1);
        std::fs::write(&copy, &flipped)?;
        let answer = RecordJournal::<Fact>::open(&copy);
        assert!(
            matches!(answer, Err(JournalError::Corrupt(_))),
            "byte {index} was not refused: {answer:?}"
        );
        drop(answer);
        assert_eq!(std::fs::read(&copy)?, flipped, "byte {index} was rewritten");
    }
    Ok(())
}

/// Catches a fence that lets a retry land on a tail it never saw.
#[test]
fn sim_records_double_clicker_a_stale_tail_never_appends() -> Outcome {
    let scratch = Scratch::new("stale")?;
    let path = scratch.file("journal");
    let mut journal = RecordJournal::<Fact>::open(&path)?;
    let stale = journal.tail();
    journal.compare_and_append(stale, &fact(1, 1, b"first"))?;
    let length = std::fs::metadata(&path)?.len();
    for _ in 0..3 {
        let refused = journal.compare_and_append(stale, &fact(1, 1, b"first"));
        assert!(matches!(refused, Err(JournalError::TailMismatch { .. })));
        let batch = journal.compare_and_append_all(stale, &[fact(1, 2, b"")]);
        assert!(matches!(batch, Err(JournalError::TailMismatch { .. })));
        let empty = journal.compare_and_append_all(stale, &[]);
        assert!(matches!(empty, Err(JournalError::TailMismatch { .. })));
    }
    assert_eq!(std::fs::metadata(&path)?.len(), length);
    assert_eq!(journal.committed(), 1);
    Ok(())
}

/// Catches a shared lock, and a tenant reading through its neighbour's file.
#[test]
fn sim_records_neighbour_a_second_writer_and_a_second_tenant_stay_out() -> Outcome {
    let scratch = Scratch::new("neighbour")?;
    let (left, right) = (scratch.file("tenant-a"), scratch.file("tenant-b"));
    let mut tenant_a = RecordJournal::<Fact>::open(&left)?;
    let mut tenant_b = RecordJournal::<Fact>::open(&right)?;
    let second = RecordJournal::<Fact>::open(&left);
    assert!(matches!(second, Err(JournalError::Locked { .. })));
    let mut dice = Dice::new(14);
    let (mut model_a, mut model_b) = (Vec::new(), Vec::new());
    for _ in 0..64 {
        let record = dice.record(32)?;
        if dice.below(2) == 0 {
            tenant_a.compare_and_append(tenant_a.tail(), &record)?;
            model_a.push(record);
        } else {
            tenant_b.compare_and_append(tenant_b.tail(), &record)?;
            model_b.push(record);
        }
    }
    assert_eq!(held(&tenant_a)?, model_a);
    assert_eq!(held(&tenant_b)?, model_b);
    Ok(())
}

/// Catches a ceiling that is only documented: each bound refuses before a byte
/// moves, the refusal names its resource, and the journal still reopens whole.
#[test]
fn sim_records_giant_and_empty_every_ceiling_binds_and_recovers() -> Outcome {
    let scratch = Scratch::new("giant")?;
    let path = scratch.file("events");
    let mut journal = RecordJournal::<u64>::open(&path)?;
    let batch: Vec<u64> = (0..1000).collect();
    for _ in 0..MAX_JOURNAL_EVENTS.saturating_div(batch.len()) {
        journal.compare_and_append_all(journal.tail(), &batch)?;
    }
    let refused = journal.compare_and_append(journal.tail(), &0);
    assert!(matches!(
        refused,
        Err(JournalError::CapacityExceeded {
            resource: JournalLimitKind::Events,
            ..
        })
    ));
    drop(journal);
    assert_eq!(
        RecordJournal::<u64>::open(&path)?.committed(),
        MAX_JOURNAL_EVENTS
    );

    let path = scratch.file("frame");
    let mut journal = RecordJournal::<Fact>::open(&path)?;
    journal.compare_and_append(journal.tail(), &fact(0, 0, b""))?;
    let length = std::fs::metadata(&path)?.len();
    let refused = journal.compare_and_append(journal.tail(), &fact(0, 0, &vec![0; 64 * 1024]));
    assert!(matches!(
        refused,
        Err(JournalError::CapacityExceeded {
            resource: JournalLimitKind::FrameBytes,
            ..
        })
    ));
    assert_eq!(std::fs::metadata(&path)?.len(), length);

    // A continuing journal fills to its ceiling and is still able to seal.
    let path = scratch.file("bytes");
    let policy = ContinuationPolicy::declared();
    let mut journal = RecordJournal::<Fact>::open_continuing(&path, policy)?;
    let large = fact(0, 0, &vec![7; 60_000]);
    loop {
        match journal.compare_and_append_all(journal.tail(), core::slice::from_ref(&large)) {
            Ok(_) => {}
            Err(JournalError::CapacityExceeded {
                resource: JournalLimitKind::Bytes,
                ..
            }) => break,
            Err(other) => return Err(other.into()),
        }
    }
    let successor = journal.continue_with(&large)?.ok_or("it continues")?;
    assert_eq!(successor.committed(), 1);
    Ok(())
}

/// Catches two authorities, or none: a seal stopped at each durable boundary and
/// killed resolves to exactly one writable journal that keeps its history.
#[test]
fn sim_records_pulled_plug_each_seal_boundary_leaves_one_authority() -> Outcome {
    for boundary in SealPause::all() {
        let scratch = Scratch::new("seal")?;
        let path = scratch.file("journal");
        let policy = ContinuationPolicy::declared();
        let mut journal = RecordJournal::<Fact>::open_continuing(&path, policy)?;
        journal.compare_and_append(journal.tail(), &fact(1, 1, b"kept"))?;
        let before = journal.tail();
        journal.arm_seal_pause(boundary);
        let paused = journal.continue_with(&fact(9, 9, b"checkpoint"));
        assert!(matches!(
            paused,
            Err(JournalError::ContinuationPaused { .. })
        ));
        let sealed = journal.compare_and_append(journal.tail(), &fact(1, 2, b"late"));
        assert!(matches!(sealed, Err(JournalError::Superseded { .. })));
        drop(journal);
        let mut live = RecordJournal::<Fact>::open_active(&path, policy)?;
        if matches!(boundary, SealPause::AfterSealWrite) {
            assert_eq!(live.tail(), before, "an unacknowledged seal is taken back");
            assert_eq!(held(&live)?, vec![fact(1, 1, b"kept")]);
        } else {
            assert_eq!(live.first_sequence(), before.sequence().saturating_add(1));
            assert_eq!(held(&live)?, vec![fact(9, 9, b"checkpoint")]);
            assert!(matches!(
                RecordJournal::<Fact>::open(&path),
                Err(JournalError::Superseded { .. })
            ));
        }
        let ack = live.compare_and_append(live.tail(), &fact(2, 1, b"after"))?;
        drop(live);
        let again = RecordJournal::<Fact>::open_active(&path, policy)?;
        assert_eq!(again.tail(), ack.position(), "{boundary:?} moved on reopen");
    }
    Ok(())
}

/// Catches a seal recognised by its look, a gap hidden from a cursor, and a file
/// that claims a generation its name or its first frame denies.
#[test]
fn sim_records_wrong_order_seals_cursors_and_generations_stay_honest() -> Outcome {
    let scratch = Scratch::new("order")?;
    let path = scratch.file("journal");
    let policy = ContinuationPolicy::declared();
    let mut journal = RecordJournal::<Vec<u8>>::open_continuing(&path, policy)?;
    let lookalike = {
        let mut bytes = vec![super::SEAL_TAG];
        bytes.extend_from_slice(journal.tail().head().as_bytes());
        bytes
    };
    journal.compare_and_append(journal.tail(), &lookalike)?;
    journal.compare_and_append(journal.tail(), &vec![])?;
    drop(journal);
    let mut journal = RecordJournal::<Vec<u8>>::open_continuing(&path, policy)?;
    assert_eq!(
        journal.committed(),
        2,
        "a record that looks like a seal is one"
    );
    assert!(!journal.torn_tail_repaired());
    let page = journal.page(0, 1)?;
    assert_eq!(page.first().map(|entry| entry.record()), Some(&lookalike));

    let last = journal.tail().sequence();
    let successor = journal.continue_with(&vec![1])?.ok_or("it continues")?;
    assert!(matches!(
        journal.compare_and_append(journal.tail(), &vec![2]),
        Err(JournalError::Superseded { .. })
    ));
    assert!(matches!(
        successor.page(0, 8),
        Err(JournalError::CursorBehindGeneration { .. })
    ));
    let carried = successor.page(last, 8)?;
    assert_eq!(
        carried.first().map(|entry| entry.is_checkpoint()),
        Some(true)
    );
    assert!(successor.page(last.saturating_add(8), 8)?.is_empty());
    let predecessor_page = journal.page(0, 8)?;
    assert_eq!(predecessor_page.len(), 2, "its own seal is not an entry");

    // A base name holding a carried seal, and a generation name holding none.
    let bytes = std::fs::read(successor_path(&path))?;
    let forged = scratch.file("forged-base");
    std::fs::write(&forged, &bytes)?;
    assert!(matches!(
        RecordJournal::<Vec<u8>>::open(&forged),
        Err(JournalError::Corrupt(_))
    ));
    drop(successor);
    let empty_generation = successor_path(&forged);
    if let Some(parent) = empty_generation.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&empty_generation, b"")?;
    assert!(matches!(
        RecordJournal::<Vec<u8>>::open(&empty_generation),
        Err(JournalError::Corrupt(_))
    ));
    Ok(())
}

/// Catches a seal read as a predecessor's because it is the first frame: an empty
/// journal that seals has no record before it, and reopening must not call it corrupt.
#[test]
fn sim_records_giant_and_empty_an_empty_journal_seals_and_reopens() -> Outcome {
    let scratch = Scratch::new("empty-seal")?;
    let path = scratch.file("journal");
    let policy = ContinuationPolicy::declared();
    let mut journal = RecordJournal::<Fact>::open_continuing(&path, policy)?;
    let successor = journal
        .continue_with(&fact(1, 1, b"first"))?
        .ok_or("it continues")?;
    let tail = successor.tail();
    drop((journal, successor));
    assert!(matches!(
        RecordJournal::<Fact>::open(&path),
        Err(JournalError::Superseded { .. })
    ));
    assert_eq!(
        RecordJournal::<Fact>::open_active(&path, policy)?.tail(),
        tail
    );

    // Paused before the successor exists, the same seal is taken back whole.
    let path = scratch.file("paused");
    let mut journal = RecordJournal::<Fact>::open_continuing(&path, policy)?;
    journal.arm_seal_pause(SealPause::AfterSealWrite);
    assert!(journal.continue_with(&fact(1, 1, b"first")).is_err());
    drop(journal);
    let reopened = RecordJournal::<Fact>::open_active(&path, policy)?;
    assert_eq!(reopened.committed(), 0);
    assert!(reopened.torn_tail_repaired());
    Ok(())
}

/// Catches a model and a journal drifting apart under every door in any order: the
/// same seed takes the same walk to the same head.
#[test]
fn sim_records_monkey_random_ops_match_the_model_and_replay_exactly() -> Outcome {
    replay(0..16, |seed| {
        let scratch = Scratch::new("monkey")?;
        let path = scratch.file("journal");
        let policy = ContinuationPolicy::declared();
        let mut dice = Dice::new(seed);
        let mut journal = RecordJournal::<Fact>::open_active(&path, policy)?;
        let mut model = Vec::new();
        for _ in 0..200 {
            match dice.below(8) {
                0 | 1 => {
                    let record = dice.record(64)?;
                    journal.compare_and_append(journal.tail(), &record)?;
                    model.push(record);
                }
                2 => {
                    let record = dice.record(64)?;
                    let tail = journal.tail();
                    let ack =
                        lgwks_std::task::block_on(journal.compare_and_append_async(tail, &record))?;
                    assert_eq!(ack.position(), journal.tail());
                    model.push(record);
                }
                3 => {
                    let batch = vec![dice.record(16)?, dice.record(16)?, dice.record(16)?];
                    journal.compare_and_append_all(journal.tail(), &batch)?;
                    model.extend(batch);
                }
                4 => {
                    let tail = journal.tail();
                    drop(journal);
                    journal = RecordJournal::<Fact>::open_active(&path, policy)?;
                    assert_eq!(journal.tail(), tail, "a clean reopen moved the tail");
                }
                5 => {
                    let tail = journal.tail();
                    let live = journal.path.clone();
                    drop(journal);
                    let mut file = std::fs::OpenOptions::new().append(true).open(&live)?;
                    file.write_all(&[0, 0])?;
                    drop(file);
                    journal = RecordJournal::<Fact>::open_active(&path, policy)?;
                    assert_eq!(journal.tail(), tail, "a torn tail cost a whole frame");
                    assert!(journal.torn_tail_repaired());
                }
                6 => {
                    let record = dice.record(64)?;
                    journal = journal.continue_with(&record)?.ok_or("it continues")?;
                    model = vec![record];
                }
                _ => {
                    let limit = usize::try_from(dice.below(16))?;
                    let after = journal.first_sequence().saturating_sub(1);
                    let page = journal.page(after, limit)?;
                    let bound = limit.min(model.len());
                    let want = model.get(..bound).ok_or("model")?;
                    let got: Vec<&Fact> = page.iter().map(|entry| entry.record()).collect();
                    assert_eq!(got, want.iter().collect::<Vec<_>>());
                }
            }
        }
        assert_eq!(held(&journal)?, model);
        assert_eq!(
            journal
                .first_sequence()
                .saturating_add(u64::try_from(model.len())?),
            journal.tail().sequence().saturating_add(1),
            "a sequence fell out of the chain"
        );
        Ok(journal.tail().head())
    })
}

/// Catches a hand: a forged frame, a foreign successor, a lying length and a device
/// that fails mid-append are each refused or settled, never believed.
#[test]
fn sim_records_saboteur_forged_bytes_and_a_failing_device_are_not_believed() -> Outcome {
    let scratch = Scratch::new("saboteur")?;
    let path = scratch.file("forged");
    seeded_file(&path, 21, 3, 16)?;
    let mut bytes = std::fs::read(&path)?;
    let tail_frame = bytes.split_off(bytes.len().saturating_sub(48));
    std::fs::write(
        &path,
        [
            bytes.as_slice(),
            tail_frame.as_slice(),
            tail_frame.as_slice(),
        ]
        .concat(),
    )?;
    assert!(matches!(
        RecordJournal::<Fact>::open(&path),
        Err(JournalError::Corrupt(_))
    ));

    let path = scratch.file("lying");
    seeded_file(&path, 22, 3, 16)?;
    let mut bytes = std::fs::read(&path)?;
    let last = RecordJournal::<Fact>::open(&path)?
        .offsets
        .last()
        .copied()
        .ok_or("frames")?;
    let prefix = usize::try_from(last)?.saturating_add(3);
    let low = bytes.get_mut(prefix).ok_or("prefix")?;
    *low = low.saturating_add(9);
    std::fs::write(&path, &bytes)?;
    let lied = RecordJournal::<Fact>::open(&path);
    match lied {
        Err(JournalError::Corrupt(ref held)) => {
            assert_eq!(*held.kind(), CorruptionKind::Framed);
        }
        ref other => assert!(other.is_err(), "a lying length was believed: {other:?}"),
    }

    // A foreign file at the successor's name never decides the authority.
    let path = scratch.file("foreign");
    let policy = ContinuationPolicy::declared();
    let mut journal = RecordJournal::<Fact>::open_continuing(&path, policy)?;
    journal.compare_and_append(journal.tail(), &fact(1, 1, b"kept"))?;
    let kept = journal.tail();
    journal.arm_seal_pause(SealPause::AfterSealWrite);
    let paused = journal.continue_with(&fact(9, 9, b"checkpoint"));
    assert!(paused.is_err());
    drop(journal);
    let next = successor_path(&path);
    if let Some(parent) = next.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&next, b"somebody else's bytes")?;
    let live = RecordJournal::<Fact>::open_active(&path, policy)?;
    assert_eq!(live.tail(), kept);
    drop(live);

    // A device that fails leaves an answer only a reopen can settle.
    let path = scratch.file("device");
    let mut journal = RecordJournal::<Fact>::open(&path)?;
    journal.compare_and_append(journal.tail(), &fact(1, 1, b"kept"))?;
    let tail = journal.tail();
    journal.storage.fail_next_commit();
    let lost = journal.compare_and_append(tail, &fact(1, 2, b"unknown"));
    assert!(matches!(lost, Err(JournalError::OutcomeUnknown { .. })));
    let refused = journal.compare_and_append(tail, &fact(1, 3, b"after"));
    assert!(matches!(refused, Err(JournalError::Storage(_))));
    drop(journal);
    let settled = RecordJournal::<Fact>::open(&path)?;
    let records = held(&settled)?;
    assert_eq!(records.first(), Some(&fact(1, 1, b"kept")));
    assert!(records.len() <= 2, "only what was asked for can be there");
    Ok(())
}
