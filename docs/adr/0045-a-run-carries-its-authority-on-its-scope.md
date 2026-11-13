# ADR 0045: a run carries its authority on its scope

Status: **accepted**, 2026-11-13, for #909 step 1.

Stakeholders: every flow that calls `observe`, `act`, `send`, `fetch`, `ask` or a gated outside
word; every host that grants capabilities.
Concerns: an absent authority read as a covering one.
Requirements: R9 (`docs/requirements/d-correctness.md`), INV-BOT-194
Viewpoint: the authority viewpoint, one issuer.
Elements and relations: `script::Scope` (holds the authority), `script::run_store::Authority`
(the check), `task::execute` (the host attaches it), `script::domain` (resolves the registry
through it).

## Context

`Scope::require` read the run's authority from a task-local. A task-local does not cross a
spawn, and a missing one returned `Ok`. So a step on a `join_all_bounded` task, or under a
hand-built `Scope::root`, passed every check, even when the host granted nothing.

## Decision

- The host attaches the run's authority to the root scope (`Scope::hosted`). Every child,
  sibling and clone shares it. The `AUTHORITY` task-local is gone.
- A scope with no authority covers nothing. `require` names every capability it asked for in
  `FlowError::Blocked`. An empty requirement asks for nothing and passes.
- The domain registry is read from the same authority, so a spawned step resolves domains too.

## Alternatives rejected

- **Keep the task-local and refuse on a missing one.** A spawned step under a granting host is
  then refused for a capability it holds. The bug moves; it does not close.
- **Re-install the task-local in every spawn helper.** Every spawn site, ours and a consumer's,
  must remember it. One that forgets fails closed but wrong, and the authority stays ambient.
- **Split `Scope` into hosted and unhosted types now.** It changes the signature of every word
  and every consumer. #909 step 3 already takes the semver break, so the split rides with it.

## Consequences

- A flow run under `Scope::root` that reaches a capability is `Blocked`. Run it under a `Host`.
- A spawned step under a host is checked against that host's grant, as the body is.

## Correspondence

Answers R9 and #909 step 1. No module is added or removed, so CODEBOOK.md is unchanged.

## Verification

- `tests/it/sim_require_closed.rs` sweeps grants, needs, two tenants and three placements against
  a set model, and drives 100, 1,000 and 10,000 spawned steps.
