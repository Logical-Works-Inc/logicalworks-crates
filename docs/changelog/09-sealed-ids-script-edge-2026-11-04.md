# lgwks_std 4.0.0 / lgwks_ast 5.0.0 / lgwks_deps 6.0.0 / lgwks_macros 2.0.0 / lgwks_bot 7.0.0 / drishti 0.1.0

2026-11-04

Every crate moves to `lgwks_std` 4.0.0, whose `json::Value` is this
crate's own tree ([notes](08-own-json-value-2026-11-03.md), #831). A crate whose
public items carry that type takes a major bump with it.

## lgwks_std 4.0.0 (breaking)

- `json::Value`, `json::Map` and `json::Text` are this crate's own (#831).
- `legacy`: EBCDIC, packed decimal, COBOL copybooks, the compound-file
  container and its VBA streams, Access rows and fax images, each under
  `legacy::limits` (#792).
- `Uuid::now_v7() -> Uuid` never refuses. It shares `new_v7`'s generator, and
  a stamp past 48 bits pins at the ceiling (INV-ID-4, #851).
- `Uuid::pack_tuple` / `unpack_tuple`: the FoundationDB tuple-layer UUID
  element, `0x30` then 16 bytes, with typed `TupleError` refusals
  (INV-ID-5). `Uuid` implements no serde trait (ADR 0010). The rest of the
  tuple layer and the ECS identity surface stay open on #851.

## lgwks_ast 5.0.0 (breaking)

- The `legacy` and `script` verdicts return `lgwks_std` 4's `json::Value`.
- `script::emit::script` writes the Rust for a parsed script (#787), the one
  emitter `script!` and `lgwks expand` share. `script::tree` lost the items
  that moved there.
- `legacy` comprehends COBOL and JCL (#777).
- The parser reads `await approval`, `fetch`, `ask`, `send`, `serve` and
  `route` (#781 to #784).

## lgwks_deps 6.0.0 (breaking)

- The register is schema 3 (#788): every approved edge names how it ends
  (`[approved.retire]`), what it costs and when it was last reviewed, so
  `lgwks-deps check` refuses a schema-2 register it used to pass.
- The `lgwks` binary: `new`, `check`, `expand`, `run`, `legacy` and `id`
  (#787, #792, #851). `lgwks new` pins `lgwks_bot = "=7.0.0"` and
  `lgwks_std = "=4.0.0"`.
- The `sealed-aead` storefront feature re-exports the one AEAD the sealed
  records use (#785). candle is pinned to the upstream zip 9.1.3 fix (#793).

## lgwks_macros 2.0.0 (breaking)

- `script!` parses with `lgwks_ast::script::parse` and calls
  `lgwks_ast::script::emit::script`, so it compiles byte for byte what
  `lgwks expand` prints (#787).
- New words: `await approval`, `fetch`, `ask`, `send`, `serve` and `route`.
  Their expansion calls `lgwks_bot` 7 items, so 2.0.0 needs `lgwks_bot` 7.

## lgwks_bot 7.0.0 (breaking)

- `lgwks_bot::json` is `lgwks_std` 4's `json`.
- Per-tenant cells replace the request feed's mutex partitions (#790), and the
  app SDK registers routes and `on` handlers as cell transitions (#791).
- `script::approval`: a human decision that survives restart (#784).
- `script::fetch`, `script::ask` and `script::send`, the outside words (#783),
  and `rt::serve` with `serve::route`, the tenant HTTP edge (#782).
- `sealed`: the journal and run-store records are sealed, with crypto-shred
  and erasure (#785).
- Model-written plans are admitted and run over HTTP (#786).

## drishti 0.1.0

The first upload of 0.1.0. The 2026-10-29 entry listed it, but crates.io held
0.0.1 alone until now. The crate itself is unchanged since then.
