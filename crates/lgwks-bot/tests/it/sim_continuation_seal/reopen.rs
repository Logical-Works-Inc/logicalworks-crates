//! What a reopen reads back: the replay of every generation, every acknowledged
//! position, tenants that share a directory, and a lengthened frame behind a seal.

use super::*;

/// The streaming replay of every generation yields exactly its committed events.
pub(super) fn replay_agrees(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("replay")?;
    let mut journal = quiet(&dir.join("run.jrnl"))?;
    let mut oracle = Oracle::default();
    let generations = draw(sim, 1, 4);
    let mut next = 1_u64;
    for generation in 1..=generations {
        let count = draw(sim, 1, 20);
        write_drawn(sim, &mut journal, &mut oracle, 1, (next, count))?;
        next = next.saturating_add(count);
        let streamed = journal.replay()?.collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            streamed,
            journal.committed()?,
            "generation {generation} streams exactly what it committed"
        );
        if generation < generations {
            journal = seal(&mut journal)?;
        }
    }
    assert_fold_agrees(&journal, &oracle, 1)?;
    sim.record("replay-equals-committed");
    sim.trace.record_number("replay-generations", generations);
    Ok(())
}

/// Every position a successor acknowledged reads back, as the event it
/// acknowledged, after a reopen.
pub(super) fn positions_read_back(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("positions")?;
    let base = dir.join("run.jrnl");
    let mut journal = quiet(&base)?;
    let mut oracle = Oracle::default();
    let hops = draw(sim, 1, 3);
    let mut next = 1_u64;
    for _ in 0..hops {
        let count = draw(sim, 1, 8);
        write(&mut journal, &mut oracle, 1, (next, count), Fate::Applied)?;
        next = next.saturating_add(count);
        journal = seal(&mut journal)?;
    }
    let mut acknowledged = Vec::new();
    for _ in 0..draw(sim, 2, 10) {
        for event in ladder(Fate::Applied, 1, next)? {
            let ack = journal.compare_and_append(journal.tail(), &event)?;
            acknowledged.push((ack.position(), event));
        }
        oracle.dispatch(1, next, Fate::Applied);
        next = next.saturating_add(1);
    }
    let generation = journal.generation();
    drop(journal);

    let reopened = FileJournal::open_active(&base)?;
    assert_eq!(
        reopened.generation(),
        generation,
        "the reopen reaches the generation that acknowledged them"
    );
    for &(position, ref event) in &acknowledged {
        let entry = reopened
            .committed_entry(position)?
            .ok_or("an acknowledged position reads back after a reopen")?;
        assert_eq!(entry.position(), position, "at the position acknowledged");
        assert_eq!(
            entry.event(),
            event,
            "and it holds the event that was acknowledged there"
        );
    }
    assert_fold_agrees(&reopened, &oracle, 1)?;
    sim.record("acknowledged-positions-read-back");
    sim.trace.record_number("positions", acknowledged.len());
    sim.trace.record_number("positions-generation", generation);
    Ok(())
}

/// One tenant's live journal, its oracle and its progress through the interleave.
struct InterleavedLane {
    /// The journal the tenant appends to, replaced by each successor.
    journal: FileJournal,
    /// What the tenant's journal must recover.
    oracle: Oracle,
    /// The next attempt number the tenant writes.
    next: u64,
    /// How many times the tenant's journal has continued.
    continued: u64,
}

