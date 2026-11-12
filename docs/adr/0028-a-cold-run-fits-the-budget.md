# ADR 0028: a cold run fits the budget

Status: **accepted**, 2026-11-12. The Director's orders were "HOW MANY TIMES HAVE I FUCKING SAID
UNDER 90 SECOND TESTING AND SUB 5 MINUTE CI" and "WHY IS CRATES SO BLOATED AND WHY IS CACHING SO
SHIT".

Stakeholders: the Director; a contributor whose pull request edits a manifest or the cargo config; the release that bumps every crate.
Concerns: a pull request that compiles every dependency again and passes its 300 s only on a warm cache; a nested app build that holds a shard.
Requirements: R13
Viewpoint: the gate viewpoint, one row per CI job and the cache entry it restores.
Elements and relations: the `hosted-rust` action and the rust-cache entry it keys, the `tests_linux` matrix and its `apps` rows, and the `tests` and `app-builds` lanes of `scripts/gate-lanes.toml`.

## Context

PR #916 bumps lgwks_bot to 8.3.0, adds one `[[bin]]` and adds a `[profile.dev]` table to
`.cargo/config.toml`. Every one of its tests passed, and run 43144663118 still failed. Workspace
slice 10 was cancelled at its 5-minute cap, and the run wall was 324 s.

Two causes stacked. First, rust-cache keys on a hash of `.cargo/config.toml` and every member
manifest. When that hash misses, it restores the nearest entry and deletes each artifact older
than a week. Entries carry artifacts forward unrebuilt, so nearly every artifact went. Each
workspace shard compiled its 186 dependencies again, 141 to 159 s against 42 to 49 s on main.
A local probe with #916's manifests on a warm target rebuilt only `lgwks_std` itself.

Second, slice 10 held `lgwks_e2e` (97 s) and `expanded_app_passes_clippy` (73 s). Each
compiles a scaffolded app's whole dependency tree, one in build mode and one in check mode,
behind one cargo lock. Main's warm run spent 115 s in the slice holding them.

## Decision

1. **The dependency build is keyed by what it is compiled from.** That is `rustc -vV`, the
   toolchain pin, the registry and git packages of `Cargo.lock`, the members' dependency lists
   without their own versions, the root `[profile.*]` tables and the build environment. The
   action turns rust-cache's own hashes off, so a restore is exact or empty, and nothing it
   restored is deleted.
2. **The scaffolded-app journeys run on their own rows.** The workspace lane excludes them. The
   `app-builds` lane runs them, and CI runs it as two `tests_linux` rows, `apps run` and `apps
   clippy`, each building lgwks_deps' `it` binary alone.
3. **The cold floor is the longest shard, so the long ones are cut.** Run 43158681921
   passed every test and walled at 312 s. The storefront consumers, a nested build
   per test behind one lock, go from three shards to six. `lgwks_ast`'s markdown
   guard launched its 1,050 child processes one after another (57.8 s on CI). It
   now runs them in waves of one per core, at most 8, and waits on each child's
   pipe closing instead of a timed park: 16.5 s to 2.3 s locally.

## Alternatives rejected

- **A longer job timeout.** The budget is the Director's 300 s, and the run-wall row holds it.
- **Restoring on a partial key without the pre-clean.** rust-cache has no input for that, and a
  fork of the action is a second cache owner.
- **One `apps` row for both families.** The run and the check share no artifacts, so the two
  builds serialize on the lock for no reuse.
- **Pointing the apps at the suite's target directory.** Their feature set differs from the
  workspace's, so every shared dependency has another hash and rebuilds anyway.

## Consequences

- A pull request that changes a member's version, a binary target or a config comment restores
  main's dependency build whole. A change to the registry packages compiles cold, as before.
- A member's feature change that moves no package keeps the key, so cargo rebuilds what changed
  on each run until the lockfile next moves. Cargo's own fingerprints keep that correct.
- The first run after this lands builds every entry cold, once, and main saves the new keys.

## Correspondence

Refines the caching layer `docs/workflow/ci-layout.md` describes. Relates to ADR 0011, whose
docs-only path skips these jobs.

## Verification

- The `gate-parity` lane holds the `tests` and `app-builds` lanes to `ci.yml`. Its rule 6
  refuses a moved test the workspace lane still runs or no `apps` row runs, swept by
  `sim_a_moved_test_left_in_the_workspace_or_dropped_from_ci_is_named`. Its reader now
  reads TOML literal strings, which it used to drop, so rule 2 compares those lanes too.
- The `Build and workspace tests` row's run-wall step fails a run past 300 s.
- Each job prints `deps key <hash>`. A run on a pull request that changes no registry package
  restores with `full match: true`.
