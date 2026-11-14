# ADR 0049: a run carries its store, definition and effect policy on its scope

Status: **accepted**, 2026-11-13, for #909 (extends ADR 0045).

Stakeholders: every flow that calls `remember`, `act`, `send`, `await approval` or a gated
outside word; every host that installs a store or a custom `EffectPolicy`.
Concerns: run state lost across a spawn, read back as "none".
Requirements: R9 (`docs/requirements/d-correctness.md`), INV-BOT-195
Viewpoint: the run-state viewpoint, one scope.
Elements and relations: `script::Scope` (carries the store, definition and policy),
`script::run_store` (reads them), `task::execute` (attaches them), `script::approval_policy`
(captures the policy).

## Context

A task-local does not cross a spawn, and `rt::task::join_all_bounded` spawns every input.
Three pieces of a run's state still lived in task-locals after ADR 0045 moved its authority:
the durable store, the definition identity it records under, and the flow's effect policy.
A step polled on a spawned task read none of them.

- Its `act` skipped `admit_external_handoff` and ran an external effect over an ephemeral store.
- Its `remember` ran the body unrecorded and could not replay it.
- Its `record_refusal` recorded nothing.
- Its `gate_approval` fell back to `DefaultPolicy`, so a stricter policy stopped applying.

## Decision

- `ScopeInner` carries `records`, `definition` and `effect_policy`. They are inherited by
  every descended scope through the existing clone, so a clone still copies pointers.
- `Scope::hosted` takes the authority, the store and the definition. `task::execute` passes
  `self.records(run)` and `Arc::new(recorded_definition)`.
- `run_store::installed` and `installed_definition` read the scope. The `RECORDS` and
  `DEFINITION` task-locals and `run_store::within` are removed.
- `Scope::rooted` captures the policy installed by `with_effect_policy`, and `gate_approval`
  reads the scope first, then the task-local, then `DefaultPolicy`. The public
  `with_effect_policy` signature does not change.

## Alternatives rejected

- **Re-install the task-locals in every spawn helper.** Every spawn site, ours and a
  consumer's, must remember it; one that forgets fails silently. ADR 0045 rejected the same
  shape for the authority, and the bug moves rather than closing.
- **Make every word `Send` and pass the state as an argument.** It changes the signature of
  every word and every consumer, and the state returns to being a parameter each caller must
  thread by hand.
- **Read the policy from the task-local inside `gate_approval` alone.** The task-local does
  not cross a spawn, which is the whole defect.

## Consequences

- A step on a spawned task records, replays and gates under its run, exactly as the body does.
- A root scope built outside `with_effect_policy` still falls back to the task-local, then to
  `DefaultPolicy`; the public surface is unchanged.
- `Scope` grows by three pointer-sized fields; a clone copies them, and the scope remains
  cheap to clone.
- The run-state attachment lives in `script::scope::run_state`, beside the scope's path and
  cancellation, because it is a second responsibility and `scope.rs` was at the size law.

## Correspondence

Refines ADR 0045, which moved the run's authority first and named the same spawn boundary.
No module is removed; `script::scope::run_state` is added as a submodule of `script::scope`.

## Verification

- `tests/it/sim_scope_run_state.rs` drives a spawned `act` over an ephemeral store, a spawned
  `remember` across a resume, and a spawned `act` under a custom policy that gates a local
  effect.
