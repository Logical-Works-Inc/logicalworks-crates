//! The pool's lock helpers: one recovery from poisoning per lock shape.

use std::sync::{Condvar, Mutex, MutexGuard, WaitTimeoutResult};
use std::time::Duration;

/// Take `mutex`, treating poisoning as non-fatal.
///
/// Poisoning means some thread panicked while holding the lock. Every critical
/// section behind this mutex moves a whole job in or out and writes no
/// partial state, so a poisoned lock still guards a consistent value and the
/// panic itself is already being resumed on the awaiter. Recovering the guard
/// is therefore correct here, and it is why this is not an `unwrap`: a
/// `JoinHandle` must not turn a worker's failure into a deadlock for the task
/// awaiting it.
pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Wait on `condition` until woken, treating poisoning as non-fatal.
///
/// The same guarded state `lock` recovers, through the same argument: every
/// critical section behind these primitives moves a whole job in or out and
/// writes no partial state, so a poisoned guard still names a consistent value
/// and the panic that poisoned it is already being resumed on the awaiter. One
/// helper per lock shape rather than one recovery per call site, because a
/// fourth spelling of the same recovery is a fourth place to get it wrong.
///
/// Untimed because `block_on` waits for exactly one thing, its waker, and has
/// no deadline of its own to bound the wait with; the pool itself only ever
/// waits against a deadline.
pub(super) fn wait<'a, T>(condition: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    match condition.wait(guard) {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Wait on `condition` for at most `timeout`, treating poisoning as non-fatal.
///
/// [`wait`]'s argument applies unchanged to the timed form: the guard it returns
/// names the same consistent state, and the wait outcome is reported beside it.
pub(super) fn wait_timeout<'a, T>(
    condition: &Condvar,
    guard: MutexGuard<'a, T>,
    timeout: Duration,
) -> (MutexGuard<'a, T>, WaitTimeoutResult) {
    match condition.wait_timeout(guard, timeout) {
        Ok(waited) => waited,
        Err(poisoned) => poisoned.into_inner(),
    }
}
