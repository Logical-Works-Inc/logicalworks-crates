# ADR 0030: a loom model of the v7 generator

Status: **accepted**, 2026-11-12. The Director's order was "I DO NOT FUCKING UNDERSTAND WHY YOU
MENTION BLOCKERS YOU CAN JUST FUCKING FIX."

Stakeholders: the Director; every consumer that keys records by `Uuid::new_v7`; logical_ci's A02 verdict.
Concerns: an ordering claim about threads that no test explored; a verdict that stayed UNMEASURED.
Requirements: R13
Viewpoint: the test viewpoint, one row per obligation and the model that discharges it.
Elements and relations: `lgwks_std::id::v7`, the new `id::loom_v7_tests` model, the `loom` dev edge under `cfg(loom)`, logical_ci's `ci/property-and-loom`.

## Context

ADR 0029 made the logical_ci verdict run. On 7fa13b2b its A02 family was UNMEASURED: the
property tests passed, and `ci/property-and-loom` found no `#[cfg(loom)]` model. `v7::new_v7`
reads the clock and the entropy before it takes the process-wide lock, and INV-ID-2 says the
issue order is still the sort order. The seeded sims in `id::sim_v7` drive the ordering rule
on one thread. None explores two threads racing for the lock.

## Decision

1. **loom models the lock protocol.** `id::loom_v7_tests` runs `new_v7`'s steps under loom's `Mutex`
   on two threads. Each thread reads its clock before it locks, and one clock is a millisecond
   earlier. Over every interleaving loom drives, both issue, the identifiers increase in lock
   order, and a later lock with an earlier clock keeps the last stamp.
2. **loom is a dev edge of `lgwks_std` under `cfg(loom)` only.** `lgwks_std` owns it in
   `contract/APPROVED.toml` like its other test-only edges (#924). `cfg(loom)` joins the workspace's
   check-cfg list. A build without `--cfg loom` resolves loom in `Cargo.lock` and compiles none of it.

## Alternatives rejected

- **A seeded sweep of thread schedules.** `lgwks_std::seeded` draws inputs; it cannot enumerate
  the orders a scheduler can pick, so a passing sweep proves only the orders it happened to run.
- **Removing A02 from the tenant's gates.** That turns a missing proof into a narrower claim.

## Consequences

- The model covers the protocol around the `static` lock, not the `static` itself, which
  loom cannot replace. `new_v7` and the model share `V7Generator::issue_at`.
- loom 0.7.3 adds 9 packages to `Cargo.lock` (loom, generator, tracing-log, windows-*). No
  build compiles them unless it sets `--cfg loom`.
- Under `--cfg loom` both models pass in release: `test result: ok. 2 passed; 0 failed`. That
  line is libtest's stdout. logical_ci v0.2.0's `loom_receipt` reads only the lane's stderr
  (`src/property.rs:152`), so the local run on c7109c29 was NO-GO in 74.3 s with "the loom run
  printed no `test result: ok`".

## Correspondence

Refines ADR 0029. Relates to ADR 0010, the estate UUID whose v7 order this model checks.

## Verification

- `loom_the_lock_order_is_the_issue_order_whatever_the_clock_each_thread_read`, run by
  logical_ci's `ci/property-and-loom` on every `logical_ci` job.
- `deps-register` holds the loom edge to its approval.
