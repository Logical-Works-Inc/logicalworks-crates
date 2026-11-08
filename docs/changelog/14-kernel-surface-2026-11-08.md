# lgwks_bot 8.1.0

Released 2026-11-08. A consumer hosting flows gets a supported path and a clock value it can record.

## lgwks_bot 8.1.0 (minor)

- `lgwks_bot::kernel` is the supported path to `clock`, `skew`, `journal`, `broker`, `dist_lease`, `gate`, `effect` and `retry`. Each is the same items as its support path. A removal there is now a major release (ADR 0021).
- `kernel::clock::EpochClock` anchors a clock to the Unix epoch. `system()` reads the system clock once, `system_after(last)` never reads before a record, and `seeded(seed)` and `restored(instant)` read no real clock.
- `kernel::clock::RecordedInstant` is nanoseconds since the Unix epoch, saved as eight little-endian bytes through the journal's `Durable`.
- The `recorded-instant` binary drives the clock and a journal of instants from a shell, so a restart can be tried under a real kill.
- `ClockError` gains `BeforeEpoch`. The enum is `#[non_exhaustive]`, so no existing match breaks.
