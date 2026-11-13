# ADR 0044: a record states only what the device proved

Status: **accepted**, 2026-11-13, for #910 defects 2, 3 and 8.

Stakeholders: every consumer of `RecordJournal`, `FileJournal`, `IdempotentPost`, the CAS
`RecordStore` and the tenant snapshot; every operator who restarts a run after a fault.
Concerns: a fault read as a fact. An unreadable successor read as absent, an unknown effect read
as not applied, and a replace that a power loss can undo.
Requirements: INV-BOT-7, INV-BOT-192, INV-BOT-193
Viewpoint: the durability viewpoint, one fact per record.
Elements and relations: `journal::records` and `journal::file::seal` (the successor read),
`idempotent` (the answer table), `durable` (the replace) and its callers `cas` and
`tenant_store::snapshot`.

## Context

Both journals asked "does the successor exist?" and read every open or read error as "no". A
looping name or a bad directory then rolled back an acknowledged seal, or opened a sealed base
as live. `IdempotentPost` read a 5xx and every transport failure as `NotDelivered`, so the ledger
recorded "not applied" for effects that may have run. It also followed redirects, and a hop to
another origin drops the key. CAS and the snapshot each renamed a file without syncing it or its
directory.

## Decision

- A successor read that fails for any reason but `NotFound` is `JournalError::Storage`. The base
  is not touched.
- `IdempotentPost` never follows a redirect. A 2xx is applied. 401, 403 and 404 are the
  credential repair. 408, 425 and 429 are not delivered. Any other 4xx but 409 is refused. Every
  other status, and every failure after the connect stage, is `EffectIndeterminate`.
- One `durable::replace_durably` writes a scratch file, syncs it, renames it and syncs the
  directory. CAS and the snapshot call it. The scratch name is the full name plus
  `.lgwks-scratch`.

## Alternatives rejected

- **Retry the successor read.** A loop or a wrong type does not clear itself. A retry only delays
  the same wrong answer.
- **Keep following redirects and add the key back on each hop.** `lgwks_std::http` drops caller
  headers on a cross-origin hop by design. Overriding that leaks credentials to a new origin.
- **Put the replace in `lgwks_std::fs`.** Two callers, both in this crate. A public item with no
  outside caller is refused (SD-6). It moves when a second crate needs it.

## Consequences

- A consumer that saw `NotDelivered` for a 5xx now sees `EffectIndeterminate`. The run holds the
  key as a barrier and reconciles, which is the documented contract of that variant.
- A replace costs two syncs. A Windows build skips the directory sync, as before.

## Correspondence

Refines ADR 0016 (`RecordJournal`). Indexed in CODEBOOK.md's `journal::records` row.

## Verification

- `journal::sim_successor_fault` breaks the successor with a loop, a file and a removal.
- `idempotent::sim_answer` checks every `u16` status and every stage and kind against a model.
- `tests/it/retry_proxy.rs` drives the adapter through real sockets.
- `durable::sim_durable` and `tests/it/sim_cas_disk.rs` walk the replace against a model.
