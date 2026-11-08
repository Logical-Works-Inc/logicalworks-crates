# lgwks_std 4.1.0 / lgwks_ast 6.0.0 / lgwks_deps 6.2.0 / lgwks_macros 2.1.0 / lgwks_bot 8.0.0 / drishti 0.1.3

Released 2026-11-08. A consumer's own durable log and what folds over it, a fence a generation does not answer, a stop that survives a kill, and an edge that answers health and identity itself.

## lgwks_bot 8.0.0 (major)

Major because `lgwks_bot` names `lgwks_ast` types in its public items (`rt::serve::RouteMethod`, `Language`) and `lgwks_ast` moved a major.

- `RecordJournal` keeps a consumer's own record kinds. An acknowledgment exists only after the frame is written and synced, and committed bytes that do not re-derive the chain are refused and left as found (#875, ADR 0016).
- INV-FENCE-1: the broker takes a reported generation, `revoke_operation` withdraws one way, and a counted offline bound is spent. INV-FENCE-1 refuses a reach in one order: closed, revoked, stale, spent (#876, ADR 0017).
- `RecordProjection` folds records onto the ECS world in sequence under a named row limit, and a refused catch-up refuses every later one until rebuilt (#877, ADR 0018).
- `JournaledCancel` moves a stop through requested, cancelling and finished. An acknowledged request survives a kill and fires the restored token (#878, ADR 0019).
- The tenant store keeps a forward-only cursor per tenant and consumer. Snapshots are format 2, and an older snapshot falls back to the replay it already had.
- Both edges answer `GET` and `HEAD` `/healthz` with no tenant (#879). `App::authenticate` runs before the verb, the body and the mailbox, a refusal is `401` with nothing queued, and `App::route_as` hands the handler only the `Principal` (#880, ADR 0020).
- `GatedIdentifiedSpawn` holds its child behind an anonymous pipe in place of `SIGSTOP` polling (#808).

## lgwks_ast 6.0.0 (major)

- **Breaking:** the `thiserror` re-export is gone, so `extern crate lgwks_ast as thiserror;` no longer resolves. Use `lgwks_ast::define_error!`, or `#[derive(lgwks_macros::Error)]` for a `source` or a `From`.
- `error_derive` is the zero-`syn` expansion behind that derive (#842), and `thiserror` is retired from the register.

## lgwks_macros 2.1.0 (minor)

- `#[derive(lgwks_macros::Error)]` reads `#[error("..")]`, `#[error(transparent)]`, `#[from]` and `#[source]` (#842). `sim_derive_error` holds it outside the crate against a model.

## lgwks_std 4.1.0 (minor)

- `process::capture_descendants_many` snapshots many roots from one table read (#798).
- `id::tuple` packs the FoundationDB tuple typecodes under named bounds (#851). The ECS identity index in that issue is not in this release.

## lgwks_deps 6.2.0 (minor)

- `lgwks-deps advisories` evaluates a lockfile against a local RustSec checkout (#799). The digest is FNV-1a-128.
- `lgwks new` pins `lgwks_bot` 8.0.0 and `lgwks_std` 4.1.0.

## drishti 0.1.3 (patch)

- Builds against `lgwks_bot` 8.0.0, with no change of its own.

## Not in this release

ADR 0014's "What landed" says what of its first delivery was removed. `math` (#837), `ec`, `vz`, `warden`, `SqliteExecutor` and the `web-loco`, `pki-x509`, `pdf-sign`, `mcp-remote`, `ui-preview` and `proptest` features are not shipped. #815 and #816 are not started.
