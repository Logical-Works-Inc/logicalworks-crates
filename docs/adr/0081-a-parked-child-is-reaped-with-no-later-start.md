# ADR 0081: a parked child is reaped with no later start

Status: **accepted**, 2026-11-23.

Stakeholders: the Director; every program that drops a `Supervisor` with children running.
Concerns: no killed child stays a zombie, holding its pid, for the life of the process.
Requirements: INV-BOT-LEDGER-1.
Viewpoint: the child ownership in `rt/supervise/start.rs`.
Elements and relations: `OwnedChild::drop` parks into `ORPHANS`; `park` starts `reap_parked` on the process driver,
owned by the `REAPER` set.

## Context

A dropped `OwnedChild` kills its child and, when it has not exited yet, parks it in `ORPHANS`. Only the next start
or drop reaped that ledger. A process that dropped a supervisor and spawned nothing after kept those children as
zombies until it exited. No test read the ledger (#1128 G4). The new sim failed on seed 4: 1 orphan stayed parked.

## Decision

1. A park starts one reaper task when none runs. It reaps `ORPHANS` every `REAP_POLL` (5 ms) and stops when the
   ledger is empty.
2. The reaper's state is read and written only while `ORPHANS` is held, so a park cannot miss an exiting reaper.
3. The task runs on the process driver, which is never dropped, and a static `JoinSet` owns it. Each park collects
   the ended tasks first, so the set holds at most one live task.

## Alternatives rejected

- **Wait for the child in `drop`.** A drop runs on a runtime worker, and a child in an uninterruptible wait would
  block it for as long as the wait lasts.
- **A reaper on the caller's runtime.** The sim's runtime drops right after its supervisor, which aborts the task
  before the child is torn down.

## Consequences

- A process with parked children runs one more task, at one reap per 5 ms, until they are gone.
- A driver the OS refused leaves the children parked for the next start, named in the trace.

## Correspondence

Closes #1128 G4. Relates to ADR 0077, the other cleanup a dropped group owes.

## Verification

`sim_ledgers_drain_after_every_shape_of_run` drives 24 seeds of real children through shutdown or a dropped
supervisor and reads both ledgers empty after each, reaping nothing by hand.
