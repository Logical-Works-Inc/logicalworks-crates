# Census of the runtime crate

Taken on main `d0f2a8c9`, 2026-11-14, for #1014 (B1 of #986). Crate: `crates/lgwks-bot`.
The full census is one SQLite file attached to #1014, with the tool and the queries that made it.

## How each count was taken
- **Files, Lines.** `git ls-files crates/lgwks-bot/src crates/lgwks-bot/tests`, then `wc -l` on each file.
- **Code, Doc, Test.** One class per line. `///` and `//!` lines are doc. A line is test when its file is under
  `tests/`, is `tests.rs`, is in a `tests/` directory or is named `sim_*` or `test_*`. The `#[cfg(test)] mod` that
  closes a file is test too. Blank lines and plain `//` comments count only in Lines: 10,109 of them.
- **Module.** The first path part below `src/`; `src/sim_x.rs` counts in `x`. **Gate.** The feature on its `mod` line.
- **Pub.** Items whose line starts `pub ` (not `pub(crate)`), outside test code, from the code graph `nodes` table.
- **Local.** Pub items with no code-graph edge (`calls`, `references`, `imports`, `instantiates`, `implements`) from
  outside the module. **Unref.** Pub items with no edge at all.
- **Uses, Used by.** How many other modules this one names in `use` lines of its non-test files, and how many name
  it. The module names are in the `module_imports` table.

The graph misses method calls. A second reader re-checked ten rows drawn by seed 1014: all ten match on files and
lines. Of 18 Unref items in those rows, 9 have a real caller, 8 of which the graph did not see. So Local and Unref
are upper bounds, and no item goes on the delete list from the graph alone.

