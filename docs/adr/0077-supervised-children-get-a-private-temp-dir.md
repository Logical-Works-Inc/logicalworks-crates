# ADR 0077: supervised children get a private temp dir

Status: **accepted**, 2026-11-23.

Stakeholders: the Director; every repository that starts a child through `lgwks_bot`; the build Mac.
Concerns: no supervised child leaves files in temp, on any path, and the disk never fills from them.
Requirements: INV-BOT-TEMP-1.
Viewpoint: the `lgwks_bot` process start, as `Supervisor::run_process` and `spawn_process` reach it.
Elements and relations: `rt/supervise/scratch.rs`, `start.rs`, `group.rs`, `run.rs`, `drive.rs`.

## Context

On 2026-11-23 the build Mac held 110,965 `xcrun_db-*` files, 86 GiB, in the Darwin user temp dir.
Its disk went past the 256 GiB cap twice in one day. A fault lane ran `git` and `cc` under a file-size
limit. The `/usr/bin` shims call `xcrun`, which writes its cache through a `mkstemp` file and renames
it. The limit cut the write, so the temp file stayed. `xcrun` writes there whatever `TMPDIR` says.
A child killed by a deadline also leaves its own temp files. Every repository spawns through this
crate, so the crate owns the fix.

## Decision

1. **A private dir per child.** `start` makes `lgwks-tmp-<pid>-<n>/<child>` and sets `TMPDIR`, `TMP`
   and `TEMP` to it. The process-group guard owns it, and it is removed after the group is killed.
2. **The caller's choice wins.** A spec that sets `TMPDIR` or clears the environment gets nothing.
3. **Crash-safe roots.** A root holds an `.owner` file that its process locks. The kernel drops the
   lock on `SIGKILL`. A root a process opens removes each sibling whose lock it can take and whose
   owner file is 10 s idle, at most once per 2 s: a younger root is still being opened. The last
   child's drop removes the root, lock still held (amended 2026-11-23, #1128).
4. **Writers that ignore `TMPDIR`.** While a group runs and when it ends, at most once each 2 s, a
   sweep removes `xcrun_db-XXXXXXXX` files idle for 10 s or more. It reads the own temp dir and
   `getconf DARWIN_USER_TEMP_DIR`.

## Alternatives rejected

- **A sweep in `logical_ci` only.** Every other repository would still leak. Two sweeps would drift.
- **Move the Darwin user dir per child.** `xcrun` reads `confstr`, which no variable moves.
- **Sweep only when a group ends.** One long lane leaked about 1,500 files every few seconds.

## Consequences

- A child that writes to a fixed path such as `/tmp/x` still writes there. That is not claimed.
- Each start makes one directory, and each end removes it.

## Correspondence

Closes the leak traced to the `logical_ci` fault lane on 2026-11-23.

## Verification

`sim_scratch_leaves_nothing` drives a real `sh` through `run_process` over 48 seeds. Each child
exits, fails, hits its deadline or leaves a background writer. It failed before the change: `TMPDIR`
was the shared temp dir. `sim_scratch_sweeps_dead_roots_and_xcrun_leftovers` and the in-crate
`supervise::sim_scratch` sims hold the sweep against reference models.
