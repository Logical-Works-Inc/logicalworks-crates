//! The chain and the seal: what a successor is called and chains from, what the
//! seal writes, what a sealed or half-sealed predecessor refuses, and how a reopen
//! reaches the live generation.

use super::*;

/// Every successor is the next fixed-width name in its base's own directory, and
/// the file and its checkpoint name one generation.
pub(super) fn generation_names(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("names")?;
    let base = dir.join("run.jrnl");
    let continuations = draw(sim, 1, 5);
    let mut journal = quiet(&base)?;
    let mut oracle = Oracle::default();
    let mut next = 1_u64;
    for generation in 2..=continuations.saturating_add(1) {
        let count = draw(sim, 1, 6);
        write(&mut journal, &mut oracle, 1, (next, count), Fate::Applied)?;
        next = next.saturating_add(count);
        let predicted = journal.successor_path();
        journal = seal(&mut journal)?;
        let expected = dir
            .join("run.jrnl.cont")
            .join(format!("{:06}", generation.saturating_sub(1)));
        assert_eq!(
            journal.path(),
            predicted.as_path(),
            "the successor is where its predecessor said it would be"
        );
        assert_eq!(
            journal.path(),
            expected.as_path(),
            "a fixed-width number in the base's own `.cont` directory"
        );
        let carried = journal
            .checkpoint()
            .ok_or("a successor is opened from a checkpoint")?
            .generation();
        assert_eq!(
            (journal.generation(), carried),
            (generation, generation),
            "the file and the checkpoint it was opened from name one generation"
        );
    }
    assert_fold_agrees(&journal, &oracle, 1)?;
    sim.record("each-successor-is-the-next-name");
    sim.trace
        .record_number("names-continuations", continuations);
    sim.trace.record_number("names-attempts", next);
    Ok(())
}

/// A successor's checkpoint names the tail it sealed, and its own chain verifies
/// from there.
pub(super) fn chain_base(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("base")?;
    let path = dir.join("run.jrnl");
    let attempts = draw(sim, 2, 30);
    let mut journal = quiet(&path)?;
    let mut oracle = Oracle::default();
    write_drawn(sim, &mut journal, &mut oracle, 1, (1, attempts))?;
    let sealed_at = journal.tail();
    let mut successor = seal(&mut journal)?;
    let predecessor = successor
        .checkpoint()
        .ok_or("a successor is opened from a checkpoint")?
        .predecessor();
    assert_eq!(
        predecessor, sealed_at,
        "the checkpoint names the tail it sealed"
    );
    assert_eq!(
        successor.base(),
        sealed_at,
        "and the successor's chain starts there"
    );
    assert!(
        successor.committed()?.is_empty(),
        "a successor has admitted nothing of its own at birth"
    );
    let more = draw(sim, 1, 12);
    write_drawn(
        sim,
        &mut successor,
        &mut oracle,
        1,
        (attempts.saturating_add(1), more),
    )?;
    let entries = successor.committed_entries()?;
    assert_eq!(
        verify_chain_from(successor.chain_from(), &entries)?,
        successor.tail(),
        "the successor's own chain verifies from its own base to its tail"
    );
    assert_fold_agrees(&successor, &oracle, 1)?;
    sim.record("successor-chains-from-the-sealed-tail");
    sim.trace.record_number("base-attempts", attempts);
    sim.trace.record_number("base-more", more);
    Ok(())
}

