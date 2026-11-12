# lgwks_deps 7.0.0 / lgwks_macros 2.1.2 / lgwks_bot 8.3.1 / drishti 0.1.5

Released 2026-11-12. Every registry edge is a caret requirement, the lock holds the release, and five storefront edges move to their next major.

## lgwks_deps 7.0.0 (major)

- `chacha20poly1305` 0.11, `jsonschema` 0.61, `prettyplease` 0.3, `rusqlite` 0.42 (bundled SQLite 3.54.1) and `syn` 3 are the versions re-exported, so code that names their types through `lgwks_deps` moves with them (#928).
- Every third-party requirement is a caret, not an exact `=` pin (#925, ADR 0031). A consumer's `cargo update` now resolves matched releases, so the `appcui` feature builds again: `appcui` 0.5.7 with `appcui_proc_macro` 0.3.8 (#841).

## lgwks_bot 8.3.1 (patch)

- A record `lgwks_bot` sealed before this release opens after it: a fixed RFC 8439 vector holds the cipher's bytes (#928).
- `estate serve` arms its SIGTERM and SIGINT handlers before it prints `serving`. A SIGTERM sent at the ready line drains and exits 0, where it used to kill the process by signal 15 (#931).

## lgwks_macros 2.1.2, drishti 0.1.5 (patch)

They require `lgwks_deps` ^7.0.0 and change no public item. `lgwks_std` and `lgwks_ast` did not move.
