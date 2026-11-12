---
name: lgwks-dependency-update
description: Use when a Dependabot pull request is open or red, when a lockfile is behind, when `lgwks-deps freshness` names stale packages, when a nested fixture or bench lockfile is stale, or when a new major of an approved edge is published. Covers landing a compatible update, landing a major, and re-locking every nested workspace root.
---

# Landing a dependency update

An update pull request is work to land. Closing one leaves the lockfile behind its
advisories, and the next one arrives larger. Read its failed checks, fix what they
name, and merge it. Run every command from the repository root.

## 1. Tell the two kinds apart

| The pull request changes | It is | Route |
|---|---|---|
| `Cargo.lock` alone | a compatible update | step 2 |
| a requirement in a `Cargo.toml` | a major, or a floor that moved | step 3 |

Every registry edge is a caret requirement and the lockfile holds the release
(ADR 0031, INV-DEP-5). So a patch or minor release changes no manifest, no rung of
`contract/APPROVED.toml` and no nested lockfile.

## 2. A compatible update

```sh
cargo run --locked -p lgwks_deps -- freshness .   # what is behind, and by how much
cargo update                                      # the root lockfile, newest compatible
cargo run --locked -p lgwks_deps -- check .       # must print OK
./scripts/ci-local.sh --lane advisories
```
Then run the suites. A failure here is a real change upstream: a deprecation with
warnings denied, a moved type, a changed default. Fix the code in the same pull
request. Do not write an exact `=` requirement to hold the old release.

## 3. A major, or any manifest change

1. Move the requirement in the owner's `Cargo.toml`.
2. Move the rung: `version` in `contract/APPROVED.toml` equals the requirement as
   `cargo metadata` prints it (`^1`, not `1`), and `approved_on` takes today's date.
3. Fix the code the compiler names.
4. Re-lock every nested root, as step 4 lists them.
5. Measure the cold build before and after, as `skills/lgwks-dependency-admission/SKILL.md` asks.

## 4. Re-lock the nested roots

A manifest change, or a release that bumps a workspace crate's version, leaves the
nested lockfiles stale. They are four roots under `bench/` and eleven under
`crates/lgwks-deps/tests/fixtures/`, three of them in `contract-override/`.

```sh
cargo update --workspace --manifest-path crates/lgwks-deps/tests/fixtures/storefront-matrix/Cargo.toml
cargo metadata --locked --format-version 1 --manifest-path crates/lgwks-deps/tests/fixtures/storefront-matrix/Cargo.toml >/dev/null
./scripts/ci-local.sh --lane lockfile
```
Repeat the first two for each root. The `lockfile` lane names the first stale
fixture, and the suites that build a root `--locked` name the rest.

## 5. When a release is truly broken

Hold one release only with a record. Write the exact requirement, and add an
`upstream-pin` row to `contract/PATCHES.toml` with the upstream issue, the pull
request and an expiry. The gate refuses the row once it lapses, and
`ledger::tests::the_committed_register_pins_no_registry_edge_exactly` refuses an
exact pin with no row.

## 6. Read the red checks as instructions

| The check says | Repair |
|---|---|
| `RequirementDrift` | The rung and the manifest disagree: step 3.2. |
| `stale fixture lockfile` or `cannot update the lock file` | Step 4. |
| `use of deprecated` | Upstream moved an item: change the call, keep warnings denied. |
| `UnusedApproval` | A rung names a requirement no manifest authors: fix the rung. |
| an advisory id | Update past it, or assess it in `SECURITY.md` with a `reopen if`. |
