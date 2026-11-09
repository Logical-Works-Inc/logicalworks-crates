# Dependency doctrine

The dependency policy for the Logical Works crates: which capabilities are
provided in-tree, which third-party crates are admitted, and how a new dependency
is registered. It applies to every crate in this workspace.

Every dependency edge carries supply-chain surface, build-time cost, and license
and advisory obligations. Widely used crates (`tokio`, `syn`, `serde_json`,
`regex`, `uuid`, `chrono`, `walkdir`, `reqwest`) are frequently added by habit
even when a first-party equivalent already exists in the tree. This project pays
those costs for a small, deliberate set of dependencies and declines the
remainder.

The rule, in one line: **if the capability is in `std`, `lgwks_std`,
`lgwks_bot`, or `lgwks_ast`, use it; a new third-party dependency is an
optional, feature-gated edge of `lgwks_deps`, registered with a reason and
never built unless the end user selects it.**

The doctrine is split by responsibility; the section numbers are stable:

- §0 to §2 (this page): the law, the admission ladder, the decision flow.
- §3: the [replacement matrix](dependency-doctrine-matrix.md).
- §4 and §5: the [worked examples](dependency-doctrine-examples.md), `tokio` and `syn`.
- §6 to §8: the [gaps, procedure and enforcement](dependency-doctrine-gaps.md).

## 0. The dependency law (Decision, 2026-09-12)

Cognitive load is `std` + `bot` + `deps`, plus the standalone `ast`:

- `lgwks_std`: the core surface, depended on by the other three or used alone.
- `lgwks_bot` — async, runners, actors.
- `lgwks_deps` — the **storefront**: install it, select features, get deps.
- `lgwks_ast` — the one parser. Its only workspace edge is the optional one to
  `lgwks_std` for its `tool`.

A new dependency is registered with `owner = "lgwks_deps"` and exposed as an
optional feature of `lgwks_deps` (for example `--features gpui`).

The storefront's default selection is empty: a default that cannot build on a
declared target is not a default, as ADR 0003 records.

`lgwks_std`'s core feature stack and
`lgwks_ast`'s parser are settled. `lgwks_std` cannot route through
`lgwks_deps` because `lgwks_deps` depends on `lgwks_std`; the reverse would be
a cycle.

## 1. The admission ladder

`cargo run -p lgwks_deps -- tiers` prints the rungs. Lowest rung first; each
step up is an escalation that needs a stated reason:

| # | Rung | Meaning |
|---|------|---------|
| 1 | `std` / `core` / `alloc` | Preferred unconditionally. Zero supply-chain cost. |
| 2 | `lgwks_std` | The workspace substrate: hex, base64, time, id, hash, glob, pattern, json, ron, wire, fs, task, leb128, encoding, http, online. |
| 3 | `lgwks_bot` | Async, runners, and actor roles, including the curated runtime surface. |
| 4 | `lgwks_deps` storefront | Every other third-party capability, as an optional feature the end user selects. |
| 5 | ELIMINATE | The capability belongs in `lgwks_std`/std: write the minimal zero-dep implementation there. |
| 6 | CONSOLIDATE | Several crates solve the same problem: pick one, retire the rest. |
| 7 | VENDOR | Audited upstream source checked into the workspace, not a registry edge. |
| 8 | BOUNDARY | A third-party crate approved in `contract/APPROVED.toml` with a human sign-off and a reason naming what `std` cannot do. |

ELIMINATE and CONSOLIDATE crates never get a register entry: they become a
module in `lgwks_std` (or are removed). `tier` in the register admits only
`boundary` and `vendor`. A new BOUNDARY edge is normally an optional feature of
the `lgwks_deps` storefront (`owner = "lgwks_deps"`), so it is never built
unless the end user selects it.

## 2. Decision flow

Before adding any edge:

1. **Does `std` do it?** `OnceLock`/`LazyLock` (not `once_cell`/`lazy_static`),
   `available_parallelism` (not `num_cpus`), `std::sync::mpsc`,
   `std::thread`, `std::fs`, `std::net`, integer `from_le_bytes` (not
   `byteorder`). If yes, stop.
2. **Does a workspace module do it?** Check the matrix in §3. If yes, import
   `lgwks_std`, `lgwks_bot`, or `lgwks_ast`, not the crate underneath.
3. **Is it a Rust parser or multi-language parser?** Both belong to `lgwks_ast`,
   whose `scan` reads Rust source (ADR 0002). A second parser is not admitted (§5).
4. **Is it async / actor work?** Async, runners, and actor roles are
   `lgwks_bot`; its tokio engine is selected through the `lgwks_deps` storefront.
   The zero-dependency synchronous executor is `lgwks_std::task`. See §4.
5. **Otherwise** it is a storefront BOUNDARY decision: add the crate as an
   optional feature of `lgwks_deps`, run
   `cargo run -p lgwks_deps -- request <crate> <version>`, fill the block with
   `owner = "lgwks_deps"`, and commit. The commit is the approval.
6. **Is it day 0?** The revenue slice admits 2–6 crates; everything else waits
   for its measured trigger in the [deferral catalog](deferral.md). An approval
   the catalog still defers is refused with the owner and the trigger.
7. **Mortgage every edge.** Each approval carries a row in the companion
   ledger `contract/LIFECYCLE.toml` naming `replacement`, `removal_trigger`
   and `last_reviewed`; an approval with no row is refused as
   `MissingLifecycle`. The `ledger` binary of `lgwks_deps` prints the review page the quarter closes on.

A direct edge that skips this flow fails `lgwks-deps check` with the exact
crate, requirement, and source class named.

## Schema 3: every edge mortgages its exit in the register (#788)

Each `[[approved]]` entry carries a required `[approved.retire]` table with
`replacement` (the `std`, `core`, `alloc` or workspace module path that will
own the capability), `blocker` (one sentence naming what is missing today),
`cost` (an inline table with the measured `cold_build_s`, `crates`,
`binary_bytes`, the `measured` date and the exact `command`), and `review`
(the date the mortgage lapses). `[policy] schema = 3`; an older register is
refused with the migration. `lgwks-deps check` refuses a missing table, an
unowned replacement, an expired review and a non-positive cost, each naming
the entry and the field. `lgwks-deps retire --report [--json]` prints every
mortgage with the summed distance to zero.
