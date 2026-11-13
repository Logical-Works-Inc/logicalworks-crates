//! What a zero-timeout `WaitForSingleObject` on a process handle answers.
//!
//! A process object is signalled once the process has exited, so the wait
//! is the liveness reading; the exit code is not, because `STILL_ACTIVE`
//! (259) is also a code a process may exit with (#913). The reading is pure
//! and lives apart from the `kernel32` calls so every host judges it.

/// `WAIT_OBJECT_0`: the waited object is signalled (the process exited).
pub(super) const WAIT_OBJECT_0: u32 = 0;
/// `WAIT_TIMEOUT`: a zero-timeout wait found the process still running.
pub(super) const WAIT_TIMEOUT: u32 = 258;

/// Whether a wait that returned `result` found the process exited: `Some`
/// for the two answers a process handle gives, `None` for `WAIT_FAILED` and
/// any other result, which the caller reports as the wait's own error.
pub(super) const fn exited_from_wait(result: u32) -> Option<bool> {
    match result {
        WAIT_OBJECT_0 => Some(true),
        WAIT_TIMEOUT => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{WAIT_OBJECT_0, WAIT_TIMEOUT, exited_from_wait};

    /// `<synchapi.h>` and the `WaitForSingleObject` page: a signalled process
    /// has exited, a timeout means it runs, and `WAIT_FAILED` (all bits set)
    /// or `WAIT_ABANDONED` (0x80, a mutex's answer) is neither.
    #[test]
    fn a_wait_result_is_exited_running_or_no_answer() {
        assert_eq!(exited_from_wait(WAIT_OBJECT_0), Some(true), "signalled");
        assert_eq!(exited_from_wait(WAIT_TIMEOUT), Some(false), "timed out");
        for other in [u32::MAX, 0x80, 1, 257, 259] {
            assert_eq!(exited_from_wait(other), None, "{other:#x} is no answer");
        }
    }
}
