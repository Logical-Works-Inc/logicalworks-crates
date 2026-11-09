---
name: lgwks-dependency-admission
description: Use when adding, upgrading, or auditing a third-party dependency in a LogicalWorks repository, when `lgwks-deps check` refuses an edge (unregistered edge, consumer not allowed, requirement/source drift, kind not allowed, unused approval), or when deciding whether a crate belongs in lgwks_std, lgwks_bot, lgwks_ast, or the register. Covers the admission ladder, the APPROVED.toml schema, and the fail-closed rules.
---

# Adding a dependency to a gated LogicalWorks repo

The register is `contract/APPROVED.toml`; the gate is `lgwks-deps`. A build
cannot carry an external edge that is not in the register, and the register
cannot carry an approval with no authored edge. Both directions are enforced.
Run every command from the repository root.

## 0. First, try not to add anything

Read `docs/dependency-doctrine.md` and check the replacement matrix. If the
capability is in `std`, `lgwks_std`, `lgwks_bot`, or `lgwks_ast`, stop: using
the crate directly is duplication and will be refused or reverted. If it is a
general primitive with no equivalent, the correct answer is a new `lgwks_std`
module (ELIMINATE), not a consumer-local dependency.

## 1. Find the rung, print the form

```sh
cargo run --locked -p lgwks_deps -- tiers
cargo run --locked -p lgwks_deps -- request <crate> <version>
```
Lower rungs are preferred. Only `boundary` and `vendor` produce a register
entry; `eliminate` and `consolidate` produce a module in `lgwks_std` instead.
`request` prints a fillable `[[approved]]` block. No command approves
anything: approval is the committed diff with a human's name on it.

## 2. Fill every field: the register schema

Append the block to `contract/APPROVED.toml`. Every field is required, and
`contract.rs` refuses a missing field, an unknown key, a bad tier, a bad date,
and a reason that is not a real sentence.

```toml
[[approved]]
crate = "rustix"                 # package name exactly as Cargo.lock spells it
tier = "boundary"                # boundary | vendor
version = "^1"                   # EXACTLY the manifest requirement cargo metadata reports
owner = "lgwks_std"              # workspace crate responsible for the capability
capability = "fs.available_space" # stable semantic capability name
source = "registry"              # registry | git | path
allowed_consumers = "lgwks_std"  # comma-separated workspace crate names
allowed_kinds = "normal"         # normal, build, and/or dev
reason = "Filesystem capacity available to an unprivileged process is absent from the stable standard library."
approved_by = "maintainer"       # the human who decided
approved_on = "2026-09-12"       # ISO YYYY-MM-DD
review = "https://github.com/..." # path or URL to the evidence

[approved.retire]                # schema 3: how this edge leaves (INV-DEP-21)
replacement = "..."              # the std or workspace module that takes over
blocker = "..."                  # one sentence: why it has not yet
cost = { cold_build_s = 1.0, crates = 1, binary_bytes = 1, measured = "2026-09-12", command = "..." }
review = "2026-12-01"            # the date this mortgage lapses
```
Every approved edge also needs its row in `contract/LIFECYCLE.toml`, with `crate`,
`replacement`, `removal_trigger`, `last_reviewed` and `review` (INV-DEP-19).

Rules the parser enforces:

- `version` must equal what `cargo metadata --no-deps` reports for the edge.
  `version = "1"` in the manifest is reported as `^1`; write `^1`. Drift is
  refused as `RequirementDrift`.
- `reason` is a sentence: at least four words, at least 24 characters, ending
  in a full stop, and not just the crate name. Say what `std` cannot do.
- `allowed_consumers` / `allowed_kinds` are comma-separated; a consumer not on
  the list is refused as `ConsumerNotAllowed`.
- A `registry` approval does not cover a `git` or `path` edge (`SourceDrift`).

## 3. Add the authored edge under its owner

- Core primitive → `crates/lgwks-std` (its existing feature stack is settled).
- Async, runners, actor roles → `crates/lgwks-bot`; its runtime engine is
  selected through the `lgwks_deps` storefront's `tokio` feature.
- Parsing → `crates/lgwks-ast`, the one parser.
- **Everything else → an optional feature of `crates/lgwks-deps`**, the storefront:
  register with `owner = "lgwks_deps"` and expose it such as `gpui = ["dep:gpui"]`.
  It is on by default unless ADR 0003 records its cost or a declared target denies it.

## 4. Update everything that pins the closure

- The `## Dependency philosophy` bullets in `crates/lgwks-std/README.md`, for an
  edge of `lgwks_std`: the `contract-drift` lane compares them to the manifest keys.
- `contract/CATALOG.toml` and `docs/catalog.md`, if it adds a capability.
- `docs/guides/lgwks-deps/storefront.md` or the feature table of the owner's guide,
  if you added a feature.
- `Cargo.lock`, and every nested lock the edge reaches.

## 5. Prove it

```sh
cargo metadata --locked >/dev/null              # lockfile current
cargo run --locked -p lgwks_deps -- check .     # must print OK
./scripts/ci-local.sh --lane deps-register
./scripts/ci-local.sh --lane std-first
```
A refusal names the crate, the requirement, and the source class, and each refusal
is a decision, not paperwork. Measure the cold build before and after: an edge that
takes a gate past its budget is a defect, whatever the register says.

## 6. Read the refusals as instructions

| Refusal | Repair |
|---|---|
| `UnregisteredEdge` | Add the `[[approved]]` block and the authored edge. |
| `ConsumerNotAllowed` | Add the consumer to `allowed_consumers`, or move the edge to the owner. |
| `RequirementDrift` | Make `version` equal the manifest requirement (`^1`, not `1`). |
| `SourceDrift` | `source` must be `registry`, `git`, or `path` matching the edge. |
| `KindNotAllowed` | Add `normal`/`build`/`dev` to `allowed_kinds`. |
| `UnusedApproval` | An approval with no authored edge is stale: add the edge or delete the block. |
| `ForeignWorkspaceMember` | A workspace member declares a foreign repository; consume the published crate instead. |

The gate is fail-closed: a missing, unparseable or unreadable input is a refusal.
