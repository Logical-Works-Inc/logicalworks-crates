# lgwks_std 4.0.1 / lgwks_ast 5.1.0 / lgwks_deps 6.0.1 / lgwks_macros 2.0.1 / lgwks_bot 7.1.0 / drishti 0.1.1

2026-11-05

Release following the hardening and completion of open capability tokens, OpenAPI generation,
fail-closed durable starts, and adversary simulation suites.

## lgwks_bot 7.1.0 (minor)

- `cap_token`: Bearer capability tokens with caveats, expiry, and explainable policy decisions (#843, #779).
- `rt::serve::openapi`: Emits OpenAPI 3.1 specification documents directly from the registered routes (#832, #830).
- `task::store`: Fail-closed startup when the durable journal sink cannot persist (#829, #820).
- `task::store`: Seeded disk-fault simulation proving uncorrupted recovery across failure models (#824, #819).
- `examples`: Ceremony reference application exercising the full app builder runtime slice (#847, #780).

## lgwks_ast 5.1.0 (minor)

- `hunks`: Added unified diff reader `added_since` for changed-line gate accounting (#862).
- `scan`: Doc example scanner asserting fenced code blocks on every public item (#863, ADR 0012).
- `script::tree`: Tree blocks and block vocabularies refined for route and serve emission (#832).

## lgwks_std 4.0.1 (patch)

- `catalog`: Restored the `[[capability]]` header before the HTTP server in `contract/CATALOG.toml` (#826).
- `tests/it/sim_adversary_fools`: Hardened adversary simulation suite covering 10 failure archetypes (#866).

## lgwks_deps 6.0.1 (patch)

- Bumps pinned `lgwks_bot` (7.1.0) and `lgwks_std` (4.0.1) in `lgwks new` scaffold template.
- Updates internal workspace dependency graph requirements.

## lgwks_macros 2.0.1 (patch)

- Updates internal dependencies for `lgwks_ast` 5.1.0 and `lgwks_std` 4.0.1.

## drishti 0.1.1 (patch)

- Aligns internal workspace dependencies with `lgwks_bot` 7.1.0 and `lgwks_std` 4.0.1.
