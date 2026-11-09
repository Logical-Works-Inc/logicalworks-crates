# Guidance runner specification: Workstream C

Part of [Guidance runner specification](guidance-runner-spec.md).

## Workstream C: `lgwks_deps` invariant register

The workspace already carries an authored, reviewed, refusal-gated register for
dependency edges in `contract/APPROVED.toml`, enforced by `lgwks-deps check`.
This workstream adds a second register of the same shape for invariants, rather
than inventing a second mechanism.

Delivered:

- `contract/INVARIANTS.toml`: a register of declared invariants. Each entry
  carries `id`, `statement`, `scope` (which crate or module it binds), `owner`,
  `enforced_by` (a test path or a lint name), `approved_by`, `approved_on`, and
  `review`.
- A parser and validator for that register in `crates/lgwks-deps`, reusing the
  existing register machinery.
- Refusal rules, each with a distinct diagnostic:
  - an invariant with no `enforced_by`
  - an `enforced_by` naming a path that does not exist
  - an invariant whose `scope` names a crate that is not in the workspace
  - a duplicate `id`
  - a malformed `id` (the `INV-<SCOPE>-<SLUG>` convention already used in this
    workspace)
- A `lgwks-deps invariants [path]` subcommand that reports the register and its
  refusals, exiting non-zero on any refusal.
- `check` gains the invariant register as a second input, so
  `lgwks-deps check .` refuses both an unowned dependency edge and a declared
  invariant with no enforcement.

Requirements:

- The existing `check` behaviour and exit codes do not change for a repository
  with no invariant register. The register is optional; its absence is not a
  failure.
- No new dependencies.

Acceptance: a fixture register per refusal rule, a test that a valid register
passes, a test that a missing register is not a failure, and a test that
`check` reports both registers in one run.

## Out of scope for this stack

Named so they are not silently dropped, and so a later reader knows they were
considered.

- ~~The interface model (element entities, recognition-vector components,
  acquisition systems) on the ECS substrate.~~ **Started**: Workstream A landed
  `lgwks_std::similarity`, so the element entities and the recognition-vector
  components are now in `crates/lgwks-bot/src/interface.rs`, with the locator
  ladder. The acquisition systems on the ECS substrate remain open and are
  sequenced in `docs/general-bot-fold.md`.
- A synchronous path for the four verb traits. It is an API decision for the
  maintainer, not an implementation task, because it touches the published
  surface of `lgwks_bot` and the one-path policy.
- Durable or resumable sessions. The `Journal` seam is the insertion point.
- Any model-backed resolver.
- Any change to `xpress-runner`, which is out of scope for this workspace
  entirely.

## Constraints binding all three workstreams

- Do not bump any crate version. Do not edit `CHANGELOG.md`. Release is a
  separate step.
- Do not edit the workspace `Cargo.toml` and do not add a dependency.
- Every `#[allow(..)]` and `#[expect(..)]` carries `reason = "..."`.
- A crate that does not compile is a refused change. `cargo clippy
  --workspace --all-targets --locked -- -D warnings` and
  `cargo fmt --all -- --check` must both be clean.
- Every public item is documented. `missing_docs` is denied workspace-wide.
