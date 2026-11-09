---
name: lgwks-release
description: Use when bumping a crate version, cutting or publishing a release, tagging, or proving a published version from crates.io. Lists every file a bump moves, the publish order and the traps of a cut.
---

# Cutting a release

`docs/releasing.md` is the process, and this is its checklist with what has broken a cut.
Publishing is the Director's authority (`GOVERNANCE.md`): an agent publishes only on their order.

## 1. Choose each version

A removed or renamed public item, or a new variant on an exhaustive enum, is a major bump. A new public item is a minor bump. A stricter refusal is a minor bump too, and a fix that changes no public item is a patch.

Find what a release removes before calling it a patch, because a removed re-export leaves no `pub ` line in the file a reader looks at:

```sh
git diff -U0 <last-tag> origin/main -- crates/<dir>/src
```
Read the removed `pub` and `pub use` lines it prints, and keep `contract/PUBLIC_SURFACE.toml` in step.

## 2. Bump in one commit

- The `[package] version` of each moved crate.
- Its row in `[workspace.dependencies]` of the root `Cargo.toml`.
- The version table in the root `README.md`, which a test reads.
- `BOT_VERSION` and `STD_VERSION` in `crates/lgwks-deps/src/bin/lgwks/new.rs`, the versions a scaffold pins.
- The tags under `previousVersions` in `context7.json`.
- The root `Cargo.lock`, through `cargo update --workspace`.
- Every nested lock, each with its `cargo update --workspace --manifest-path`. They live in the following places:
  - `bench/Cargo.lock`, `bench/ai-authoring/Cargo.lock`, `bench/async/Cargo.lock`, `bench/std-measure/Cargo.lock`
  - every `Cargo.lock` under `crates/lgwks-deps/tests/fixtures/`, including `contract-override/*`
- A page in `docs/changelog/` and a row at the top of `CHANGELOG.md`, stating what a consumer will notice.

Prove each nested lock with `cargo metadata --locked --offline --manifest-path <dir>/Cargo.toml`.

## 3. Gate, merge, then publish

Land the bump through a pull request whose every check is green, then publish from `main`, leaves first. `lgwks_ast` needs only `lgwks_std`, and `lgwks_deps` needs both:

```sh
cargo publish -p lgwks_std --locked
cargo publish -p lgwks_ast --locked
cargo publish -p lgwks_deps --locked
cargo publish -p lgwks_macros --locked
cargo publish -p lgwks_bot --locked
cargo publish -p drishti --locked
```
Skip a crate that did not move. An uploaded version cannot be withdrawn, and a chain that stops halfway is finished later, never rolled back.

## 4. Tag after the upload

A tag or a release created before the upload is a claim the registry does not back. Tags are `<package>-v<version>`, as in `lgwks_bot-v8.1.0` and `drishti-v0.1.3`:

```sh
git tag lgwks_std-v4.1.1 && git push origin lgwks_std-v4.1.1
gh release create lgwks_std-v4.1.1 --title "lgwks_std 4.1.1" --notes-file <notes>
```
The first `script!` block of `crates/lgwks-bot/README-script.md` at the tag is what the consumer below compiles for `lgwks_bot`.

## 5. Prove it from a stranger's side

```sh
cargo run --locked -p lgwks_deps --bin published-consumer
```
The release is done at `published consumers: all passed`, with one receipt line per consumer. It needs the network, and it runs from the tree whose tags exist locally.

Measure too: a cold default build of the new version in an empty package with no compiler cache, against the 60 s budget, and `cargo tree -e normal` for the crate count.

## Traps

- A release-notes page naming a revocation word needs INV-FENCE-1 beside it, because a suite test scans first-party text for a claim the crate lacks.
- A bump moves fixture locks, so the plan and the diff are larger than they look.
- `lgwks_bot`'s lockfile changes with `cargo update --workspace`, and nothing else in it may move. Read the lock diff.
- A disk at its limit fails a publish's verify step. Free incremental build output, not the whole target directory.