## Modules
| Module | Files | Lines | Code | Doc | Test | Pub | Local | Unref | Uses | Used by | Gate |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `rt` | 59 | 23134 | 9896 | 5782 | 5831 | 377 | 243 | 64 | 5 | 6 | rt |
| `ecs` | 32 | 15622 | 5548 | 3620 | 4949 | 110 | 71 | 18 | 11 | 0 |  |
| `journal` | 36 | 15034 | 5842 | 4040 | 4390 | 178 | 113 | 35 | 2 | 10 |  |
| `script` | 38 | 14259 | 7148 | 4408 | 1616 | 313 | 192 | 63 | 12 | 2 | script |
| `task` | 23 | 11147 | 6022 | 3409 | 803 | 178 | 121 | 27 | 7 | 2 | script |
| `bin` | 21 | 7586 | 6097 | 869 | 0 | 15 | 14 | 2 | 1 | 0 |  |
| `domain` | 17 | 6038 | 3370 | 1635 | 458 | 193 | 157 | 68 | 8 | 0 |  |
| `session` | 11 | 5072 | 2714 | 1476 | 483 | 155 | 131 | 47 | 2 | 0 |  |
| `app` | 10 | 3513 | 1442 | 536 | 1396 | 25 | 6 | 3 | 4 | 0 | edge |
| `proposal` | 8 | 3436 | 1920 | 1207 | 0 | 160 | 127 | 39 | 2 | 2 | script |
| `tenant_store` | 6 | 2628 | 1623 | 838 | 0 | 45 | 31 | 11 | 1 | 1 | wire |
| `frontier` | 6 | 2558 | 700 | 633 | 1109 | 43 | 42 | 10 | 0 | 0 |  |
| `error` | 5 | 2333 | 819 | 763 | 653 | 6 | 1 | 1 | 3 | 14 |  |
| `language` | 4 | 2157 | 371 | 307 | 1406 | 16 | 7 | 0 | 0 | 2 |  |
| `cell` | 5 | 1990 | 1061 | 402 | 404 | 7 | 1 | 1 | 2 | 2 | edge |
| `semantic` | 3 | 1976 | 298 | 293 | 1312 | 19 | 15 | 6 | 1 | 2 |  |
| `spec` | 3 | 1942 | 681 | 512 | 657 | 28 | 25 | 13 | 5 | 4 |  |
| `sealed` | 8 | 1744 | 917 | 608 | 110 | 18 | 6 | 2 | 4 | 2 | sealed |
| `effect` | 5 | 1611 | 617 | 594 | 272 | 37 | 13 | 3 | 0 | 7 |  |
| `broker` | 3 | 1570 | 509 | 583 | 425 | 24 | 7 | 5 | 2 | 2 |  |
| `inspect` | 5 | 1498 | 826 | 333 | 208 | 50 | 38 | 17 | 0 | 1 | inspect |
| `retry` | 3 | 1465 | 315 | 197 | 904 | 26 | 25 | 6 | 1 | 0 |  |
| `dist_lease` | 3 | 1403 | 781 | 506 | 0 | 55 | 44 | 5 | 1 | 1 |  |
| `cap` | 2 | 1373 | 265 | 329 | 710 | 31 | 9 | 5 | 4 | 16 |  |
| `interface` | 3 | 1355 | 455 | 278 | 532 | 35 | 35 | 12 | 0 | 0 |  |
| `clock` | 3 | 1101 | 472 | 497 | 52 | 33 | 9 | 2 | 0 | 8 |  |
| `cap_token` | 2 | 1051 | 718 | 253 | 0 | 42 | 12 | 1 | 1 | 0 |  |
| `idempotent` | 3 | 952 | 217 | 111 | 589 | 11 | 11 | 1 | 2 | 0 | net |
| `review` | 1 | 899 | 478 | 239 | 0 | 25 | 25 | 12 | 0 | 0 | script,process |
| `stability` | 1 | 897 | 268 | 249 | 331 | 20 | 16 | 4 | 0 | 1 |  |
| `registry` | 1 | 814 | 420 | 331 | 0 | 24 | 12 | 4 | 4 | 4 |  |
| `gate` | 2 | 735 | 131 | 111 | 471 | 15 | 2 | 0 | 3 | 8 |  |
| `cas` | 1 | 733 | 433 | 179 | 64 | 29 | 26 | 2 | 4 | 0 |  |
| `lib` | 2 | 725 | 178 | 518 | 8 | 0 | 0 | 0 | 11 | 1 |  |
| `verb` | 1 | 551 | 122 | 268 | 122 | 5 | 0 | 0 | 3 | 6 |  |
| `keyed` | 1 | 320 | 156 | 104 | 37 | 6 | 3 | 1 | 2 | 1 | time |
| `skew` | 1 | 266 | 90 | 118 | 38 | 14 | 5 | 2 | 0 | 2 |  |
| `durable` | 2 | 246 | 39 | 30 | 169 | 0 | 0 | 0 | 0 | 0 |  |
| `sim_stream` | 1 | 113 | 0 | 0 | 113 | 0 | 0 | 0 | 0 | 1 |  |
| `kernel` | 1 | 52 | 24 | 20 | 0 | 0 | 0 | 0 | 0 | 0 |  |
| `json` | 1 | 3 | 1 | 1 | 0 | 0 | 0 | 0 | 0 | 0 |  |
| src total | 343 | 141902 | 63984 | 37187 | 30622 | 2368 | 1595 | 492 | | | |
| `tests/` | 287 | 125970 | 0 | 0 | 125970 | | | | | | |
| crate total | 630 | 267872 | 63984 | 37187 | 156592 | | | | | | |

`frontier`, `interface` and `review` have no `use` line either way. `rt` reaches `frontier` by path, and examples
and tests reach `review`. `interface` has callers in its own tests only, so it is the next caller check.

## Features and the lanes that enable them
Lane ids are from `scripts/gate-lanes.toml`. Full set: `tests-bot-full`, `tests-bot-saturation`,
`tests-bot-darwin-trows`, `clippy-feature-bot`. Default set: `compile`, `tests`, `bot-shard-targets`, `doc-tests`, `lint`.

