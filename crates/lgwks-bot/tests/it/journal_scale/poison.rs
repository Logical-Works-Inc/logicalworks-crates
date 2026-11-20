//! The bare-write control the latency family judges the append path against, and the
//! hand-off slot it shares with its writer thread: a slot whose lock is poisoned is a
//! failure the test reports, never a hang.

use std::error::Error;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Instant;

/// The size of the bare write beside each append, near one framed event.
const CONTROL_BYTES: usize = 128;

/// A bare durable write that is reached and answered the way an append is.
///
/// An append is handed to its storage owner's thread and answered back: the owner
/// wakes, writes and syncs, and the caller wakes again, so a host with a long run
/// queue charges it a wait at each wake. A bare write on the caller's own thread
/// is charged none of those waits, so on a loaded host it read about a quarter of
/// the path (run 38938628208: append p50 13,488us against 3,251us bare, where an
/// idle host reads 78us against 68us) and every stall the run queue dealt landed
/// on the path alone. This control makes the same two hand-offs around the same
/// write and `sync_all`, so the two differ by what the path adds and not by how
/// many times each waits to be scheduled.
pub(super) struct HoppedWrite {
    /// What the caller asked for and what the writer answered.
    slot: Arc<Slot>,
    /// The writer's thread, joined on drop so it never outlives the test.
    writer: Option<std::thread::JoinHandle<()>>,
}

/// The hand-off state and the one condition variable both sides wait on, as the
/// storage owner's request slot has them.
struct Slot {
    /// One request in flight, one answer waiting.
    state: Mutex<Handoff>,
    /// Woken on every change of `state`.
    signal: Condvar,
}

/// The control's hand-off state: one request in flight, one answer waiting.
#[derive(Default)]
struct Handoff {
    /// A write was asked for and the writer has not taken it yet.
    asked: bool,
    /// The outcome of the last write, until the caller takes it.
    answered: Option<std::io::Result<()>>,
    /// The caller is done with the writer.
    stop: bool,
}

/// A poisoned hand-off slot, as the error a test reports.
fn poisoned<T>(error: &PoisonError<T>) -> String {
    format!("the control's hand-off slot was poisoned: {error}")
}

impl HoppedWrite {
    /// Start the writer thread on `file`.
    pub(super) fn start(mut file: std::fs::File) -> std::io::Result<Self> {
        use std::io::Write as _;

        let slot = Arc::new(Slot {
            state: Mutex::new(Handoff::default()),
            signal: Condvar::new(),
        });
        let theirs = Arc::clone(&slot);
        let writer = std::thread::Builder::new()
            .name(String::from("control-writer"))
            .spawn(move || {
                // A poisoned slot still carries its data: the writer keeps serving, so the
                // caller always gets an answer and never waits on a writer that left.
                loop {
                    let held = match theirs.state.lock() {
                        Ok(held) => held,
                        Err(poison) => poison.into_inner(),
                    };
                    let mut held = match theirs
                        .signal
                        .wait_while(held, |state| !state.asked && !state.stop)
                    {
                        Ok(held) => held,
                        Err(poison) => poison.into_inner(),
                    };
                    if held.stop {
                        return;
                    }
                    held.asked = false;
                    drop(held);
                    let outcome = file
                        .write_all(&[0xA5; CONTROL_BYTES])
                        .and_then(|()| file.sync_all());
                    let mut held = match theirs.state.lock() {
                        Ok(held) => held,
                        Err(poison) => poison.into_inner(),
                    };
                    held.answered = Some(outcome);
                    drop(held);
                    theirs.signal.notify_all();
                }
            })?;
        Ok(Self {
            slot,
            writer: Some(writer),
        })
    }

    /// One durable write, in microseconds from the request to the answer.
    pub(super) fn timed(&self) -> Result<u128, Box<dyn Error>> {
        let start = Instant::now();
        let mut held = self.slot.state.lock().map_err(|error| poisoned(&error))?;
        held.asked = true;
        self.slot.signal.notify_all();
        let mut held = self
            .slot
            .signal
            .wait_while(held, |state| state.answered.is_none())
            .map_err(|error| poisoned(&error))?;
        let outcome = held
            .answered
            .take()
            .ok_or("the control writer gave no answer")?;
        drop(held);
        outcome?;
        Ok(start.elapsed().as_micros())
    }
}

impl Drop for HoppedWrite {
    fn drop(&mut self) {
        match self.slot.state.lock() {
            Ok(mut held) => held.stop = true,
            Err(poison) => poison.into_inner().stop = true,
        }
        self.slot.signal.notify_all();
        if let Some(writer) = self.writer.take()
            && let Err(panic) = writer.join()
        {
            lgwks_std::trace::debug!(panic = ?panic, "the control writer panicked");
        }
    }
}
