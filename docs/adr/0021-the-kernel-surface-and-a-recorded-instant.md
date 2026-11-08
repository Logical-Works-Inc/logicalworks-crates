# ADR 0021: the kernel surface and a recorded instant

Status: **accepted**, 2026-11-08. Implements #845.

Stakeholders: the Director; a downstream consumer moving its runtime onto `lgwks_bot`.
Concerns: support code with no compatibility promise under a consumer's runtime; a clock value that cannot be recorded.
Requirements: R9, R10
Viewpoint: the consumer's viewpoint, naming items from outside the crate.
Elements and relations: `lgwks_bot::kernel`, which re-exports eight modules; `clock::EpochClock`, which anchors a `Clock`; `clock::RecordedInstant`, which a journal holds.

## Context

`clock`, `skew`, `journal`, `broker`, `dist_lease`, `gate`, `effect` and `retry` are hidden support code (#526), so a consumer building on them has no promise. `Clock` counts from a process-local origin, so its reading means nothing once recorded.

## Decision

1. **`lgwks_bot::kernel` promises the eight modules.** A removal or changed signature there is a major release. A `#[doc(hidden)]` item inside one stays outside the promise.
2. **Each kernel module is a glob of the support module.** One type has two paths, never two definitions.
3. **`RecordedInstant` is nanoseconds since the Unix epoch in a `u64`.** It saturates about the year 2554 and archives as eight little-endian bytes.
4. **`EpochClock` reads the system clock once**, then advances on the monotonic clock. `system_after` never reads earlier than the last record.
5. **`seeded` and `restored` read no real clock**, so one seed and one list of advances give one list of bytes on any host.

## Alternatives rejected

- **Removing `#[doc(hidden)]` from the eight modules.** It works too, but #526's rule that a support path carries no promise would then have eight exceptions scattered through `lib.rs`, where one module states them all.
- **Adding an epoch to `Clock`.** Every deadline would carry a value it must not read, which is what the module's rule against mixing clocks exists to prevent.
- **Extending `skew::WallStamp`.** Its contract is that virtual time never produces one, and a seeded instant must.

## Consequences

- The eight modules can no longer change freely. A glob promises whatever is later added to them, so a new public item there is a promise too.
- Two live hosts' instants carry the skew their system clocks had when anchored. `SkewBound` prices it, and a deadline stays on `Clock`.
- Semver is held by a test that names items by path, not by cargo-semver-checks, so a changed signature on an item the test does not name is caught only in review.

## Correspondence

Sits under the `script!` surface of #526, and beside the record journal of ADR 0016.

## Verification

`tests/it/sim_recorded_instant.rs` holds the bytes, replay and fools over seeds. `tests/it/recorded_instant_cli.rs` drives the shipped binary through a `kill -9`. `tests/it/kernel_surface.rs` names each promised module through its kernel path.
