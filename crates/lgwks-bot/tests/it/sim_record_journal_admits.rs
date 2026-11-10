//! A consumer that stages records for a group commit asks before it stages.
//!
//! Source: a consumer's runtime that staged a record, folded it into its view, and
//! learned at the sync that the frame ceiling refused it, leaving a view the file did
//! not hold (#906). `RecordJournal::admits` runs what an append runs before the disk,
//! so its answer and the append's must agree on every record, at the ceiling and one
//! byte past it, and asking must write nothing.

use lgwks_bot::journal::{JournalError, RecordJournal};
use lgwks_std::seeded::Seeded;

use crate::sim::scratch_file::ScratchFile;

/// A test's outcome: setup failures propagate instead of panicking.
type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Whether `journal` admits a record of `len` bytes.
fn admits(journal: &RecordJournal<Vec<u8>>, len: usize) -> bool {
    journal.admits(&vec![0xA5; len]).is_ok()
}

/// The largest record, in bytes, that `journal` admits.
fn ceiling(journal: &RecordJournal<Vec<u8>>) -> Outcome<usize> {
    let (mut low, mut high) = (0_usize, 1_usize << 20);
    if !admits(journal, low) || admits(journal, high) {
        let refusal = Err("the ceiling is not between 0 and 1 MiB".into());
        lgwks_std::trace::debug!(low, high, "the ceiling probe found no bracket");
        return refusal;
    }
    while high.saturating_sub(low) > 1 {
        let middle = low.saturating_add(high.saturating_sub(low) >> 1_u32);
        if admits(journal, middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    Ok(low)
}

/// Catches an `admits` that disagrees with the append at the ceiling: the largest
/// record it admits appends, and one byte more is refused by both.
#[test]
fn sim_record_journal_admits_giant_and_empty_the_ceiling_is_the_appends() -> Outcome {
    let scratch = ScratchFile::new("record-admits-ceiling")?;
    let mut journal = RecordJournal::<Vec<u8>>::open(scratch.path())?;
    let largest = ceiling(&journal)?;
    assert_eq!(journal.committed(), 0, "asking wrote nothing");
    let over = vec![0xA5; largest.saturating_add(1)];
    assert!(matches!(
        journal.admits(&over),
        Err(JournalError::CapacityExceeded { .. })
    ));
    assert!(matches!(
        journal.compare_and_append(journal.tail(), &over),
        Err(JournalError::CapacityExceeded { .. })
    ));
    journal.compare_and_append(journal.tail(), &vec![0xA5; largest])?;
    journal.compare_and_append(journal.tail(), &Vec::new())?;
    assert_eq!(journal.committed(), 2);
    Ok(())
}

/// One seeded run: records of random sizes around the ceiling, each asked about and
/// then appended, so the two answers are compared on every record.
fn agreement(seed: u64, largest: usize) -> Outcome<Vec<bool>> {
    let mut dice = Seeded::from_seed(seed);
    let scratch = ScratchFile::new(&format!("record-admits-{seed}"))?;
    let mut journal = RecordJournal::<Vec<u8>>::open(scratch.path())?;
    let low = u64::try_from(largest.saturating_sub(64))?;
    let mut answers = Vec::with_capacity(32);
    for _ in 0..32 {
        let len = usize::try_from(low.saturating_add(dice.below(128)?))?;
        let record = vec![0x5A; len];
        let before = journal.committed();
        let asked = journal.admits(&record).is_ok();
        assert_eq!(
            journal.committed(),
            before,
            "seed {seed}: asking wrote nothing"
        );
        let appended = journal.compare_and_append(journal.tail(), &record).is_ok();
        assert_eq!(asked, appended, "seed {seed}: {len} bytes");
        assert_eq!(asked, len <= largest, "seed {seed}: {len} bytes");
        answers.push(asked);
    }
    Ok(answers)
}

/// Catches an `admits` that drifts from the append, or one that writes: across
/// seeds, the two agree on every record and each seed replays to one trace.
#[test]
fn sim_record_journal_admits_monkey_asking_and_appending_agree() -> Outcome {
    let scratch = ScratchFile::new("record-admits-probe")?;
    let largest = ceiling(&RecordJournal::<Vec<u8>>::open(scratch.path())?)?;
    let mut refused = 0_usize;
    for seed in 0..32_u64 {
        let first = agreement(seed, largest)?;
        assert_eq!(first, agreement(seed, largest)?, "seed {seed} replays");
        refused = refused.saturating_add(first.iter().filter(|asked| !**asked).count());
    }
    assert!(refused > 0, "some record was over the ceiling");
    Ok(())
}
