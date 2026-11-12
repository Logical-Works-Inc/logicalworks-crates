//! The loom model of the process-wide version 7 generator (INV-ID-2).
//!
//! `v7::new_v7` reads the clock and the entropy before it takes the lock, so a
//! thread that read an earlier millisecond can lock second. The claim is that
//! the lock order is the issue order: whatever interleaving loom drives, the
//! identifiers come out strictly increasing in the order the lock was held, and
//! the late thread with the earlier clock is clamped to the last stamp rather
//! than reordered. Source: RFC 9562 section 6.2, method 1 (a counter within a
//! stamp), and logical_ci's `ci/property-and-loom` obligation (its issue #36).
//!
//! Loom explores every interleaving of the threads under `loom::model`. It runs
//! under `RUSTFLAGS="--cfg loom" cargo test --release --lib loom`.

use loom::sync::{Arc, Mutex};
use loom::thread;

use super::Uuid;
use super::v7::{V7Error, V7Generator};

/// The generator and every issue it answered, in lock order.
type Shared = Arc<Mutex<(V7Generator, Vec<Result<Uuid, V7Error>>)>>;

/// The entropy of the thread whose clock reads earlier.
const EARLY: [u8; 10] = [0x7f; 10];
/// The entropy of the thread whose clock reads later.
const LATE: [u8; 10] = [0x01; 10];

/// One issue of `new_v7`'s protocol: the clock and entropy are already read,
/// then the lock is taken, the generator issues and its answer is recorded
/// under that same lock.
fn issue(shared: &Shared, now_ms: u64, entropy: [u8; 10]) {
    let mut guard = match shared.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let &mut (ref mut generator, ref mut answers) = &mut *guard;
    answers.push(generator.issue_at(now_ms, entropy));
}

/// One issue of `now_v7`'s protocol: as [`issue`], through the clamped rule
/// that never refuses, with `entropy` `None` when the OS refused the read.
fn issue_clamped(shared: &Shared, now_ms: u64, entropy: Option<[u8; 10]>) {
    let mut guard = match shared.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let &mut (ref mut generator, ref mut answers) = &mut *guard;
    answers.push(Ok(generator.issue_clamped(now_ms, entropy)));
}

/// Every answer `shared` holds once each thread has issued.
fn answers(shared: &Shared) -> Vec<Result<Uuid, V7Error>> {
    match shared.lock() {
        Ok(guard) => guard.1.clone(),
        Err(poisoned) => poisoned.into_inner().1.clone(),
    }
}

/// The 48-bit millisecond stamp of a version 7 identifier.
fn stamp(id: Uuid) -> u64 {
    let [b0, b1, b2, b3, b4, b5, ..] = *id.as_bytes();
    u64::from_be_bytes([0, 0, b0, b1, b2, b3, b4, b5])
}

/// Two threads, one reading a later clock than the other, issue in either
/// lock order; every order issues two identifiers that increase in lock order,
/// and the second keeps the first's stamp when its clock is not later.
#[test]
fn loom_the_lock_order_is_the_issue_order_whatever_the_clock_each_thread_read() {
    loom::model(|| {
        let shared: Shared = Arc::new(Mutex::new((V7Generator::new(), Vec::new())));
        let other = Arc::clone(&shared);
        let earlier_clock = thread::Builder::new()
            .name("earlier-clock".to_owned())
            .spawn(move || issue(&other, 1_000, EARLY));
        issue(&shared, 1_001, LATE);
        let joined = earlier_clock.map(thread::JoinHandle::join);
        assert!(
            matches!(joined, Ok(Ok(()))),
            "the earlier-clock thread was spawned and finished"
        );
        let ids: Vec<Uuid> = answers(&shared).into_iter().flatten().collect();
        assert_eq!(ids.len(), 2, "both issues inside 48 bits were answered");
        assert!(
            ids.windows(2)
                .all(|pair| matches!(pair, [first, second] if first < second)),
            "identifiers increase in lock order: {ids:?}"
        );
        let stamps: Vec<u64> = ids.iter().copied().map(stamp).collect();
        assert!(
            stamps == [1_000, 1_001] || stamps == [1_001, 1_001],
            "a later lock with an earlier clock keeps the last stamp: {stamps:?}"
        );
    });
}

/// `now_v7` on two threads at the same millisecond, one whose entropy read was
/// refused: in either lock order both issue, and the counter alone keeps the
/// two identifiers distinct and increasing (#851).
#[test]
fn loom_a_refused_entropy_read_still_issues_in_lock_order() {
    loom::model(|| {
        let shared: Shared = Arc::new(Mutex::new((V7Generator::new(), Vec::new())));
        let other = Arc::clone(&shared);
        let refused = thread::Builder::new()
            .name("refused-entropy".to_owned())
            .spawn(move || issue_clamped(&other, 2_000, None));
        issue_clamped(&shared, 2_000, Some(LATE));
        let joined = refused.map(thread::JoinHandle::join);
        assert!(
            matches!(joined, Ok(Ok(()))),
            "the refused-entropy thread was spawned and finished"
        );
        let ids: Vec<Uuid> = answers(&shared).into_iter().flatten().collect();
        assert_eq!(ids.len(), 2, "the clamped rule never refuses");
        assert!(
            ids.windows(2)
                .all(|pair| matches!(pair, [first, second] if first < second)),
            "identifiers increase in lock order: {ids:?}"
        );
    });
}
