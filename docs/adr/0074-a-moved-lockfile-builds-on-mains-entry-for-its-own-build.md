# ADR 0074: a moved lockfile builds on main's entry for its own build

Status: **accepted**, 2026-11-22.

Stakeholders: the Director; a contributor whose pull request moves `Cargo.lock`.
Concerns: a pull request that compiles every dependency again although main holds its build.
Requirements: R13
Viewpoint: the gate viewpoint, one row per CI job and the cache entry it restores.
Elements and relations: the `hosted-rust` action, its deps-key step, and the read-only
`actions/cache/restore` step after rust-cache.

## Context

ADR 0036 restores main's newest entry of the job's `cache-key` group when a pull request misses
its exact key. One group holds several builds: each job's environment is part of the key. PR #1125
moved `Cargo.lock`. In run 45084656634 every workspace slice restored
`workspace-664f76a10a2b`, which another job saved. The slices compiled all 217 crates, 124 to
146 s against 66 to 79 s on #1121, and the run walled at 310 s. Main held each slice's own entry,
`workspace-464d8c33bf75`.

## Decision

1. **The deps-key step keys two locks with one function.** It keys the job's `Cargo.lock`, and on
   a pull request or merge group also the base commit's `Cargo.lock`, read raw through the
   contents API. Both use the job's compiler, pin, profiles and environment, before rust-cache
   exports its variables.
2. **The base restore looks for that key first.** Its restore keys are
   `v0-rust-<group>-<base key>-`, then `v0-rust-<group>-`.

## Alternatives rejected

- **The group prefix alone.** GitHub returns the newest entry of any job in the group.
- **A depth-1 fetch of the base commit.** It downloads the tree where one file is read.
- **A key script beside the action.** The repository holds no shell sources.

## Consequences

- A pull request that moves `Cargo.lock` builds on main's build of the same job.
- A failed read gives the base key `none`, which matches no entry, and the group fallback holds.
- A pull request that also changes the pin or a profile misses the base key, as it misses the
  build, and falls back.
- Every pull request job makes one API read, bounded at 10 s.

## Correspondence

Refines ADR 0036's decision. Described in `docs/workflow/ci-layout.md`.

## Verification

- `check_gate_parity::workflow_calls::tests::a_moved_lockfile_restores_mains_nearest_build`
  names a base restore that lost its own-build key, its group fallback, or keys the base lock
  with another hash.
- Each job that misses prints `base deps key <key>` and `base build <key>`.
