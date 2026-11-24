---
name: lgwks-stay-on-the-rails
description: Use before hand-rolling spawn, retry, deadline, channel, temp-dir, cleanup or tenant code in a LogicalWorks crate, or before adding a dependency. Names what the runtime already owns, the calls that are refused, and which bug classes of issue 1128 are proven closed and which are open.
---

# Stay on the rails

The runtime holds AI-written code to rails as the borrow checker holds memory. It makes the small calls (spawn, retry, deadline, temp dir, cleanup, tenant, trust) so your code does not. When you reach for one of them, use its owner.

## What never to hand-roll

| You are about to write | Use instead | Skill |
|---|---|---|
| `tokio::spawn`, `std::thread::spawn` | A `script!` word (`each`, `together`, `graph`), or the supervisor's `spawn` | `skills/lgwks-script-flows/SKILL.md` |
| `Command::spawn`, `Command::output` | The `Supervisor`: `run_process`, `spawn_process` | `skills/lgwks-supervise-processes/SKILL.md` |
| A retry loop, a backoff, a `sleep` | `retry up to N times, waiting D:` | script-flows |
| A timeout around a call | `within D:` | script-flows |
| An unbounded channel | `rt::sync::mpsc::channel(capacity)`, never `unbounded_channel` | this page |
| A temp directory and its cleanup | The private `TMPDIR` the supervisor gives each child | supervise-processes |
| A run-once guard, a dedup table | A request key and `remember` | `skills/lgwks-durable-runs/SKILL.md` |
| A tenant string in a cache key | `Tenant`, carried by `Scope` | script-flows |
| A clock read in a decision | A `Clock` passed in | `skills/lgwks-kernel/SKILL.md` |
| An HTTP listener | `serve` or `App` | `skills/lgwks-serve-an-app/SKILL.md` |
| A new `Cargo.toml` edge | The owner in the replacement map | `skills/lgwks-std-first/SKILL.md` |

The rt layer is `lgwks_bot::rt`, whose modules are outside the promise. Name only what a skill above names, and pin an exact version if you do.

## Code that is refused

- `unwrap`, `expect`, `panic!`, and their synonyms `abort`, `panic_any`, `resume_unwind` and `catch_unwind`, outside tests.
- `let _ =`, `.ok();` and a handler that drops an error. Log it or return it.
- A catch-all error variant such as `Other(String)`, and an opaque error in a library. Write an enum with one variant per failure, and match it.
- A fail-open default: `None => Ok(())`, `_ => true`. An unknown case denies.
- A lock held across an `.await` (clippy `await_holding_lock`).
- `#[allow]`, `#[expect(dead_code)]`, `#[ignore]`, a deferred-work marker, commented-out code.
- A `pub` item with no caller outside its crate. Start private.
- A new direct dependency. `serde` is reached as `lgwks_std::json`, with `#[serde(crate = "lgwks_std::json::serde")]` on a derive.

A gate that refuses your code is right. Fix the code; never edit a lint, a gate or a test to pass.

## Bounds, always

Every queue, cache, buffer and fan-out carries a number and an overflow answer: reject, shed or push back. The words do it for you: `each` is bounded by the machine, `whenever` by `buffering N` with a declared policy, `serve` by its in-flight ceiling, `retry` by attempts and the run's retry budget. A bound that never binds is no bound.

## Issue 1128: the bug classes the rails rule out

The runtime owns the cleanup so a leak cannot start. Each row names its proof in the tree:

| Class | State | Proof |
|---|---|---|
| G1: a finished run keeps heap | Closed by a test | ADR 0080, 3,072 seeded runs in `sim_heap_a_finished_run_returns_to_its_baseline` |
| G2: a child or a test leaves temp files | Closed by the private `TMPDIR` and the per-test census | ADR 0077, ADR 0078 |
| G2b: a test leaves a live process | Closed on Linux only | ADR 0082; macOS has no subreaper |
| A child that writes to a fixed temp path | Open, stated as not claimed | ADR 0077 |
| A descendant that escapes by `setsid`, or is orphaned before cleanup | Open off Linux's cgroup and subreaper; Windows job object pending | `crates/lgwks-bot/src/rt/supervise/outcome.rs` |
| A tool that ignores `TMPDIR` (`xcrun`) | Mitigated by an age sweep, not closed | ADR 0079 |

The heap counter's own dependency is tracked as gap 1133. Issue 1128 holds every other class and its state: read it before you claim one is closed.

## Traps

- A passing test is not proof of no leak. The census and the heap baseline are the proof.
- A gap in these crates is an issue to file, not a private copy to keep.
- Capability gating is auditable authority, not a sandbox. In-process code can always dial out.
- A `Tenant` partitions a host. It does not authenticate anyone.

## Before you say done

- [ ] No spawn, no unbounded channel, no raw `Command` in the change.
- [ ] Every error is a typed variant, and no `unwrap` outside tests.
- [ ] Every new queue or cache names its bound and its overflow answer.
- [ ] The dependency list is unchanged.
