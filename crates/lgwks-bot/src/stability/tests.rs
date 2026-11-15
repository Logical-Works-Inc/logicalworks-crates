//! The stable-read protocol's unit tests.

#[cfg(test)]
use super::{
    Drift, MAX_STABILITY_READS, READ_CHUNK, ReadFailure, Reading, read_stable, read_stable_file,
    read_to_end_watching,
};
use crate::sim_stream::TempDir;
use std::collections::VecDeque;
use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

type TestResult = Result<(), Box<dyn Error>>;

/// A typed test failure, in the shape the other modules' tests use. This
/// workspace forbids `panic!`, `unwrap` and `expect` in test code too.
fn failed(cause: impl Into<String>) -> Box<dyn Error> {
    Box::new(std::io::Error::other(cause.into()))
}

/// A fixed modification time, so two readings of the same bytes agree.
///
/// The epoch itself: any fixed instant serves, and this one needs no
/// arithmetic that could fail.
fn stamp() -> SystemTime {
    SystemTime::UNIX_EPOCH
}

/// What one reading of a scripted subject produced.
///
/// A `Vec` of readings with a cursor, so a scenario can say "read 1 gives
/// A, read 2 gives B" without a closure capturing a counter, and a read past
/// the script is refused rather than invented.
struct Script {
    readings: Vec<Reading>,
    cursor: AtomicUsize,
}

impl Script {
    fn new(readings: Vec<Reading>) -> Self {
        Self {
            readings,
            cursor: AtomicUsize::new(0),
        }
    }

    fn next(&self) -> Result<Reading, ReadFailure> {
        let index = self.cursor.fetch_add(1, Ordering::Relaxed);
        match self.readings.get(index) {
            Some(reading) => Ok(reading.clone()),
            None => Err(ReadFailure::Unreadable {
                cause: Box::new(std::io::Error::other("the script has no more readings")),
            }),
        }
    }
}

/// Two agreeing readings are one settled value.
#[test]
fn two_agreeing_readings_settle() -> TestResult {
    let script = Script::new(vec![
        Reading::of_subject(b"{\"a\":1}".to_vec(), 7, stamp()),
        Reading::of_subject(b"{\"a\":1}".to_vec(), 7, stamp()),
        Reading::of_subject(b"{\"a\":2}".to_vec(), 7, stamp()),
    ]);
    let settled = read_stable(|| script.next())?;
    assert_eq!(
        settled.bytes(),
        b"{\"a\":1}",
        "the settled value must be the one two reads agreed on, not a later one"
    );
    Ok(())
}

/// Every drift axis is named rather than collapsed into "different".
#[test]
fn every_drift_axis_is_reported_by_name() -> TestResult {
    let base = Reading::of_subject(b"12345".to_vec(), 5, stamp());
    let cases: [(Reading, Drift, &str); 4] = [
        (
            Reading::of_subject(b"123456".to_vec(), 6, stamp()),
            Drift::Length,
            "a subject that grew",
        ),
        (
            Reading::of_subject(
                b"12345".to_vec(),
                5,
                stamp()
                    .checked_add(std::time::Duration::from_secs(1))
                    .ok_or_else(|| failed("one second after the epoch is representable"))?,
            ),
            Drift::Modified,
            "a subject that was touched",
        ),
        (
            Reading::of_subject(b"12435".to_vec(), 5, stamp()),
            Drift::Contents,
            "a subject rewritten in place to the same length",
        ),
        (
            Reading::of_subject(b"123".to_vec(), 5, stamp()),
            Drift::Truncated,
            "a read cut short by a concurrent writer",
        ),
    ];
    for (other, drift, why) in cases {
        assert_eq!(
            base.agrees_with(&other),
            Some(drift),
            "{why}: the axis that moved is what a caller triages on"
        );
    }
    assert_eq!(Drift::Contents.as_str(), "contents");
    assert_eq!(
        base.agrees_with(&base),
        None,
        "a reading agrees with itself"
    );
    Ok(())
}

/// A subject that never settles is a refusal, bounded at the declared
/// number of reads, and it names what it saw.
#[test]
fn a_subject_that_never_settles_is_refused_at_the_bound() -> TestResult {
    let width = usize::try_from(MAX_STABILITY_READS)?;
    let mut drawn = Vec::new();
    for index in 0..=MAX_STABILITY_READS {
        drawn.push(Reading::of_subject(
            format!("{index:0>width$}").into_bytes(),
            u64::from(MAX_STABILITY_READS),
            stamp(),
        ));
    }
    let script = Script::new(drawn);
    let outcome = read_stable(|| script.next());
    let Err(ReadFailure::Unstable(unstable)) = outcome else {
        return Err(failed(format!(
            "a subject whose bytes change on every read must be refused: {outcome:?}"
        )));
    };
    assert_eq!(
        unstable.reads(),
        MAX_STABILITY_READS,
        "the attempt must stop at the declared bound rather than loop"
    );
    assert_eq!(
        unstable.drift(),
        Drift::Contents,
        "same length, same timestamp, different bytes: that is the contents axis"
    );
    assert_ne!(
        unstable.held(),
        unstable.observed(),
        "the refusal carries both digests, so a report can name what it saw"
    );
    Ok(())
}

