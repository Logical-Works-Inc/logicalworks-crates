# ADR 0082: a test is judged by the processes it leaves

Status: **accepted**, 2026-11-23. Extends ADR 0078 from files to processes.

Stakeholders: the Director; every repository whose tests run through `churn-slot`.
Concerns: a test that leaves a process running fails, by name, on Linux.
Requirements: INV-CI-ORPHANS-1.
Viewpoint: the nextest wrapper `churn-slot` and the census in `lgwks_std::process::orphans`.
Elements and relations: `churn-slot` sets the subreaper flag, waits through `reap_until` and judges through
`stop_orphans`; both run the rounds of `sweep` over the kernel.

## Context

ADR 0078 fails a test for the files it leaves. A process it leaves was still unseen: the `lgwks run` watchdog
outlived its run by 10 s, and nine other tests left processes behind (#1128 G2b). A first cut made the wrapper the
subreaper but waited only for its own child. Each ended orphan stayed a zombie, which `kill -0` reports present, so
12 tests that wait for an orphan to go failed.

## Decision

1. On Linux the wrapper sets the subreaper flag before it starts the test.
2. It waits with `reap_until`: every child that ends is reaped at once, the test included, as init does.
3. When the test exits, `stop_orphans` kills, names and reaps each child still held, round by round, because a
   killed parent's children are adopted next. A test that exited zero and left a live process exits 1.
4. A refused flag, a refused wait or a tree still adopting after 64 rounds fails the test as unjudged (125).
5. A child in a group that a live lifeline leads (ADR 0087) is the lifeline's release work. The sweep kills that group
   with one `SIGKILL` and names none of it. Killed by pid, the lifeline left its `rm` to be adopted and named, and 10
   Linux shards of #1165 failed that way. What the lifeline did not release is the ledger's (ADR 0090).

## Alternatives rejected

- **Poll `reap_descendants` beside `try_wait`.** A timer per test, and a zombie still reads as present between polls.
- **A census by process group.** A supervised child takes a group of its own, so it would escape.

## Consequences

- `lgwks_deps` takes `lgwks_std`'s `process` feature on Linux: `rustix` and its two crates.
- macOS keeps no process census: it has no subreaper, and Linux runs every suite (ADR 0042).
- The first full Linux run under the census failed 18 tests by name. The fixes:
  - The runner's watchdog slept its whole 10 s grace after its groups had gone. It now ends with them.
  - `cargo metadata` past its deadline lost only its direct child. Its whole group is now killed.
  - Two test writers forked a `sleep` that outlived their killed shell. They now `exec` it or kill the group.
- A process with `SIGKILL` pending is already ending, so the census does not count it as a survivor.

## Correspondence

Extends ADR 0078. Closes #1128 G2b.

## Verification

`sim_orphan_census_matches_the_model` runs 4,096 seeded process trees against a model of the kernel.
`the_census_fails_a_program_that_leaves_a_live_process` drives the wrapper on Linux.
