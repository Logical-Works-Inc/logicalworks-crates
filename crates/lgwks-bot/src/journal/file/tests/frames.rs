//! Frames whose length prefix lies or whose tail is cut: refused when an acknowledged frame
//! is behind the lie, repaired when the cut is a torn append, and never trimmed on a guess.

#[cfg(test)]
use super::*;

/// The frame start offset at ordinal `at`, or the refusal that the file has none.
fn start_of(bytes: &[u8], at: usize) -> Result<usize, Box<dyn std::error::Error>> {
    let starts = frame_starts(bytes, 0)?;
    Ok(starts
        .get(at)
        .copied()
        .ok_or("the fixture file has fewer frames than the test names")?)
}

/// `bytes` up to `end`, or the refusal that `end` is past it.
fn head_of(bytes: &[u8], end: usize) -> Result<&[u8], &'static str> {
    bytes
        .get(..end)
        .ok_or("the fixture file ends before the cut the test names")
}

#[test]
fn an_inflated_length_over_a_complete_final_frame_is_refused_not_trimmed() -> TestResult {
    let (path, _guard) = subject("inflated-length")?;
    let (mut bytes, third_start) = three_frame_file(&path)?;
    let before = bytes.len();

    // Every length a scan can meet at the last frame's prefix: the
    // maximum the frame bound allows, one over it, and zero. None of them
    // is a torn append — a killed writer leaves a prefix of the bytes it
    // intended, and its completed prefix is always the true length — so
    // each must be refused with the file left byte-for-byte alone. The
    // inflation case is the destructive one: the declared payload runs
    // past the end of the file, and the tail looks torn to a reader that
    // trusts the prefix.
    let inflated = u32::try_from(MAX_FRAME_BYTES)?;
    let over_limit = inflated.saturating_add(1);
    for (name, replacement) in [
        ("the maximum legal length", inflated),
        ("one byte over the limit", over_limit),
        ("a zero length", 0u32),
    ] {
        let prefix = bytes
            .get_mut(third_start..)
            .ok_or("the fixture file ends before its third frame")?;
        for (slot, byte) in prefix
            .iter_mut()
            .take(LENGTH_BYTES)
            .zip(replacement.to_be_bytes())
        {
            *slot = byte;
        }
        std::fs::write(&path, &bytes)?;

        match FileJournal::open(&path) {
            Err(JournalError::Corrupt(corruption)) => {
                assert_eq!(corruption.at(), 2, "{name}: the lying frame is named");
                assert!(
                    matches!(corruption.kind(), CorruptionKind::Framed),
                    "{name}: the refusal names the framing, got {:?}",
                    corruption.kind()
                );
            }
            Err(other) => {
                return Err(format!("{name}: expected a corruption refusal, got {other}").into());
            }
            Ok(_) => {
                return Err(format!("{name}: a lying length must not reopen as a journal").into());
            }
        }
        assert_eq!(
            std::fs::metadata(&path)?.len(),
            u64::try_from(before)?,
            "{name}: refused bytes are never trimmed"
        );
    }
    Ok(())
}

#[test]
fn a_short_final_frame_is_still_repaired_as_a_torn_tail() -> TestResult {
    let (path, _guard) = subject("torn-tail")?;
    let (bytes, third_start) = three_frame_file(&path)?;
    let complete_len = u64::try_from(third_start)?;

    // A real interrupted append: the file ends mid-frame. This tail was
    // never acknowledged, so the reopen repairs it — and the repair must
    // survive the corruption distinction, not be swallowed by it.
    let torn = head_of(&bytes, third_start.saturating_add(10))?;
    std::fs::write(&path, torn)?;

    let reopened = FileJournal::open(&path)?;
    assert!(
        reopened.torn_tail_repaired(),
        "an interrupted append is repaired, not refused"
    );
    assert_eq!(
        reopened.committed()?.len(),
        2,
        "the two kept frames survive"
    );
    assert_eq!(
        std::fs::metadata(&path)?.len(),
        complete_len,
        "the repair restores the prefix every acknowledgment names"
    );
    Ok(())
}

/// The acknowledged final frame whose length grows by `k` is the defect #262
/// reported. `k` in `1..=32` leaves the payload whole and the head short, so the
/// payload read succeeds and the head read is what ends early; `k` above `32`
/// ends inside the payload. Both are one lie told two ways, and both refuse.
#[test]
fn a_lengthened_acknowledged_final_frame_is_refused_not_trimmed() -> TestResult {
    let (path, _guard) = subject("lengthened-final")?;
    let (bytes, third) = three_frame_file(&path)?;
    let declared = declared_at(&bytes, third);
    for extra in 1u32..=1024 {
        let lied = with_prefix(&bytes, third, declared + extra);
        std::fs::write(&path, &lied)?;
        require_refused_untouched(&path, &lied, 2, &format!("final frame L+{extra}"))?;
    }
    Ok(())
}

/// The same lie on a successor whose one event follows the seal it carries.
///
/// The cut is resolved against the last whole frame, and in a successor with
/// no event before the lie that frame is the carried seal. Resolved against
/// the events alone it was the genesis, which the acknowledged event does not
/// chain from, so the lie read as a torn append and the event was trimmed.
#[test]
fn a_lengthened_event_behind_a_carried_seal_is_refused_not_trimmed() -> TestResult {
    let (dir, _guard) = subject("lengthened-successor")?;
    let successor = sealed_successor(&dir)?;
    let bytes = std::fs::read(&successor)?;
    let event = start_of(&bytes, 1)?;
    let declared = declared_at(&bytes, event);
    for extra in 1u32..=96 {
        let lied = with_prefix(&bytes, event, declared + extra);
        std::fs::write(&successor, &lied)?;
        require_refused_untouched(&successor, &lied, 0, &format!("successor event L+{extra}"))?;
    }
    Ok(())
}