/// An unreadable subject is not an unsettled one.
#[test]
fn an_unreadable_subject_is_its_own_arm() -> TestResult {
    let outcome = read_stable(|| {
        Err::<Reading, _>(ReadFailure::Unreadable {
            cause: Box::new(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        })
    });
    let Some(failure) = outcome.as_ref().err() else {
        return Err(failed("an unreadable subject must be refused"));
    };
    assert!(
        failure.unstable().is_none(),
        "a subject that could not be read has no reading to report: {failure}"
    );
    assert!(
        failure.source().is_some(),
        "the platform's own error is retained as the cause: {failure}"
    );
    Ok(())
}

/// A source that hands over one scripted piece per `read` call, the way a
/// file being rewritten hands a reader the old document and then the new
/// one's tail.
struct Pieces(VecDeque<Vec<u8>>);

impl std::io::Read for Pieces {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let Some(piece) = self.0.pop_front() else {
            return Ok(0);
        };
        let Some(target) = buffer.get_mut(..piece.len()) else {
            let refusal = Err(std::io::Error::other(
                "a scripted piece is larger than the read",
            ));
            lgwks_std::trace::debug!(
                error = ?refusal.as_ref().err(),
                "Pieces::read: returning an error to the caller"
            );
            return refusal;
        };
        target.copy_from_slice(&piece);
        Ok(piece.len())
    }
}

/// The splice a RAM-disk run found: a read that reached the old document's
/// end, then got the new document's tail, saw the file grow, and is torn
/// although its length matches the new document's.
#[test]
fn a_read_whose_end_moved_is_torn() -> TestResult {
    let before = b"{\"revision\":1}".to_vec();
    let tail = b"\"note\":\"rewritten\"}".to_vec();
    let mut spliced = Pieces(VecDeque::from([before, tail]));
    let (bytes, grew) = read_to_end_watching(&mut spliced)?;
    assert_eq!(
        bytes, b"{\"revision\":1}\"note\":\"rewritten\"}",
        "the reader returns what it was handed"
    );
    assert!(grew, "a short read followed by more bytes saw the end move");

    let mut whole = Pieces(VecDeque::from([b"{\"revision\":2}".to_vec()]));
    let (_, grew) = read_to_end_watching(&mut whole)?;
    assert!(!grew, "one short read to the end is a whole file");

    let mut large = Pieces(VecDeque::from([vec![b'a'; READ_CHUNK], vec![b'b'; 7]]));
    let (bytes, grew) = read_to_end_watching(&mut large)?;
    assert!(
        !grew,
        "a full read followed by more is a file larger than a chunk"
    );
    assert_eq!(
        bytes.len(),
        READ_CHUNK.saturating_add(7),
        "every byte is kept"
    );
    Ok(())
}

/// A torn reading never agrees, even with an identical one: two reads that
/// straddled the same rewrite the same way are the same splice twice.
#[test]
fn a_torn_reading_agrees_with_nothing() {
    let mut torn = Reading::of_subject(b"spliced".to_vec(), 7, stamp());
    torn.torn = true;
    assert!(!torn.is_whole(), "a torn reading is not whole");
    assert_eq!(
        torn.agrees_with(&torn),
        Some(Drift::Truncated),
        "two identical splices are not a settled subject"
    );
}

/// The real file path, against a real file.
#[test]
fn a_real_file_is_read_stably_and_a_missing_one_is_unreadable() -> TestResult {
    // A directory of this test's own, made by exclusive creation and removed
    // when it drops, so no run inherits another's files.
    let directory = TempDir::new("stability-file")?;
    let path = directory.path().join("subject.json");
    std::fs::write(&path, b"{\"a\":1}\n")?;
    let settled = lgwks_std::task::block_on(read_stable_file(&path))?;
    assert_eq!(
        settled.bytes(),
        b"{\"a\":1}\n",
        "a file nobody is writing reads the bytes it holds"
    );
    assert!(
        settled.is_whole(),
        "the reading must carry the subject's own length, so a cut read is visible"
    );

    let absent = path.with_extension("absent");
    let outcome = lgwks_std::task::block_on(read_stable_file(&absent));
    assert!(
        matches!(outcome, Err(ReadFailure::Unreadable { .. })),
        "a file that does not exist is unreadable, never unstable: {outcome:?}"
    );
    Ok(())
}
