# lgwks_bot::kernel: the parts under a flow

`script!` is how a flow is written, and `lgwks_bot::kernel` is the promised path to what a host of flows stands on.

Each of its eight modules is a glob of the support module with the same name, so one type has two paths and one definition. A removal or a changed signature here is a major release, and a `#[doc(hidden)]` item inside a module stays outside the promise ([ADR 0021](../../adr/0021-the-kernel-surface-and-a-recorded-instant.md)).

| Module | Holds | Decision |
|---|---|---|
| `kernel::clock` | The declared `Clock`, `EpochClock` and `RecordedInstant` | [ADR 0021](../../adr/0021-the-kernel-surface-and-a-recorded-instant.md) |
| `kernel::skew` | Monotonic deadlines, wall timestamps and the bound on two clocks | ADR 0021 |
| `kernel::journal` | The effect journal, and `RecordJournal` for a record you define | [ADR 0016](../../adr/0016-record-journal-for-a-consumers-own-history.md) |
| `kernel::broker` | Which environments a run owns, and the fence on handing one an attempt | [ADR 0017](../../adr/0017-a-broker-fence-a-generation-does-not-answer.md) |
| `kernel::dist_lease` | The cross-host lease with fencing epochs | ADR 0021 |
| `kernel::gate` | Grant sets: admission at build, and a proof on each tick | ADR 0021 |
| `kernel::effect` | The exact key a settlement is a statement about | [`effect-kernel.md`](../../effect-kernel.md) |
| `kernel::retry` | Whether a failed attempt may be tried again | ADR 0021 |

## Try each one from the shell

Five binaries drive these parts, so you can see an answer before writing code. Each needs the features below, and a wrong call prints its usage.

```sh
cargo install lgwks_bot --features wire --bin recorded-instant
cargo install lgwks_bot --features wire --bin record-journal
cargo install lgwks_bot --features wire --bin broker-fence
cargo install lgwks_bot --features script,wire --bin durable-stop
cargo install lgwks_bot --features ecs,wire --bin record-projection
```
`durable-stop` drives the stop and cursor of [ADR 0019](../../adr/0019-a-stop-and-a-place-a-restart-keeps.md), and `record-projection` the fold of [ADR 0018](../../adr/0018-a-consumers-fold-on-the-ecs-world.md).

## A recorded instant, live

A seeded clock reads no real time, so one script prints the same bytes on any host. Started as `recorded-instant demo.journal 1000`, the binary answers each line it reads:

```text
now
{"ok":true,"nanos":500}
advance 5
{"ok":true,"nanos":505}
record
{"ok":true,"nanos":505,"sequence":1}
```
Reopen with no seed and the clock is live, yet it never reads before the journal's last record, and a caller cannot advance it. `advance 1` there answers with the refusal `not_virtual`.

## What it does not do

- A recorded instant carries the skew two hosts' system clocks had when anchored. A deadline stays on `Clock`.
- `RecordJournal` is a third store beside the effect journal and the run store, and it holds what you define. It does not replicate.
- A test holds the promise by naming items by path, `crates/lgwks-bot/tests/it/kernel_surface.rs`. There is no semver checker, so a changed signature on an unnamed item is caught only in review.
