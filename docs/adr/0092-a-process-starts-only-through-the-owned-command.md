# ADR 0092: a process starts only through the owned Command

Status: **accepted**, 2026-11-27.

Stakeholders: the Director; every repo built on the crates; the Mac and boxes their tests run on.
Concerns: a child started with `std`'s `Command` outlives a SIGKILLed parent, and a script's background job outlives
its leader.
Requirements: INV-STD-COMMAND-1, INV-PROVE-1.
Viewpoint: the process-lifetime view: who starts a child, and what ends it.
Elements and relations: `lgwks_std::process::Command` and `Child`, the lifeline (ADR 0087), the ledger (ADR 0090),
the `raw-process` scan rule (ADR 0089).

## Context

The tests of logical_ci and larry started servers with a raw `Command`. A SIGKILL ended the test and left the children
on a 24 GB Mac for hours (#1160). ADR 0089 names the async `Supervisor` as the owner, but a sync caller, a tool or a
test, had no owner to move to. The meta prover (ADR 0091) needs one owner per kind.

## Decision

1. `lgwks_std::process::Command` is `std`'s builder with an owned start. On Unix the child leads a group of its own,
   held on the lifeline before `spawn` returns.
2. When the leader ends, `wait`, `try_wait`, `status` and `output` kill its group, then reap it. The unreaped leader
   pins the group id while it is signalled.
3. Under parallel load macOS can refuse `killpg` with `EPERM` for a group that holds a zombie, and then signal no
   member (larry#131, 7 of 24 runs). On an idle Mac the same kill succeeds (measured 2026-11-27). On `EPERM` each live
   member is killed by pid from `KERN_PROC_PGRP`, for at most 64 rounds, behind a host seam a seeded model drives.
4. A dropped `Child` kills its group and reaps it. A child that could not be held is killed, and the hold's error
   returned.
5. The `raw-process` rule counts a `Command` reached through `lgwks_std` as the owner, and its finding names it.

## Alternatives rejected

- **An extension trait on `std`'s `Command`.** Its builder methods return `std`'s type, so `.arg(x).spawn()` reaches
  the raw start, and the source cannot tell the two apart.
- **Kill the group after the reap.** Once no member is left, the pid can be reissued, and a stranger that leads a new
  group under it is signalled.
- **Only the async `Supervisor`.** A sync tool or test would need a runtime to start `git`.

## Consequences

- Each start takes a lifeline hold: one pipe line and one fsync'd ledger line.
- Bounds: a child that calls `setsid` leaves its group, and the census names it (ADR 0082). About 100 µs between the
  fork and the hold's write, a SIGKILL of the caller leaves the child. Off Unix a dropped child's tree is stopped; a
  killed caller's is not.
- `Child` keeps its pipes private: `take_stdin` and `take_stdout` replace `std`'s public fields, and
  `wait_with_output` reads a piped error stream. The `edge-load` rig is the first caller moved to it.

## Correspondence

Refines ADR 0089. Uses ADR 0087 and 0090. Closes the process owner of ADR 0091.

## Verification

`sim_command_owns_every_child_by_every_end` ends a group with a background member by each of 7 ends, the owner's
SIGKILL included, on the real kernel. `sim_child_group_end_matches_the_model` drives the `EPERM` rounds against a model
that races, refuses and forks. `sim_raw_process_finds_exactly_the_raw_starts` holds the rule to its model.