/// The second shape from the same finding: an acknowledged frame with frames
/// behind it whose length is inflated past the end of the file. The true frame
/// is still the first thing behind the prefix, so it authenticates and the two
/// frames after it are not read as one more candidate and dropped.
#[test]
fn an_inflated_non_final_length_is_refused_and_every_byte_survives() -> TestResult {
    let (path, _guard) = subject("inflated-middle")?;
    let (bytes, _) = three_frame_file(&path)?;
    let middle = start_of(&bytes, 1)?;
    let remaining = u32::try_from(bytes.len() - middle)?;
    for declared in [
        remaining,
        remaining + 1,
        remaining + 31,
        remaining + 500,
        u32::try_from(MAX_FRAME_BYTES)?,
    ] {
        let lied = with_prefix(&bytes, middle, declared);
        std::fs::write(&path, &lied)?;
        require_refused_untouched(
            &path,
            &lied,
            1,
            &format!("middle frame declared {declared}"),
        )?;
    }
    Ok(())
}

/// The control: a genuinely cut append is still repaired. Every byte offset
/// inside the final frame is a place a killed writer could have stopped, and each
/// must reopen with the two acknowledged frames, report the repair, and leave the
/// file at exactly the acknowledged prefix. This is what keeps the refusal above
/// from being bought by refusing everything.
#[test]
fn an_append_cut_at_every_byte_of_the_final_frame_is_repaired() -> TestResult {
    let (path, _guard) = subject("cut-every-byte")?;
    let (bytes, third) = three_frame_file(&path)?;
    for cut in third + 1..bytes.len() {
        std::fs::write(&path, head_of(&bytes, cut)?)?;
        let reopened = FileJournal::open(&path).map_err(|error| {
            format!(
                "a final frame cut at byte {cut} of {} was refused: {error}",
                bytes.len()
            )
        })?;
        assert!(
            reopened.torn_tail_repaired(),
            "cut at {cut} was not reported as repaired"
        );
        assert_eq!(
            reopened.committed()?.len(),
            2,
            "cut at {cut} lost an acknowledged frame"
        );
        drop(reopened);
        assert_eq!(
            std::fs::metadata(&path)?.len(),
            u64::try_from(third)?,
            "cut at {cut} was not trimmed to the acknowledged prefix"
        );
    }
    Ok(())
}

/// A middle frame whose length is inflated *and* whose own payload is damaged
/// authenticates as nothing itself, but the acknowledged frame behind it still
/// does, so the open refuses rather than trimming both.
#[test]
fn a_damaged_cut_frame_with_an_acknowledged_frame_behind_it_is_refused() -> TestResult {
    let (path, _guard) = subject("damaged-middle")?;
    let (bytes, _) = three_frame_file(&path)?;
    let middle = start_of(&bytes, 1)?;
    let remaining = u32::try_from(bytes.len() - middle)?;
    let mut lied = with_prefix(&bytes, middle, remaining + 7);
    let damaged = lied
        .get_mut(middle + LENGTH_BYTES + 3)
        .ok_or("the fixture file ends before the byte the test damages")?;
    *damaged ^= 0x55;
    std::fs::write(&path, &lied)?;
    require_refused_untouched(&path, &lied, 1, "damaged middle frame")
}

/// The stated limit, pinned so the claim is no larger than the test: a final
/// frame whose length was changed and whose head was also damaged authenticates as
/// nothing and has nothing behind it, so it is indistinguishable from an append
/// cut inside its head, and it is trimmed.
#[test]
fn a_final_frame_with_a_lying_length_and_a_damaged_head_is_the_stated_limit() -> TestResult {
    let (path, _guard) = subject("two-faults")?;
    let (bytes, third) = three_frame_file(&path)?;
    let declared = declared_at(&bytes, third);
    let mut lied = with_prefix(&bytes, third, declared + 5);
    let last = lied
        .last_mut()
        .ok_or("the lied-about file has no last byte")?;
    *last ^= 0xff;
    std::fs::write(&path, &lied)?;
    let reopened = FileJournal::open(&path)?;
    assert!(reopened.torn_tail_repaired());
    assert_eq!(reopened.committed()?.len(), 2);
    Ok(())
}

/// The streaming replay answers a lying length as `open` does. It reads from
/// its own descriptor, so a file changed after `open` is the case it can meet.
#[test]
fn a_streaming_replay_refuses_a_lengthened_final_frame_and_then_ends() -> TestResult {
    let (path, _guard) = subject("replay-lengthened")?;
    let (bytes, third) = three_frame_file(&path)?;
    let journal = FileJournal::open(&path)?;
    let declared = declared_at(&bytes, third);
    for extra in [1u32, 17, 32, 33, 400] {
        std::fs::write(&path, with_prefix(&bytes, third, declared + extra))?;
        let mut replay = journal.replay()?;
        assert!(
            matches!(replay.next(), Some(Ok(_))),
            "L+{extra}: first event"
        );
        assert!(
            matches!(replay.next(), Some(Ok(_))),
            "L+{extra}: second event"
        );
        assert!(
            matches!(replay.next(), Some(Err(JournalError::Corrupt(_)))),
            "L+{extra}: the lengthened frame must be a refusal, not the end of the stream"
        );
        assert!(
            replay.next().is_none(),
            "L+{extra}: a refusal ends the stream"
        );
    }
    std::fs::write(&path, head_of(&bytes, third + 10)?)?;
    let events = journal.replay()?.collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        events.len(),
        2,
        "a cut append still ends the stream quietly"
    );
    Ok(())
}
