---
name: lgwks-test-on-the-rails
description: Use when writing or fixing a test in this workspace, or when nextest fails a test that passed its assertions. Covers seeded sim_ simulations, the virtual clock, cargo nextest, and the churn-slot census that fails a test for residue files, live processes or a heap that did not return.
---

# Testing on the rails

A test is judged by its assertions and by what it leaves. A test that passes and leaves a file or a process in place still fails, by name.

## The command

```sh
cargo nextest run --locked -p lgwks_bot --status-level fail --final-status-level fail
cargo nextest run --locked -p lgwks_deps -E 'test(/^agent_start_page::/)' --status-level fail --final-status-level fail
```
Never `cargo test`: the census and the timeouts live in `.config/nextest.toml`. Reproduce one lane with `./scripts/ci-local.sh --lane tests` (`skills/lgwks-red-gate/SKILL.md`).

## What the runner does to every test

| Rule | Mechanism | Source |
|---|---|---|
| A fresh `TMPDIR` per test | `churn-slot` makes `lwc-test-<16 hex>`; each entry left there fails the test and the first eight are named; the census then removes it | ADR 0078 |
| A live process fails the test | On Linux `churn-slot` is the subreaper, reaps during the run, kills and names what remains after; a test that exited zero then exits 1 | ADR 0082 |
| A census it cannot judge fails | A missing base directory exits 125, as does a refused flag or a tree still adopting after 64 rounds | ADR 0078, 0082 |
| No hang | A test with no progress for five 60 s periods is killed and fails | `.config/nextest.toml` |
| A finished run gives back its heap | One binary counts live bytes; 512 seeded runs of each shape (fan-out, retry, deadline, cancel, failure) must end at or below where they began | ADR 0080 |

On macOS the wrapper also holds one of a few machine-wide slots, and no process census runs there because macOS has no subreaper. Linux runs every suite. Not seen: a file written to a fixed path outside `TMPDIR`, and a `setsid` escape.

Heap accounting is the `heap-accounting` feature of the `lgwks_deps` storefront (a counting allocator, so no crate writes `unsafe impl GlobalAlloc`). An app can use it to prove its own flows return the heap.

## Why a passing test fails

- It made a file in temp and did not remove it. Use a directory under the test's `TMPDIR` and drop it.
- It started a process and did not stop it. Start children only through the supervisor (`skills/lgwks-supervise-processes/SKILL.md`), cancel, and wait for the report.
- It forked a `sleep` that outlived its shell. `exec` it, or kill the group.
- Its heap grew across runs: a cycle, a forgotten value or a cache that only grows.

## Simulations

At least half of the tests are `sim_*`, and the `simulation-evidence` lane refuses a change that adds tests below that share. A simulation is a seeded scenario over real code. The harness is `crates/lgwks-bot/tests/sim/mod.rs`.

- One `u64` seed derives everything: the fault schedule (`Faults::for_seed`), the tenant count, the partition windows and every later draw. The generator is xoshiro256 and the hash is FNV-1a, written out, so a lockfile bump cannot change a replay.
- Time is a tick counter that only the scenario moves. The wall clock is never read.
- Every asserted fact goes into a `Trace`, and the trace hashes to a `u64`. The same seed must give the same hash, so a diverging run is a defect even when it asserted green.
- The oracle is an invariant or a reference model, never the code's own output.
- Escalate: 100, 1,000, 10,000, then 100,000 and see what breaks. Include a saboteur that makes a dependency lie, and two tenants on one key.

Name each test for the bug it would catch: `sim_two_tenants_same_key`, `sim_kill_replay_no_double_effect`.

## A virtual clock in your own test

```rust
use std::time::Duration;

use lgwks_bot::kernel::clock::Clock;
use lgwks_bot::script::{Scope, Tenant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let clock = Clock::virtual_at(Duration::ZERO);
    let scope = Scope::with_clock(Tenant::new("acme")?, clock.clone());
    clock.advance(Duration::from_secs(30))?;
    assert_eq!(scope.clock().now(), Duration::from_secs(30));
    Ok(())
}
```
Modelled on `Scope::with_clock(tenant, clock)` and `Scope::clock()` in `crates/lgwks-bot/src/script/scope.rs`, and `Clock::advance` in `crates/lgwks-bot/src/clock.rs`. `HostBuilder::clock(clock)` hands the same clock to a whole host, and advancing it past the default deadline gives the `FlowError::TimedOut` a real overrun gives, with no wait.

## Refused outright

- A sleep in a test, or a sleep in a loop: wait on the clock, a channel or a condition.
- `#[ignore]`, and a test gated away on the platform where it fails.
- `SystemTime::now()` or an unseeded generator in a simulation.
- A bound that never binds: a test must drive past the bound and assert the overflow answer, then recovery.
- A test edited to pass a gate. Fix the code.

## Traps

- A test that passed twice proves nothing. Flaky is a defect: find the race.
- Two cargo runs at once fight over the target directory lock.
- A tool a test runs (`xcrun`) can leak outside any model of your code, which is why the census exists.

## Before you say done

- [ ] The new tests are at least half `sim_*`, and each names its bug.
- [ ] Each failure path asserts its exact variant, not a message.
- [ ] The nextest filter for the crate passes, and the run left no residue.
