# Releasing

A version of these crates reaches crates.io by the steps below, and only when
their conditions hold. `skills/lgwks-release/SKILL.md` is the checklist form.

## 1. Publish order

The crates form a chain, and Cargo refuses to publish a crate whose path
dependency version is not already on the registry:

```text
lgwks_std ──► lgwks_ast ──► lgwks_deps ──► lgwks_macros ──► lgwks_bot ──► drishti
```
`lgwks_ast` needs only `lgwks_std`, and only for its `tool`. `lgwks_deps` needs
`lgwks_std` and `lgwks_ast` (#787). `lgwks_macros` needs all three, `lgwks_bot`
needs those and `lgwks_macros`, and `drishti` needs `lgwks_bot`, `lgwks_std` and
`lgwks_deps`. `the_publish_order_is_the_dependency_order` reads this list against
the manifests. Skip a crate that did not move:

```sh
cargo publish -p lgwks_std    --locked
cargo publish -p lgwks_ast    --locked   # needs lgwks_std
cargo publish -p lgwks_deps   --locked   # needs lgwks_std, lgwks_ast
cargo publish -p lgwks_macros --locked   # needs lgwks_deps, lgwks_ast, lgwks_std
cargo publish -p lgwks_bot    --locked   # needs all of the above
cargo publish -p drishti      --locked   # last: needs lgwks_bot
```
`cargo publish --workspace --dry-run --locked` packages and verifies the whole
chain in dependency order before any of it is on the registry, which a per-crate
`--dry-run` cannot do for a crate whose dependency's new version is not
uploaded yet. The order is a real constraint: publishing a crate before its
dependency fails with `candidate versions found which didn't match: <older
versions>`.

`cargo publish --dry-run` reaches the upload stage without a token and stops
there, so it says nothing about authentication: confirm credentials first
(`ls ~/.cargo/credentials.toml`, or `test -n "$CARGO_REGISTRY_TOKEN"`), or the
train stops mid-chain. A partially published chain is recoverable (finish the
rest once the token is available), but uploaded versions cannot be withdrawn.

## 2. Gates that must pass first

CI runs the gate, defined once in `scripts/gate-lanes.toml`. Run its local lanes
with one command rather than from a copied list, because a workspace-wide clippy
does not compile every feature combination and the lanes do:

```sh
./scripts/ci-local.sh
./scripts/doc-lanes.sh                   # rustdoc in all four feature lanes
cargo run --locked -p lgwks_deps -- check .
```
`.cargo/config.toml` sets `rustc-workspace-wrapper = "clippy-driver"`, so every
`cargo check` here is a clippy run. The doc gate is one call because *which*
intra-doc links break depends on which features are on. The `contract-drift`
lane cross-checks `crates/lgwks-std/Cargo.toml` against the list under
`crates/lgwks-std/README.md`'s `## Dependency philosophy`; a dependency added
without a bullet fails it.

## 3. Cut the release commit

All version bumps land in a single commit:

- the `[package] version` fields, and their `[workspace.dependencies]`
  requirements in the root `Cargo.toml`;
- the version table in the root `README.md`;
- `BOT_VERSION` and `STD_VERSION` in `crates/lgwks-deps/src/bin/lgwks/new.rs`, and
  the tags in `context7.json`;
- `Cargo.lock` (`cargo update --workspace`), and the `Cargo.lock` of each nested
  workspace under `bench/` and `crates/lgwks-deps/tests/fixtures/`.

Each nested lock pins the moved crates by path, and CI checks it `--locked`, so
prove it with `cargo metadata --locked --offline --manifest-path <dir>/Cargo.toml`.
A page under `docs/changelog/` and a row in `CHANGELOG.md` follow, per crate, and
record the behavioural changes a consumer will encounter, not only the signatures.

Version position: every crate but `drishti` is past 1.0.0, so semantic
versioning applies as written. A removed or renamed public item, or a new
variant on an enum that is not `#[non_exhaustive]`, takes a **major** bump. New
public items, and new variants on `#[non_exhaustive]` enums, take a **minor**
bump. Fixes that change no public item take a **patch**. A refusal made
stricter is a minor bump, not a patch: code that compiled may stop compiling,
and the changelog says so (`lgwks_macros 1.1.0`, #265).

## 4. Tag and release: after the upload, not before

Tags are `<package>-v<version>`, one per crate that moved:

```sh
git tag lgwks_std-v4.1.1
git push origin lgwks_std-v4.1.1
gh release create lgwks_std-v4.1.1 --title "lgwks_std 4.1.1" --notes-file <notes>
```
There is no publish-on-tag workflow: `.github/workflows/` has no `cargo
publish` step, so nothing is uploaded by pushing a tag. A tag or a GitHub
release created before the upload is a claim the registry does not support.

## 5. Prove it from a stranger's side

After the last tag, build every published crate from crates.io at its newest
tag, with the README's own quickstart as the program:

```sh
cargo run -p lgwks_deps --bin published-consumer
```
It prints one receipt line per consumer (ten: `lgwks_std` default, core and
full; `lgwks_ast`; `lgwks_deps` default and `tokio-full`; `lgwks_bot` default
and full; `lgwks_macros`; `drishti`) and exits non-zero when one fails or a tag names a version crates.io
does not serve as its newest. The release is done at `published consumers: all
passed`.