| Feature | Gates | Enabled by |
|---|---|---|
| `rt`, `time` | `rt`, `keyed` | default set, full set, `clippy-process-only`, `process-windows` |
| `script`, `wire` | `script`, `task`, `proposal`, `review`, `tenant_store` | default set, full set, `process-windows` |
| `process` | `review` | full set, `clippy-process-only`, `process-windows` |
| `edge`, `net`, `inspect`, `sealed` | `app`, `cell`, `idempotent`, `inspect`, `sealed` | full set only |

## Repeated blocks and test pairs
`lgwks-ast scan` has no repeated-block rule, so this census used its own: 8 code lines in a row, string contents
emptied, 6 of them at least 12 characters, found in two modules. Two blocks pass. One is the `Verb` impl prologue at
`crates/lgwks-bot/src/cas.rs:470` and `crates/lgwks-bot/src/idempotent.rs:228`. The other is 9 lines at
`crates/lgwks-bot/src/rt/estate.rs:547` and `crates/lgwks-bot/src/task/admit.rs:42`.

`tests/` holds 34 pairs of `x.rs` and `sim_x.rs`: 19,804 and 16,785 lines. Two pairs assert the same thing:
`serve.rs` and `sim_serve.rs` share three test names, `estate.rs` and `sim_estate.rs` share three assert lines.
The other 32 pairs share only bare `assert!(` openers.

## Delete: no caller
Each item has no graph edge, no other tracked `.rs` file names it, and its own file names it once.

- `crates/lgwks-bot/src/domain/gh.rs:167` `api_path`.
- `crates/lgwks-bot/src/domain/inspect.rs:96`, `:103`, `:110`, `:117`: `with_language`, `with_language_version`, `with_rules`, `with_scope`.
- `crates/lgwks-bot/src/journal/continuation.rs:421`, `:427`: `events_watermark`, `bytes_watermark`.
- `crates/lgwks-bot/src/journal/file/open.rs:222` `own_seal`; `crates/lgwks-bot/src/proposal/codec.rs:106` `with_max_field_bytes`.
- `crates/lgwks-bot/src/proposal/completion.rs:365` `from_outcome`; `crates/lgwks-bot/src/proposal/mod.rs:661` `is_spent`.
- `crates/lgwks-bot/src/proposal/surface.rs:201` `holding_all_shipped`; `crates/lgwks-bot/src/review.rs:178`, `:361`: `step_budget`, `is_partial`.
- `crates/lgwks-bot/src/session/graph.rs:371` `with_resources`; `crates/lgwks-bot/src/session/runner.rs:199` `current_node`.
- `crates/lgwks-bot/src/session/receipt.rs:331`, `:337`: `into_records`, `into_receipts`.

## Merge: same job twice
- `genesis_head`, one signature, three copies: `crates/lgwks-bot/src/task/ledger.rs:85`,
  `crates/lgwks-bot/src/task/store.rs:143`, `crates/lgwks-bot/src/tenant_store/record.rs:30`.
- `auto_workers`, one signature: `crates/lgwks-bot/src/app.rs:430` and `crates/lgwks-bot/src/ecs/requests.rs:290`.
- The two repeated blocks above, and the `serve` and `estate` test pairs.

## Move: wrong layer
Each file sits in its ADR 0052 planned crate and imports a crate that one may not depend on.

- `crates/lgwks-bot/src/sealed/sweep.rs:30` (kernel) imports `rt::task`; lines 29 and 31 import `rt::sync` and `task`.
- `crates/lgwks-bot/src/sealed/runstore.rs:17` (kernel) imports `script::run_store`.
- `crates/lgwks-bot/src/rt/runtime.rs:452`, `:454` (rt) import `rt::estate` (app) and `task` (script).
- `crates/lgwks-bot/src/script/serve.rs:8` (script) imports `rt::serve` (app).
- `crates/lgwks-bot/src/ecs.rs:226`, `:241` (world) re-export `cell` and `ecs::requests` (app).
