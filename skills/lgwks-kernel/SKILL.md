---
name: lgwks-kernel
description: Use when building your own durable runtime on lgwks_bot::kernel instead of writing script! flows, or when you need its clock, journal, broker fence, lease, grant gate, effect key or retry class. Names each module, its promise and the shell binaries that try it.
---

# The kernel under your own runtime

`script!` is how a flow is written. `lgwks_bot::kernel` is the promised path to what a host of flows stands on, for a consumer that runs its own loop and wants the parts. If you only write flows, use `skills/lgwks-script-flows/SKILL.md`.

## The promise

Every module is a glob of the support module with the same name, so one type has two paths and one definition. A removal or a changed signature in `kernel` is a major release. A `#[doc(hidden)]` item inside a module stays outside the promise, and so does every other module of the crate (`crates/lgwks-bot/src/kernel.rs`, ADR 0021). Name items through `lgwks_bot::kernel::..` and never through the support path.

## The modules

| Module | Holds | Feature |
|---|---|---|
| `kernel::clock` | The declared `Clock`, `EpochClock`, `RecordedInstant` | |
| `kernel::skew` | Monotonic deadlines, wall timestamps, the bound on two clocks | |
| `kernel::journal` | The effect journal, and `RecordJournal` for a record you define | |
| `kernel::broker` | Which environments a run owns, and the fence on handing one an attempt | |
| `kernel::dist_lease` | The cross-host lease with fencing epochs | |
| `kernel::gate` | Grant sets: admission at build, a proof on each tick | |
| `kernel::effect` | The exact key a settlement is a statement about | |
| `kernel::retry` | Whether a failed attempt may be tried again | |
| `kernel::task` | `Host`, `Task`, `Report`, the run store (`skills/lgwks-durable-runs/SKILL.md`) | `script` |
| `kernel::cap` | Capabilities and the proofs that carry them; what `App::grant` takes | |
| `kernel::sealed` | Records sealed at rest under fresh data keys | `sealed` |

Decisions: `docs/adr/0016-record-journal-for-a-consumers-own-history.md` (the journal), `docs/adr/0017-a-broker-fence-a-generation-does-not-answer.md` (the fence), `docs/effect-kernel.md` (the effect key).

## Try one from the shell

Five binaries drive these parts, so you can see an answer before writing code. Each needs the features shown, and a wrong call prints its usage.

```sh
cargo install lgwks_bot --features wire --bin recorded-instant
cargo install lgwks_bot --features wire --bin record-journal
cargo install lgwks_bot --features wire --bin broker-fence
cargo install lgwks_bot --features script,wire --bin durable-stop
cargo install lgwks_bot --features ecs,wire --bin record-projection
```
From `docs/guides/lgwks-bot/kernel.md`. A seeded `recorded-instant` reads no real time, so one script prints the same bytes on any host.

## The clock

One clock is authoritative for every deadline this crate evaluates, and the declared `Clock` is cheap to clone: every clone shares one counter.

```rust
use std::time::Duration;

use lgwks_bot::kernel::clock::Clock;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let clock = Clock::virtual_at(Duration::ZERO);
    let landed = clock.advance(Duration::from_secs(3))?;
    assert_eq!(landed, Duration::from_secs(3));
    assert_eq!(clock.now(), Duration::from_secs(3));
    Ok(())
}
```
Modelled on the signatures in `crates/lgwks-bot/src/clock.rs`: `Clock::virtual_at(Duration)`, `advance(&self, Duration) -> Result<Duration, ClockError>` and `now() -> Duration`.

| Call | Behaviour |
|---|---|
| `Clock::wall()` | Follows real monotonic time; `advance` answers `ClockError::NotVirtual` |
| `Clock::wall_at(elapsed)` | A wall clock restored to time already spent, for a restart |
| `Clock::virtual_at(elapsed)` | Moves only when a caller advances it |
| `advance(by)` | Saturates at the ceiling and returns where it landed; refuses at the ceiling |

- A `Clock` is process-local and never serialised. A restart restores the remaining duration, not an `Instant`. An `EpochClock` gives a `RecordedInstant` that carries an epoch and can be recorded and restored exactly.
- Pausing logical time never disables the wall watchdog, so a test cannot hang on a process that stopped answering.
- Poll order across worker threads is not made deterministic by a clock. What it fixes is which deadline is eligible.

## Not claimed

- `RecordJournal` is a third store beside the effect journal and the run store, and it does not replicate.
- A recorded instant carries the skew two hosts' clocks had when anchored. A deadline stays on `Clock`.
- No semver checker holds the promise: a test names items by path, so a changed signature on an unnamed item is caught only in review.

## Before you say done

- [ ] Every kernel item is named through `lgwks_bot::kernel`.
- [ ] Time comes from a `Clock` passed in, never `Instant::now()` in a decision.
- [ ] A test drives a virtual clock past a deadline with no real wait.
