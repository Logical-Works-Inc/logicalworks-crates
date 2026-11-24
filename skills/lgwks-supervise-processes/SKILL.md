---
name: lgwks-supervise-processes
description: Use before starting any child process from lgwks_bot code, or when a spawned command leaks files, outlives its deadline or needs confinement. Covers ProcessSpec, the Supervisor, captured output, deadlines, cleanup receipts, the private TMPDIR per child and what is refused before the fork.
---

# Supervising processes

Never call `Command::spawn` in production code. The supervisor starts the child, owns its process group and its temp directory, and cleans both up on every path: a normal exit, a deadline, a cancel and a dropped task.

## Before you write

- The `process` feature is off by default, and the supervise and process modules are `#[doc(hidden)]`, outside the promise. Name them and pin an exact version of `lgwks_bot` (`skills/lgwks-use-the-crates/SKILL.md`).
- The calls are in `crates/lgwks-bot/src/rt/supervise/run.rs`: `spawn_process`, `run_process`, `run_process_observed` and `run_process_for`, with identified and gated variants of spawn. Read the signature there before use, and do not copy one from memory.
- A `ProcessSpec` is data only. You hand it to the `Supervisor` and get no process handle back, so you cannot forget to reap.
- Capture output with `StdioPolicy::Capture`. The drain after cleanup is bounded: a pipe a survivor still holds is reported as `DescriptorHeld`, and the run returns at the bound.

## What the supervisor owns

| Concern | Mechanism |
|---|---|
| The tree | A process group per child; descendants captured first, then signalled by pid |
| The deadline | Wall-clock watchdog that a paused logical clock never stops |
| The temp dir | `TMPDIR`, `TMP` and `TEMP` point at a private directory removed with the group (INV-BOT-TEMP-1) |
| Orphans | On Linux a cgroup scope and the subreaper flag close the two gaps a snapshot leaves |
| Evidence | A `CleanupReceipt` and a `Containment` on the outcome |

The private directory sits under `lgwks-tmp-<16 hex>`, one root per supervising process. The kernel drops the lock on the root's `.owner` file when the process dies. The next supervisor then sweeps that dead root.

A spec that sets `TMPDIR`, or clears the environment, gets nothing injected. The caller's choice wins (`crates/lgwks-bot/src/rt/supervise/scratch.rs`, ADR 0077).

## Read the outcome

`TaskOutcome` is `#[non_exhaustive]` and each state asks for a different response.

| Outcome | It means | You do |
|---|---|---|
| `Completed` | The body returned | Read the output |
| `Failed { status, .. }` | The process ended non-zero and the supervisor did not stop it | Read `exit_status()`; `None` means the wait itself failed |
| `Cancelled` | The token was cancelled; the supervisor stopped it | Nothing; expected |
| `Aborted` | The runtime dropped the body | Fix the body: it ignored its token |
| `Panicked` | The body panicked | Read `panic_message()` |
| `CleanupSettled` | A retained cleanup later proved the group absent | Settle your own record |

Then read `cleanup()`. Only `CleanupConfirmed` answers `is_complete()`.

| Receipt | Meaning |
|---|---|
| `CleanupConfirmed` | The group is gone and every captured descendant stopped |
| `CleanupPending` | `SIGKILL` was delivered but removal was not proved |
| `CleanupFailed` | The operating system refused the termination |
| `CleanupSurvivors { survivors }` | The group is gone; these captured pids still run |

`containment()` says how much was looked at: `mechanism()`, `captured()`, `signalled()`, `survivors()` and `residual_risk()`. A `ResidualRisk` of `TableUnreadable`, `CaptureTruncated`, `LeaderExited` or `DescriptorHeld` means the report does not account for the whole tree, and `Containment::is_complete` is then false. `LeaderExited` is the common case for a process that simply finished.

## Not claimed

- A descendant that calls `setsid` after the last capture, or one orphaned before the cleanup, is out of reach on a platform with no cgroup or subreaper. A Windows job object is pending (#263).
- A child that writes to a fixed temp path, or sets its own `TMPDIR` after it starts, writes where it says.
- macOS `xcrun` ignores `TMPDIR`; abandoned `xcrun_db-` files are swept by age (ADR 0079).

## Confinement

Confinement is an exec wrapper chosen in the spec: `Confinement::deny_beneath`, `LandlockRules` and `SandboxProfile` (`crates/lgwks-bot/src/rt/process/confine.rs`). A platform that cannot honour the request refuses it before the fork, and a Landlock allowlist past 4,096 rules or 1 MiB of path bytes is refused before the fork (`docs/adr/0072-a-landlock-allowlist-holds-4096-rules.md`). Capability gating elsewhere in these crates is auditable authority, not a sandbox.

## Traps

- Do not wrap a supervised child in `sh -c` with a built string. Pass the program and its arguments.
- Do not read `Completed` as a success of the command: a non-zero exit is `Failed`.
- Do not leave a child running at the end of a test. The census fails it (`skills/lgwks-test-on-the-rails/SKILL.md`).

## Before you say done

- [ ] No `Command::spawn`, `std::thread::spawn` or `tokio::spawn` in the change.
- [ ] Every outcome arm is matched by name, and `CleanupConfirmed` is checked where it matters.
- [ ] A test kills a child past its deadline and finds its temp directory gone.
