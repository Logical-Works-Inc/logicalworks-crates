# ADR 0036: a moved lockfile builds on main's nearest entry

Status: **accepted**, 2026-11-12. The Director's orders were "WHY ARE THESE TESTS SO LONG" and
"HOW MANY TIMES HAVE I FUCKING SAID UNDER 90 SECOND TESTING AND SUB 5 MINUTE CI".

Stakeholders: the Director; a contributor whose pull request adds or moves a registry package.
Concerns: a pull request that compiles every dependency again because one package moved.
Requirements: R13
Viewpoint: the gate viewpoint, one row per CI job and the cache entry it restores.
Elements and relations: the `hosted-rust` action, the rust-cache entry it keys and saves, and the
read-only `actions/cache/restore` step after it.

## Context

ADR 0028 keyed the dependency build by what it is compiled from, so a restore is exact or empty.
Its stated cost was that a change to the registry packages compiles cold. PR #923 added loom to
`Cargo.lock` for ADR 0030. Every test passed, and run 43311128730 walled at 312 s against its
300 s. Workspace slice 6 found no entry for `workspace-c5b4826cf677` and built for 2 m 34 s.
On main the same slice restored `workspace-7df5f3e6982e` and built for 44 s.

## Decision

When a pull request's job misses its exact key, `hosted-rust` restores main's newest entry for
the job's `cache-key` group after rust-cache has run. It names the six paths rust-cache saves,
in its order, so the entry's version matches, and restores by the prefix
`v0-rust-<group>-`. rust-cache's pre-clean of a missed key has already run, so nothing restored
is deleted. Cargo's fingerprints then rebuild only the packages the lockfile moved. The step only
reads; rust-cache stays the one owner that saves, from main alone.

## Alternatives rejected

- **Restore by rust-cache's own restore key.** Its partial match deletes every artifact older
  than a week, which is nearly all of them (ADR 0028).
- **A fork of rust-cache without the pre-clean.** It is a second cache owner, and it would
  write as well as read.
- **More workspace slices.** Every slice compiles the same dependencies, so a cold build costs
  each slice the same 2 m 34 s however many there are.

## Consequences

- A pull request that moves a registry package builds on main's last dependency build and
  compiles only what moved. A change that moves nothing still restores whole, as before.
- A group's prefix also matches a longer group's key that begins with it. Today the only such
  pair, `workspace` and `workspace-core`, saves different paths, so the versions never match.
- A restore that finds nothing, or an entry of another version, leaves the job cold, as before.

## Correspondence

Refines ADR 0028's first decision and supersedes its rejected partial-key alternative. Described in
`docs/workflow/ci-layout.md`.

## Verification

- `check_gate_parity::workflow_calls::tests::a_moved_lockfile_restores_mains_nearest_build`
  holds the committed action, and names a restore that is gone, runs on a hit or on main,
  names other paths, reads another group's prefix, or saves a second cache.
- Each job that misses prints `base build <key>` with the entry it laid down, or `none`.
- The `Build and workspace tests` row's run-wall step fails a run past 300 s; #923's next run
  holds it with Cargo.lock still moved.
