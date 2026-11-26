# ADR 0087: a lifeline releases what a dead owner held

Status: **accepted**, 2026-11-26.

Stakeholders: the Director; every program that starts a supervised child or attaches a RAM disk.
Concerns: a process group or RAM disk outlives the owner that a SIGKILL ended, and holds the host's memory.
Requirements: INV-STD-LIFELINE-1, INV-BOT-LIFELINE-1.
Viewpoint: the process-lifetime view of `lgwks_std::process::lifeline` and its callers.
Elements and relations: `hold_group`, `hold_device`, `hold_run` and `LifelineHold` in `lgwks_std`; `start.rs` holds
each supervised group; `ram_disk.rs` holds each RAM disk; the Linux census passes over a lifeline.

## Context

On 2026-11-26 a 24 GB Mac sat at memory pressure "warn" for hours (#1160). 14 orphaned processes and two RAM disks
held it. The mutants gate had SIGKILLed test binaries at their timeout. A guard's `Drop` runs only while its owner runs
code, and macOS has no `PR_SET_PDEATHSIG` and no subreaper. A RAM disk SIGKILLed between `hdiutil attach` and its
format had no volume name, so nothing could find it again.

## Decision

1. Each process gets one lifeline: `/bin/sh` started with `argv[0]` `lgwks-lifeline`, leading its own process group.
   It reads hold lines on a pipe. Only the owner holds the write end, close-on-exec.
2. A hold writes `+ id kind value` before it returns, and a drop writes `- id`. At end of file the lifeline kills each
   held group (`kill -9 -<pgid>`), then detaches each held device, then runs each held release program, then exits.
3. The owner dies by any path, and the kernel closes the pipe. So the release never needs the owner's code.
4. A lifeline that died is started again at the next hold or drop, and every open hold is replayed to it first. The
   old one is SIGKILLed before its pipe closes, so a restart releases nothing.
5. `start.rs` holds each supervised group under the start lock and refuses a start it cannot hold. `ram_disk.rs`
   holds the device as soon as `hdiutil` names it.
6. Values are limited to letters, digits and `_ . / + -`. Holds are bounded at `MAX_HOLDS` (65,536); past that, a hold
   is refused as `QuotaExceeded`.

## Alternatives rejected

- **`PR_SET_PDEATHSIG`.** Linux only, it needs `unsafe` `pre_exec`, it fires when the spawning thread exits (a
  runtime's pool threads do), and it reaches only the direct child, not the group.
- **A kqueue `NOTE_EXIT` reaper binary.** macOS only, and a consumer of the crates ships no helper binary.
- **A ledger swept at the next start only.** That release is unbounded in time: the Mac held its leaks for hours
  because no start came. The ledger stays in #1160 for the case where the lifeline dies too.

## Consequences

- Each process that holds anything runs one more `sh` for its life. An idle `/bin/sh` in `read` measured 2,208 KiB
  resident on macOS 27 (`ps -o rss`, 2026-11-26).
- A hold is one pipe write under a process-wide lock.
- An unsupervised fork in the window between `pipe` and `FD_CLOEXEC` on macOS can keep the pipe open. The release
  then waits until that child exits.
- A release program that hangs keeps its lifeline alive.
- The script is one ordinary literal per line. The mutants gate reads Rust a line at a time, so it rewrote the `&&`
  and `$((n+1))` of a multi-line raw string, and each lifeline ended at once with status 2.
  `sim_lifeline_script_runs_what_is_held_at_end_of_file` runs the script itself.
- The lifeline holds none of its owner's streams. On an inherited stderr it kept nextest's capture pipe open after the
  test exited (a leak to nextest), and it stalls a parent that reads the owner's output to its end.
- A parent that SIGKILLs an owner keeps the owner's stdin until it reaps the owner. `Child::wait` closes stdin first,
  and an owner that reads that end of file can exit normally and release every hold before the kill lands.
- The shared test volume (ADR 0026) no longer stays until the host restarts. Each `churn-slot` holds it through a
  locked file in `volume-holders/`. The last wrapper to leave schedules `churn-slot --release-shared` 60 s later, and
  the lifeline of one that died runs it at once. The release checks once: it detaches a volume with no live holder
  and no use in 60 s, and else schedules its next check. No thread waits; the delay is a child `sleep`. The next test
  attaches a new volume. The 60 s linger is measured: an attach and APFS format of the volume took 1.98 to 2.15 s on this Mac
  (2026-11-26), so a release at every gap between two tests would cost 2 s each.

## Correspondence

Refines ADR 0026, decision 2: the shared volume is released when idle. Relates to ADR 0081 (the parked-child reaper)
and ADR 0082 (the census). The census kills a lifeline and does not name it. Part of #1160.

## Verification

`sim_lifeline_releases_every_hold_of_a_killed_owner` starts real owners over seeded plans and SIGKILLs some of
their lifelines. Then it SIGKILLs each owner. Every held group must stop and each held release program must run. A
dropped one must not run, and the lifeline must end. `a_supervised_group_dies_with_its_sigkilled_supervisor` does the
same through a real `Supervisor`. `sim_lifeline_refuses_every_value_its_line_cannot_carry` holds the value rule.
`sim_shared_volume_detaches_only_when_unheld_and_idle` drives 128 seeded runs of arrivals, leaves, SIGKILLs and
releases against a model. No detach occurs while a wrapper holds the volume, or before 60 s idle.
