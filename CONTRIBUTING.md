# Contributing

This page lists the rules a change meets before it merges.

## Dependency policy

Read [`docs/dependency-doctrine.md`](docs/dependency-doctrine.md) before adding a
dependency. The default answer to "can I add this crate?" is no. If the
capability exists in `std`, `lgwks_std`, `lgwks_bot`, or `lgwks_ast`, use it.

A new third-party dependency becomes an optional, feature-gated edge of
`lgwks_deps` with `owner = "lgwks_deps"`, registered in
`contract/APPROVED.toml`. It is never a direct edge and never a new top-level
crate. `lgwks-deps check` refuses both unregistered edges and stale unused
approvals.

## Workflow

1. Branch from `main`. One concern per branch.
2. Check the existing surface before writing new code. If the capability is
   genuinely missing, follow `skills/lgwks-dependency-admission/SKILL.md`.
3. Update `CHANGELOG.md` under `[Unreleased]`, with per-crate Added, Changed,
   and Decisions entries. A behaviour or message change without a changelog
   entry is incomplete.
4. Document every `pub mod` listing with a `///` on the declaration line;
   parent listings render empty otherwise.
5. New public API ships with a `/// ```rust` doc example so `cargo test --doc`
   compiles it, or an example under `examples/`.
6. Run the full gate before pushing.

```sh
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo run -p lgwks_deps -- check .
./scripts/lgwks-std-package-smoke.sh
```
## Contributions and copyright

**This repository takes no outside contributions, to any crate.** Pull
requests, issues and comments are open to Logical Works collaborators only
(the repository's interaction limit, `collaborators_only`), and a workflow
run from a fork waits for a maintainer's approval
(`fork-pr-contributor-approval`, `all_external_contributors`). A patch from
outside is closed unmerged on arrival, whatever its content (Director
2026-11-04: "NO EXTERNAL CONTRIBUTIONS ALLOWED").

`lgwks_bot` has a second reason: it is offered under two licences, including a
commercial one, which requires the licensor to hold every right in every
contribution ([`LICENSING.md`](LICENSING.md)).

## Standards

- `#![forbid(unsafe_code)]` everywhere; no `unwrap` outside tests; typed errors
  at library boundaries, with `source()` forwarding where a wrapped error
  exists.
- Match the file you are in: naming, comment density, idioms. Comments state
  constraints the code cannot show. No narration, no history, no reassurance.
- `0.x` means minors may break. Bump the crate version, record it in the
  changelog, and never match on error strings across crates.