/// Two tenants interleaved flush by flush in one directory continue on their own
/// triggers and never cross.
pub(super) fn tenants_interleaved(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("interleave")?;
    let mut lanes = Vec::new();
    for name in ["tenant-1.jrnl", "tenant-2.jrnl"] {
        lanes.push(InterleavedLane {
            journal: continuing(&dir.join(name), 96, QUIET_BYTES)?,
            oracle: Oracle::default(),
            next: 1,
            continued: 0,
        });
    }
    let steps = draw(sim, 24, 24);
    for _ in 0..steps {
        let who = usize::try_from(sim.rng().below(2))?;
        let action = u64::MAX.saturating_sub(u64::try_from(who)?);
        let count = draw(sim, 1, 8);
        let lane = lanes
            .get_mut(who)
            .ok_or("the draw names a tenant of the two")?;
        write_drawn(
            sim,
            &mut lane.journal,
            &mut lane.oracle,
            action,
            (lane.next, count),
        )?;
        lane.next = lane.next.saturating_add(count);
        if let Some(successor) = continue_if_due(&mut lane.journal)? {
            lane.journal = successor;
            lane.continued = lane.continued.saturating_add(1);
        }
    }
    for (who, lane) in lanes.iter().enumerate() {
        let tenant = u64::try_from(who)?;
        assert_fold_agrees(&lane.journal, &lane.oracle, tenant.saturating_add(1))?;
        let mine = action_of(u64::MAX.saturating_sub(tenant))?;
        for held in lane.journal.recover().attempts() {
            assert_eq!(
                held.key().action(),
                mine,
                "tenant {who} recovered another tenant's attempt"
            );
        }
        sim.trace
            .record_number(&format!("interleave-{who}-continued"), lane.continued);
    }
    let Ok([first, second]) = <[InterleavedLane; 2]>::try_from(lanes) else {
        lgwks_std::trace::debug!("tenants_interleaved: returning an error to the caller");
        return Err("two tenants interleave in one directory".into());
    };
    assert_ne!(
        first.journal.path(),
        second.journal.path(),
        "the two tenants never share a live file"
    );
    sim.record("interleaved-tenants-never-cross");
    sim.trace.record_number("interleave-steps", steps);
    Ok(())
}

/// One frame's start offset per frame of `bytes`, read off the length prefixes.
fn frame_starts(bytes: &[u8]) -> Result<Vec<usize>, Box<dyn Error>> {
    let mut starts = Vec::new();
    let mut at = 0_usize;
    while at < bytes.len() {
        starts.push(at);
        let prefix: [u8; 4] = bytes
            .get(at..at.saturating_add(4))
            .ok_or("a whole prefix at every frame start")?
            .try_into()?;
        let payload = usize::try_from(u32::from_be_bytes(prefix))?;
        at = at
            .saturating_add(4)
            .saturating_add(payload)
            .saturating_add(32);
    }
    Ok(starts)
}

/// A lengthened acknowledged event in a successor is refused, wherever it sits
/// behind the seal the successor carries and by however much it was lengthened.
///
/// The open resolves a cut against the last whole frame before it, which in a
/// successor holding one event is the carried seal. Resolved against the genesis
/// instead, that one acknowledged event read as an append nobody finished and was
/// trimmed. So after every append the seed lengthens every event frame the
/// successor holds — the first append is the one-event successor — and each lie is
/// refused with the file byte-identical; restoring the bytes reopens them whole,
/// so what is refused is the lie and not the file.
pub(super) fn lengthened_behind_seal(sim: &mut sim::Sim) -> TestResult {
    let dir = sim.scratch("lengthened")?;
    let mut journal = quiet(&dir.join("run.jrnl"))?;
    let mut oracle = Oracle::default();
    let before = draw(sim, 1, 6);
    write_drawn(sim, &mut journal, &mut oracle, 1, (1, before))?;
    let path = seal(&mut journal)?.path().to_path_buf();
    let own = draw(sim, 1, 4);
    for number in 1..=own {
        {
            let mut successor = FileJournal::open(&path)?;
            successor.compare_and_append(successor.tail(), &intent(2, number)?)?;
        }
        let bytes = std::fs::read(&path)?;
        let starts = frame_starts(&bytes)?;
        for (target, &at) in starts.iter().enumerate().skip(1) {
            let prefix: [u8; 4] = bytes
                .get(at..at.saturating_add(4))
                .ok_or("every frame has a prefix")?
                .try_into()?;
            let extra = 1_u32.saturating_add(sim.rng().below(96));
            let mut lied = bytes.clone();
            let lengthened = u32::from_be_bytes(prefix)
                .saturating_add(extra)
                .to_be_bytes();
            lied.splice(at..at.saturating_add(4), lengthened);
            std::fs::write(&path, &lied)?;
            let answer = FileJournal::open(&path);
            assert!(
                matches!(answer, Err(JournalError::Corrupt(_))),
                "event {target} of {number} lengthened by {extra} is refused, got {answer:?}"
            );
            assert_eq!(
                std::fs::read(&path)?,
                lied,
                "and the file is byte-identical"
            );
            sim.trace.record_number("lengthened-by", u64::from(extra));
        }
        std::fs::write(&path, &bytes)?;
        let whole = FileJournal::open(&path)?;
        assert_eq!(
            u64::try_from(whole.committed()?.len())?,
            number,
            "the restored file reopens with every acknowledged event"
        );
    }
    sim.trace.record_number("lengthened-events", own);
    Ok(())
}