/// A reopen through the base path reaches the live generation, at its tail, with
/// the fold the oracle recorded — after any number of continuations, none included.
pub(super) fn reopen_reaches_live(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("walk")?;
    let base = dir.join("run.jrnl");
    let continuations = u64::from(sim.rng().below(6));
    let mut journal = quiet(&base)?;
    let mut oracle = Oracle::default();
    let mut next = 1_u64;
    for _ in 0..continuations {
        let count = draw(sim, 1, 10);
        write_drawn(sim, &mut journal, &mut oracle, 1, (next, count))?;
        next = next.saturating_add(count);
        journal = seal(&mut journal)?;
    }
    let trailing = u64::from(sim.rng().below(10));
    write_drawn(sim, &mut journal, &mut oracle, 1, (next, trailing))?;
    let live = journal.path().to_path_buf();
    let generation = journal.generation();
    let tail = journal.tail();
    drop(journal);

    let reopened = FileJournal::open_active(&base)?;
    assert_eq!(
        reopened.path(),
        live.as_path(),
        "the base path reaches the live file"
    );
    assert_eq!(
        reopened.generation(),
        generation,
        "of the generation the run left"
    );
    assert_eq!(reopened.tail(), tail, "at the tail the run left");
    assert_fold_agrees(&reopened, &oracle, 1)?;
    sim.record("base-path-reaches-the-live-generation");
    sim.trace.record_number("walk-continuations", continuations);
    sim.trace.record_number("walk-trailing", trailing);
    Ok(())
}

/// A base named like a generation is refused before the file exists, and the same
/// name inside a chain's own directory is that chain's generation.
pub(super) fn ambiguous_base(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("named")?;
    let number = draw(sim, 1, 999_998);
    let name = format!("{number:06}");
    let path = dir.join(&name);
    let refused = FileJournal::open_continuing_with(&path, ContinuationPolicy::declared());
    assert!(
        matches!(
            refused.as_ref().err(),
            Some(&JournalError::CapacityExceeded {
                resource: JournalLimitKind::Generation,
                requested,
                ..
            }) if requested == number
        ),
        "a base named {name} could be read as a generation and must be refused naming \
         it: {:?}",
        refused.as_ref().err()
    );
    assert!(!path.exists(), "refused before the file was created");

    let chain = dir.join("run.jrnl.cont");
    std::fs::create_dir_all(&chain)?;
    let generation = FileJournal::open(chain.join(&name))?;
    assert_eq!(
        generation.generation(),
        number.saturating_add(1),
        "inside a chain's directory the same name is that chain's generation"
    );
    sim.record("generation-shaped-base-refused");
    sim.trace.record_number("named", number);
    Ok(())
}

/// The seal is one frame, byte-identical in both files.
pub(super) fn seal_bytes(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("frame")?;
    let base = dir.join("run.jrnl");
    let sealed = dir.join("run.jrnl.cont").join("000001");
    let attempts = draw(sim, 1, 40);
    let mut journal = quiet(&base)?;
    let mut oracle = Oracle::default();
    write_drawn(sim, &mut journal, &mut oracle, 1, (1, attempts))?;
    let before = length(&base)?;
    assert!(!sealed.exists(), "no successor exists before the seal");

    let successor = seal(&mut journal)?;
    let successor_bytes = std::fs::read(&sealed)?;
    let grown = std::fs::read(&base)?.split_off(usize::try_from(before)?);
    assert_eq!(
        grown, successor_bytes,
        "the predecessor grew by exactly the successor's whole first frame, byte for byte"
    );
    assert_fold_agrees(&successor, &oracle, 1)?;
    sim.record("seal-frame-identical-in-both-files");
    sim.trace.record_number("frame-attempts", attempts);
    sim.trace
        .record_number("frame-bytes", successor_bytes.len());
    Ok(())
}

