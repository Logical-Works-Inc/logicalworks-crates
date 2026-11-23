# ADR 0079: the shared Darwin temp dir is scanned once per machine

Status: **accepted**, 2026-11-23. Amends ADR 0077, decision 4.

Stakeholders: the Director; every macOS program that starts many children through `lgwks_bot`.
Concerns: the temp sweeps cost a bounded amount per machine, not per process.
Requirements: INV-BOT-TEMP-1.
Viewpoint: the sweeps in `rt/supervise/scratch.rs`.
Elements and relations: `may_scan` and `claim` gate `sweep_if_due` and `process_root`.

## Context

In 10.0.1 every process scanned the Darwin user temp dir at its first spawn and every 2 s after.
That dir is shared by every tool on the machine. On 2026-11-23 it held 30,145 entries, almost none
from `lgwks_bot`. logical_ci starts thousands of short processes, and each one paid a full scan. Its
ownership lane went from 17 s to 130 s, and its suite ran past a 10 min job (#1130). Linux was
unchanged: 18.07 s on 10.0.0, 18.34 s on 10.0.1.

## Decision

1. Each scan of the shared dir needs a claim on a stamp file there: `.lgwks-sweep-files` for idle
   `xcrun_db-*` files, `.lgwks-sweep-roots` for dead roots.
2. A process wins when it creates the stamp, or locks it and the last win is 2 s old or more, or
   more than 2 s ahead (a clock stepped back). A win sets the stamp's time. A held lock loses.
3. A process's own private temp dir is scanned as before.

## Alternatives rejected

- **A longer interval.** Each new process would still scan at its first spawn.
- **Drop the running sweep.** One long lane leaked about 1,500 files every few seconds (ADR 0077).

## Consequences

- Two stamp files stay in the shared dir. They are reused and never grow.
- A sweep can be late by one interval when the winner dies before it scans.

## Correspondence

Fixes the macOS slowdown reported on logical_ci PR #536.

## Verification

`sim_scratch_machine_claim_matches_the_model` runs 2,000 seeds with contended locks and clock steps
against a model written from the rule.
