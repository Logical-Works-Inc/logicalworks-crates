# Requirements E. The mess

Part of [the product requirements](../../REQUIREMENTS.md), which state how a requirement changes.

## E. The mess

### R12. Human and in-real-life mess is in scope and the register is closed.

> Every class of real-world mess this product claims to survive MUST be named
> in a finite register, and each row MUST be judged handled or declared out of
> scope with a reason. An unlisted class MUST be a defect in the register, not
> a silent pass. A release MUST NOT claim readiness with a row unjudged.

**Invariants.** The register is finite and closed for a given release. A row's
judgment names its evidence class. Adding a class is a supersession of this
requirement's register, not a quiet edit.

**Callers.** Every task that touches a human or a real system.

**The register**, as it stands. Seventeen rows, of which thirteen have evidence.

| # | Class | Judgment | Evidence |
|---|---|---|---|
| M1 | The request may or may not have landed before the process died | handled | `tests/it/durable_dispatch.rs`, unknown-outcome barrier |
| M2 | The record itself cannot be read | handled | `tests/it/durable_dispatch.rs`, read failure is refusal |
| M3 | An event returns, or a payload is equal but the event is new | handled | `tests/it/durable_dispatch.rs`, identity split |
| M4 | A spawned process left orphans | partial | `tests/it/process_ownership.rs` (T19/T20), `tests/it/process_escape.rs` (T21, Unix); a descendant orphaned to init is uncontained, and no escape is driven on Windows |
| M5 | A locator resolved to the wrong frame or wrong kind | handled | `tests/it/locator_eligibility.rs` |
| M6 | A contradictory outcome arrives after settlement | handled | `tests/it/durable_dispatch.rs`, refused |
| M7 | A retrying intermediary re-sends a non-idempotent effect | handled | `tests/it/retry_proxy.rs` drives a real loopback proxy; `tests/it/sim_retry_proxy.rs` |
| M8 | The target is gone, not moved | unjudged | studied in `docs/frontier.md`; false-heal 40.5 to 57.1 per cent on removed elements, and the decoy and drift score ranges overlap so no threshold separates them |
| M9 | A dialog or another application stole focus mid-flow | out of scope, unreasoned | no evidence |
| M10 | A file was half-written when read | handled | `tests/it/stability.rs`, `tests/it/sim_stability.rs` |
| M11 | Clock skew between two hosts | handled | `tests/it/skew.rs`, `tests/it/sim_skew.rs` |
| M12 | A credential expired mid-run | handled | `tests/it/credential.rs`, `tests/it/sim_credential.rs` |
| M13 | Locale and format shifts | handled, macOS | `lgwks_std` `tests/it/sim_locale.rs` |
| M14 | Concurrent editors on one record | handled | `tests/it/cas.rs`, `tests/it/sim_cas.rs` |
| M15 | Software-update drift in an observation surface | unjudged | no evidence |
| M16 | The operator cancelled halfway through | partial | `tests/it/sim_cancel_under_load.rs`; supervised processes under a stop at tens of processes, not 1,000 |
| M17 | The accessibility tree mutated during a read | unjudged | no evidence |

**Falsifier.** A release that claims readiness with any row unjudged, or with a
row judged `handled` on evidence of class `unmeasured`.

**Status.** `unmet`, measured only on the thirteen rows with evidence. Closing M8, M9,
M15 and M17 is most of the remaining product work.
