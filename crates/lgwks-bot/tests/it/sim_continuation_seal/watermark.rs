//! The watermark: where a continuation falls due, by events and by bytes, and
//! what a journal that never chose a lifecycle reports.

use super::*;

/// The event trigger falls on the flush that reaches it, and never before.
///
/// The watermark is checked against this family's own count of what it wrote and
/// against the file's length on the disk, so the trigger is a fact about the file
/// rather than a number the journal agrees with itself about.
pub(super) fn event_watermark(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("events")?;
    let path = dir.join("run.jrnl");
    let events_at = draw(sim, 24, 100);
    let mut journal = continuing(&path, events_at, QUIET_BYTES)?;
    let mut appended = 0_u64;
    let mut attempt = 0_u64;
    loop {
        let watermark = journal.continuation_watermark()?;
        assert_eq!(
            watermark.events().0,
            appended,
            "the watermark counts exactly the events this file committed"
        );
        assert_eq!(
            watermark.bytes().0,
            length(&path)?,
            "and exactly the bytes on the disk"
        );
        assert_eq!(
            watermark.is_due(),
            appended >= events_at,
            "due at {appended} events of a {events_at}-event trigger"
        );
        if watermark.is_due() {
            break;
        }
        attempt = attempt.saturating_add(1);
        let rungs = ladder(Fate::Applied, 1, attempt)?;
        journal.compare_and_append_all(&rungs)?;
        appended = appended.saturating_add(u64::try_from(rungs.len())?);
    }
    sim.record("event-watermark-falls-on-its-flush");
    sim.trace.record_number("events-at", events_at);
    sim.trace.record_number("events-appended", appended);
    Ok(())
}

/// The byte trigger alone makes a continuation due, with the event trigger unmet.
pub(super) fn byte_watermark(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("bytes")?;
    let path = dir.join("run.jrnl");
    let bytes_at = draw(sim, 2_048, 6_144);
    let events_at = ContinuationPolicy::declared().events_at();
    let mut journal = continuing(&path, events_at, bytes_at)?;
    let mut attempt = 0_u64;
    loop {
        let watermark = journal.continuation_watermark()?;
        let on_disk = length(&path)?;
        assert_eq!(
            watermark.is_due(),
            on_disk >= bytes_at,
            "due at {on_disk} bytes of a {bytes_at}-byte trigger"
        );
        assert!(
            watermark.events().0 < events_at,
            "the event trigger never fell, so the byte trigger is the one being tested"
        );
        if watermark.is_due() {
            break;
        }
        attempt = attempt.saturating_add(1);
        journal.compare_and_append_all(&ladder(Fate::Applied, 1, attempt)?)?;
    }
    let successor = seal(&mut journal)?;
    assert_eq!(
        successor.generation(),
        2,
        "a byte-triggered continuation is an ordinary one"
    );
    sim.record("byte-watermark-alone-continues");
    sim.trace.record_number("bytes-at", bytes_at);
    sim.trace.record_number("bytes-attempts", attempt);
    Ok(())
}

/// A journal opened without a lifecycle is never due and never continues.
pub(super) fn never_due(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("plain")?;
    let path = dir.join("run.jrnl");
    let attempts = draw(sim, 4, 28);
    let mut journal = FileJournal::open(&path)?;
    let mut oracle = Oracle::default();
    write_drawn(sim, &mut journal, &mut oracle, 1, (1, attempts))?;
    let watermark = journal.continuation_watermark()?;
    assert!(
        !watermark.is_due(),
        "a journal that never chose a lifecycle is never due"
    );
    assert!(
        journal.continue_as_file()?.is_none(),
        "and asking it to continue is a no rather than a seal"
    );
    assert!(
        !journal.successor_path().exists(),
        "so no successor was written"
    );
    assert_eq!(journal.generation(), 1, "and it stays the first generation");
    assert_fold_agrees(&journal, &oracle, 1)?;
    sim.record("plain-journal-never-due");
    sim.trace.record_number("plain-attempts", attempts);
    Ok(())
}

/// The headroom falls by exactly what each flush appended, events and bytes both.
pub(super) fn headroom(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("headroom")?;
    let path = dir.join("run.jrnl");
    let mut journal = quiet(&path)?;
    let mut next = 1_u64;
    let flushes = draw(sim, 2, 6);
    for _ in 0..flushes {
        let before = journal.continuation_watermark()?;
        let length_before = length(&path)?;
        let count = draw(sim, 1, 12);
        let mut events = Vec::new();
        for number in next..next.saturating_add(count) {
            events.extend(ladder(drawn(sim), 1, number)?);
        }
        journal.compare_and_append_all(&events)?;
        next = next.saturating_add(count);
        let after = journal.continuation_watermark()?;
        // Headroom is the ceiling less the count, and the ceiling is fixed at open, so the
        // headroom falls by exactly what the count rose.
        assert_eq!(
            before.events().1,
            after.events().1,
            "the event ceiling is fixed"
        );
        assert_eq!(
            after.events().0.saturating_sub(before.events().0),
            u64::try_from(events.len())?,
            "the event headroom fell by the events this flush appended"
        );
        assert_eq!(
            before.bytes().1,
            after.bytes().1,
            "the byte ceiling is fixed"
        );
        assert_eq!(
            after.bytes().0.saturating_sub(before.bytes().0),
            length(&path)?.saturating_sub(length_before),
            "and the byte headroom by what the file grew"
        );
    }
    sim.record("headroom-falls-by-what-was-appended");
    sim.trace.record_number("headroom-attempts", next);
    Ok(())
}
