# Async runtime parity: What closed means

Part of [Async runtime parity](async-parity.md).

## 4.1 What "closed" means for virtual time and introspection (#152)

`tokio::time::pause`/`advance` work by replacing the engine's timer driver.
This crate does not, and the difference is the point rather than a gap.

**What shipped.** `rt::clock::Clock` is a *declared* clock: `Clock::wall`
follows real time, `Clock::virtual_at` is driven by the caller, and
`rt::time::Deadline` names the clock that governs a deadline instead of an
opaque `Instant`. The `Supervisor::snapshot` call reads the supervisor's own admission
and reporting fields for a bounded view of live capacity and the next eligible
action.

**Why not the engine's `pause`.** It would require enabling `tokio/test-util`,
which is a new feature edge on the `tokio` edge this workspace admits by exact
allowlist (`contract/APPROVED.toml`, INV-DEP-3) — not a decision a parity review
makes on its own. And it would be the *wrong* shape for this crate: `pause`
swaps the engine's timer and leaves every `std::time::Instant` reading in the
program still on real time. This crate already has wall-clock budgets
(`Supervisor`'s cooperative drain grace, a process deadline), so a "paused"
runtime would still hang on any of them. A declared clock names which clock
governs each deadline and keeps the watchdog separate — the property
`INV-BOT-30` states and `tests/it/sim_clock.rs` proves.

**What this does not claim.** A declared clock determinizes *which deadline is
eligible*. It does not make poll order across workers deterministic, it does not
make an external system deterministic, and it does not measure cross-host clock
skew. Those are named in `rt::clock`'s module docs as explicitly out of scope.

**The remaining gap, stated.** There is no way to advance the *engine's* timer
from the public surface. A test that needs a real `sleep` to resolve without a
real wait still waits. That is the honest residual of not taking a new
dependency edge, and it is the one thing a future admission of `tokio/test-util`
would buy.

## 5. Three deliberate divergences

- **No `#[main]`/`#[test]` attribute macro.** A re-exported proc-macro expands
  to `::tokio` paths a consumer without a `tokio` edge cannot resolve, so it
  would compile only for consumers who had already broken the single-entry rule.
  Entry is `Runtime::block_on`.
- **`rt::time::sleep` defers construction to first poll.** tokio's own `sleep`
  captures the reactor at construction and panics with *"there is no reactor
  running"* when built outside a runtime, the most common footgun in the
  library. The wrapper makes `sleep(d)` constructible anywhere, which is a
  divergence in behaviour and a strict improvement in ergonomics. `interval` is
  the exception, because it is a value rather than a future and must still be
  built inside a runtime; the module documents that.
- **Supervised execution is stricter than the ecosystem's, not wider.**
  The `rt::supervise::Supervisor` type has no direct counterpart in §2. The `tokio_util`
  `TaskTracker` tracks tasks and lets a caller wait for them, but it does not
  bound how many run at once, does not refuse when full, and carries no loop
  budget. Requiring a `Budget` before a loop can be written is a tighter
  contract than any runtime here imposes, and that is the point: the rule in
  this workspace is that no background task is untracked and no loop is
  unbounded. A consumer who wants the looser model still has a bare `JoinSet`,
  which tracks what it starts and aborts on drop, but imposes no ceiling of its
  own.

## 5a. What "supervised" guarantees, and the five things it does not

The phrase "cannot express an untracked task or an unbounded loop" is about the
**API surface**, and it is true there. Read as a runtime claim it would be false,
so the boundary is stated explicitly. Five distinct facts are easy to collapse
and are kept apart:

1. **Tracked lifetime ≠ termination.** Every supervised task is registered and
   reported. Registration is a fact about bookkeeping, not about the task
   having stopped. `TaskId` tells you what was tracked.
2. **A cancellation *request* ≠ actual termination.** A cancellation token is a
   signal a task observes at a poll boundary. Nothing forces a task to observe
   it.
3. **A non-yielding callback is not preemptible.** A `Future` that never
   returns `Pending`, or a closure that never yields, holds its thread until it
   finishes on its own terms. This runtime is cooperative; there is no
   preemption here, and shutdown **waits** for such work rather than
   interrupting it. `shutdown_timeout` bounds how long shutdown *waits*, and a
   timed-out shutdown leaves the task running rather than terminating it.
4. **Bounded APIs are bounded where they say they are.** `join_all_bounded`
   bounds concurrent tasks by `limit`, and the `Supervisor` refuses past its
   in-flight ceiling. The **low-level `rt::task::JoinSet` has no capacity
   ceiling at all** — it tracks and aborts on drop, and a consumer can insert
   unboundedly many tasks into it. A bound on one API is not a property of the
   runtime.
5. **Progress detail can be dropped; terminal state is authoritative.** The
   per-task progress buffer is bounded and overflows *by design*: counters
   increment and the retained detail is the most recent window, while the
   terminal/Unknown state of a task stays retrievable. "The progress log is
   bounded" is not "the task's outcome is bounded".

So the accurate sentence is: *this crate offers a supervised API whose declared
ceilings are real, and which cannot express an untracked task; it does not
guarantee that a supervised task terminates when asked, and it makes no
preemption claim.* Any stronger reading — that a bounded API cannot leak, that
cancellation always wins, that shutdown implies termination — is not what the
code does.

## 6. What parity is not claimed

Parity with tokio's *engine* is not the claim and would be meaningless. The
engine is tokio. Parity with its *surface* is claimed for the matrix in §2 and
only there. The facade is narrower on purpose: `rt::fs` runs on the blocking
pool rather than a true async file API, and `rt::net` reaches the network
directly rather than through the workspace's HTTP egress policy. Both are stated
in the module docs, where a consumer will read them, and not only here.
