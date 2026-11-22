# lgwks_std 5.0.0 / lgwks_ast 6.1.1 / lgwks_deps 8.0.0 / lgwks_macros 2.1.4 / lgwks_bot 10.0.0 / drishti 0.3.0

Cut 2026-11-21. Every dependency moves to its newest release (#1115 and this cut). Three crates take a major bump. `lgwks_deps` re-exports jiff 1, rusqlite 0.43 and cosmic-text 0.19. `lgwks_std` and `lgwks_bot` remove public items. Each version follows from the removed `pub` lines of its crate against its last tag.

## What a consumer notices

- `lgwks_deps` 8.0.0 re-exports `jiff` 1.0, `rusqlite` 0.43 and `cosmic_text` 0.19. Code that names their types through `lgwks_deps::storefront` follows each upstream's own changes.
- `lgwks_std` 5.0.0 has no `http::Response::text_lossy`, and `http::Options` has no public fields. Use `text()` and handle its `Err` for a body that is not UTF-8.
- `lgwks_bot` 10.0.0 removes `sealed::runstore` and `sealed::SweepError` (#1092).
- `drishti` 0.3.0 reads its ink and runtime values through methods.
- `lgwks_ast` 6.1.1 and `lgwks_macros` 2.1.4 change no public item.

## lgwks_std 5.0.0 (major: removed items)

- `http::Options` fields are private. Set each through the builder method of the same name (#1103).
- `http::Response` moves to its own module. `status` and `truncation` are methods, not fields, and `text_lossy` is removed. `headers`, `header_values`, `final_target`, `redirect_chain`, `body` and `text` stay (#1103).
- A stopped test shell is continued until it exits, and is never waited on (#1111).
- `lgwks_std` builds for `aarch64-apple-ios`. A unix with no process-table reader refuses `capture_descendants_many` with `Unsupported` and never answers an empty table (#892).

## lgwks_deps 8.0.0 (major: re-exported majors, removed fields)

- Re-exports: `jiff` ^1.0, `rusqlite` ^0.43 and `cosmic-text` ^0.19 (#1115).
- The gate's types are read through methods, not public fields (#1103):
  - `contract::Contract::enforce` and `repository`.
  - `invariants::Outcome::status` and `line`.
  - `kind`, `source`, `optional`, `workspace` and `target_repository` on a `metadata` dependency edge. Its `license` fields are private.
- A locked package adds `is_crates_io`.
- `jsonschema` ^0.62.0 behind `json-schema`, and `appcui` 0.5.10 behind `appcui`. The 0.5.9 pair that #1112 held back did not build, and 0.5.10 builds.
- `lgwks new` pins `lgwks_bot` 10.0.0 and `lgwks_std` 5.0.0.

## lgwks_bot 10.0.0 (major: removed items)

- The kernel imports no upper layer (#1092). `sealed::runstore` and `sealed::SweepError` are removed.
- A `KeyProvider` can be written outside the crate (#1060). A key fault fails the sealed pending listing and never shortens it (#1085).
- A model call is a durable step, so a resume never pays twice (#1107). A `justify` token rule bounds the model spend of a run (#1108).
- The `estate` smoke starts its own executable, so a host with no `/usr/bin/true` passes (#890).
- A build with `ecs` and without `script` is clean under `-D warnings` (#1058).
- A Landlock allowlist holds 4,096 rules, up from 64, and its paths sum to at most 1 MiB (#1114, ADR 0072).

## drishti 0.3.0, lgwks_ast 6.1.1, lgwks_macros 2.1.4

- `drishti` (a 0.x minor, so a breaking bump): the ink lab's `alpha`, `blur` and `displacement` and the runtime's `idle`, `epoch`, `uploaded_bytes` and `reused` are methods, not fields. The text painter takes cosmic-text 0.19 and stays left-aligned.
- `lgwks_ast`: each parse builds its own parser and keeps none between calls. A moved file counts only the lines its move changed (#1103).
- `lgwks_macros`: requirements only.
