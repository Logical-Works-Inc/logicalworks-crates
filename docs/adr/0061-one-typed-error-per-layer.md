# ADR 0061: one typed error per layer

Status: **accepted**, 2026-11-15. Closes #990, from epic #984.

Stakeholders: consumers of `lgwks_bot` that match on its errors; `lgwks_bot` maintainers; the Director.
Concerns: a lower layer that cannot be read without an upper one; one error enum that grows a variant for every layer; a retry class that changes as a failure travels.
Requirements: R9
Viewpoint: the layer viewpoint, following one failure from the module that raises it up to the consumer.
Elements and relations: `error::KernelError`, `ecs::EcsError`, `session::SessionError`, `script::FlowError`, `BotError`, `tests/it/core_boundary.rs`.

## Context

`BotError` was one enum for the whole crate. The kernel (clock, journal, gate, effect) named variants that only the world or the session raise, so reading the kernel meant reading the world, and a new variant anywhere changed every layer's match.

## Decision

1. Each layer defines the error it raises, where it raises it: `KernelError` in `error`, `EcsError` in `ecs`, `SessionError` in `session`, `FlowError` in `script`.
2. A layer wraps the layers below it with `From`, so `?` does the wrap and the failure is carried unchanged. Every error has `retry_class` and `dispatch_certainty`, and a wrapper delegates to what it wraps.
3. `BotError` is the app layer's sum of the layer errors, one wrapper variant per layer, `#[non_exhaustive]`, re-exported at the crate root. No `Other(String)` and no alias of the old enum.
4. A lower layer never names an upper one. `core_boundary` reads every source file and refuses a kernel module that names `crate::session`, `crate::ecs`, `crate::spec`, `crate::stability` or `crate::world`, or `super::session` or `super::ecs`.
5. Two kernel types moved down to meet rule 4: `EffectEvidence` to `journal`, and the `CasWrite` verb adapter to `domain::cas`.

## Alternatives rejected

- **Keep one enum and document the layers.** Nothing refuses the import, so the kernel drifts up again.
- **A trait object per layer.** A consumer could no longer match a failure, and the retry class would be a runtime answer.

## Consequences

- A consumer that matched a moved variant on `BotError` now matches `EcsError`, `KernelError` or `SessionError`. That is the one break and it lands in 9.0.0.
- A new layer adds one error type and one `BotError` variant, and no variant in any other layer.

## Correspondence

Refines the error vocabulary of INV-BOT-ERROR-TYPED and relates to ADR 0006 decision 4, which `core_boundary` already holds for the driver.

## Verification

- `crates/lgwks-bot/tests/it/core_boundary.rs`: `a_kernel_error_naming_an_upper_layer_is_refused` and `sim_a_planted_upper_import_is_refused_in_every_kernel_module`.
- `crates/lgwks-bot/tests/it/core_boundary/layer_errors.rs` and `crates/lgwks-bot/src/sim_error.rs`: a failure keeps its retry class through every layer.
