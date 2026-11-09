---
name: lgwks-change-a-crate
description: Use when adding or changing a public item, module, binary, feature, test or behaviour in a crate of this repository. Lists what each kind of change must also touch so the gate passes the first time.
---

# Changing a crate

Each row below is a change and what must land in the same commit, or a lane refuses it.

## Before the first edit

1. Read `INVARIANTS.md` and the crate's own `INVARIANTS.md` whole.
2. Find what exists in `contract/CATALOG.toml`, or run `cargo run --locked -p lgwks_deps --bin catalog -- --json .`. A second implementation of a capability is a defect.
3. Name the journey the change serves, and how it fails. It lands wired into a real run path, because a module with tests and no caller is not a feature.

## What each change must also touch

| You add | Also change | The lane that refuses otherwise |
|---|---|---|
| A `pub` library item | Its ceiling in `contract/PUBLIC_SURFACE.toml`, a caller outside the crate, and a fenced example in its doc | `std-first`, `examples` |
| A `pub` item in `lgwks_ast` or `lgwks_macros` | The same; these crates are held whole | `examples-held` |
| A capability | A row in `contract/CATALOG.toml`, then `cargo run --locked -p lgwks_deps --bin catalog -- --write .` | `catalog` |
| A cargo feature | The same row, the default set unless ADR 0003 records why not, and `docs/guides/lgwks-bot/features.md` for `lgwks_bot` | `catalog`, `feat-bot` |
| A module | Its row in `CODEBOOK.md` if the map lists its crate, and a `///` on the declaration | `lint` |
| A binary | A `[[bin]]` with `required-features`, and a test that runs it | `compile` |
| A rule worth keeping | One line in `INVARIANTS.md`: rule, why, enforcing test | `invariants` |
| A decision | `docs/adr/` page with every field, a row in `docs/adr/README.md` | `doc-governance`, `tests` |
| A documentation page | A row in `docs/README.md`, or a link from a page with one; then regenerate `llms.txt` | `llms-txt` |
| A released change | A page in `docs/changelog/` and a row at the top of `CHANGELOG.md` | review |

## Writing the code

- Typed errors, with no `unwrap`, `expect` or `panic` outside tests. A wrapped error keeps its `source()`.
- Every queue, cache, fan-out and payload has a bound and an overflow answer. Every spawned task has an owner that stops it.
- No lock held across `.await`. No print from a library: `lgwks_std::trace` is the path.
- Log at the site that returns an error, or the `scan` lane reports `unlogged-err-return`.
- A file holds at most 750 code lines, and 1000 lines in all. Split at a real seam before the limit.

## Writing the tests

- A module of `crates/lgwks-deps/tests/it/` is registered in its `main.rs`, and the `lgwks_bot` suite's is the same with a shard group: `#[cfg(any(not(lwc_it_group), lwc_it_group = "gN"))]`.
- Drive the shipped binary or the public API, and assert what a user sees. A unit test alone proves little.
- At least half the tests are simulations, in a `sim_` module where one seed drives every draw, and the same seed gives the same trace. The `simulation-evidence` lane counts them.
- No sleep, no wall clock and no unseeded randomness. A test cites what it derives from, and names the failure it would catch.
- Tests use `Result` and `?`, not `unwrap`.

## Prove it

```sh
cargo nextest run --locked -p lgwks_bot -E 'test(/kernel_surface/)'
./scripts/ci-local.sh --lane std-first
./scripts/ci-local.sh --lane scan
./scripts/ci-local.sh --lane doc-governance
./scripts/ci-local.sh --fast
```
Run `./scripts/doc-lanes.sh` before a pull request that edits `//!` or `///` docs, because a pull request's CI skips the main-only docs job (INV-DOC-1).

## Landing

One concern to a branch, branched from `main`. A red lane routes you to `skills/lgwks-red-gate/SKILL.md`, and a version bump to `skills/lgwks-release/SKILL.md`.