/// A sealed predecessor refuses every append, every second seal and every open,
/// each naming its successor, and keeps the bytes the seal left.
pub(super) fn sealed_refuses(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("sealed")?;
    let base = dir.join("run.jrnl");
    let attempts = draw(sim, 1, 30);
    let mut journal = quiet(&base)?;
    let mut oracle = Oracle::default();
    write_drawn(sim, &mut journal, &mut oracle, 1, (1, attempts))?;
    let successor = seal(&mut journal)?;
    let next = successor.path().to_path_buf();
    let named = next.display().to_string();
    let sealed_length = length(&base)?;
    assert_eq!(
        journal.sealed_by(),
        Some(next.as_path()),
        "the sealed handle names where authority went"
    );

    let fresh = attempts.saturating_add(draw(sim, 1, 5));
    let refused = journal.compare_and_append(journal.tail(), &intent(1, fresh)?);
    assert!(
        matches!(&refused, Err(JournalError::Superseded { path }) if *path == named),
        "an append to the sealed handle names the successor: {refused:?}"
    );
    assert!(
        matches!(
            journal.continue_as_file(),
            Err(JournalError::Superseded { .. })
        ),
        "a sealed handle cannot seal a second time"
    );
    drop(journal);
    let reopened = FileJournal::open(&base);
    assert!(
        matches!(reopened.as_ref().err(), Some(JournalError::Superseded { path }) if *path == named),
        "opening the sealed file names its successor: {:?}",
        reopened.as_ref().err()
    );
    assert_eq!(
        length(&base)?,
        sealed_length,
        "every refusal left the sealed file's bytes where the seal put them"
    );
    assert_fold_agrees(&successor, &oracle, 1)?;
    sim.record("sealed-predecessor-refuses");
    sim.trace.record_number("sealed-attempts", attempts);
    Ok(())
}

/// A seal stopped at any boundary leaves exactly one file that takes appends, and
/// that file still holds what the oracle recorded, the unknown attempt included.
pub(super) fn stopped_seal(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("stopped")?;
    let base = dir.join("run.jrnl");
    let successor = dir.join("run.jrnl.cont").join("000001");
    let boundaries = SealPause::all();
    let boundary = boundaries[usize::try_from(sim.rng().below(4))?];
    let attempts = draw(sim, 2, 20);
    let unknown = attempts.saturating_add(1);
    let mut journal = quiet(&base)?;
    let mut oracle = Oracle::default();
    write_drawn(sim, &mut journal, &mut oracle, 1, (1, attempts))?;
    write(&mut journal, &mut oracle, 1, (unknown, 1), Fate::Unknown)?;

    journal.arm_continuation_pause(boundary);
    let stopped = journal.continue_as_file();
    assert!(
        matches!(stopped, Err(JournalError::ContinuationPaused { boundary: at }) if at == boundary),
        "an armed seal stops at {boundary}"
    );
    let after = journal.compare_and_append(journal.tail(), &intent(1, unknown.saturating_add(1))?);
    assert!(
        matches!(after, Err(JournalError::Superseded { .. })),
        "a stopped seal leaves its handle read-only: {after:?}"
    );
    drop(journal);

    let mut live = FileJournal::open_active(&base)?;
    let other = if live.path() == base.as_path() {
        successor
    } else {
        base
    };
    if other.exists() {
        let foreign = intent(77, 1)?;
        let refused = FileJournal::open(&other)
            .and_then(|mut file| file.compare_and_append(file.tail(), &foreign));
        assert!(
            refused.is_err(),
            "after a seal stopped at {boundary}, {} is live and {} still took an append",
            live.path().display(),
            other.display()
        );
    }
    assert_eq!(
        live.recover().status(key_of(1, unknown)?),
        Some(AttemptStatus::OutcomeUnknown),
        "the attempt left unknown is unknown in whichever file is live"
    );
    assert_fold_agrees(&live, &oracle, 1)?;
    for number in 1..=unknown {
        let again = live.compare_and_append(live.tail(), &intent(1, number)?);
        assert!(
            again.is_err(),
            "attempt {number} was admitted a second time after a seal stopped at {boundary}"
        );
    }
    live.compare_and_append_all(&ladder(Fate::Applied, 1, unknown.saturating_add(1))?)?;
    sim.record(&format!("stopped-at-{boundary}"));
    sim.trace
        .record_number("stopped-generation", live.generation());
    sim.trace.record_number("stopped-attempts", attempts);
    Ok(())
}
