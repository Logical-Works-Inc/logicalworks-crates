# ADR 0019: a stop and a place a restart keeps

Status: **accepted**, 2026-11-08. Implements #878.

Stakeholders: the Director; agent-harness authors (Larry); `lgwks_bot` maintainers.
Concerns: one fact held in a token and a journal; a stop a restart forgets; a stage read out of order; a saved place that moves back or crosses tenants.
Requirements: R9, R10
Viewpoint: the state viewpoint, mapping durable records to what the next process does.
Elements and relations: `lgwks_bot::script::JournaledCancel`, `CancelStage`, `CancelRecord`, `CancelError`; `RecordJournal` from ADR 0016; `TenantStore::advance_cursor`, `TenantStore::cursor`; the `durable-stop` binary.

## Context

A scope's cancellation is an in-memory token. A consumer whose cancellation is a journaled transition keeps two copies of one fact, and its restart reads only one. The same consumer pages over its journal with a position that has no durable home.

## Decision

1. **The journal holds the stop.** `JournaledCancel` writes through the consumer's own record type, through `CancelRecord`, into the journal the consumer already owns.
2. **One order.** `Requested`, `Cancelling`, `Finished`. A step out of order is refused and writes nothing, and a record out of order is `Diverged` at restore.
3. **Record, then fire.** `request` appends and then fires the token, so an acknowledged stop survives a kill. A refused append still fires the token, because a full disk must not hold back a stop, and the refusal says the record is missing.
4. **The tenant store holds the place.** A cursor is a new record kind keyed by tenant and consumer. It only moves forward, saving the same place writes nothing, and a tenant holds at most 64.

## Alternatives rejected

- **Journaling inside `Scope::cancel`.** It returns nothing, so a refused append would have nowhere to go, and every scope would carry a journal handle.
- **A cancellation log of our own.** That is the second copy of the fact this removes, in a second file.
- **A cursor in the journal.** Each saved place would use up the generation it pages over.

## Consequences

- `Scope::cancel` and a supervisor's token stop a run in memory and a restart forgets them. Only `request` records.
- A continued journal is read from its checkpoint, so the checkpoint must carry the stage of any stop still open.
- `restore` reads the whole generation, once per stop.
- The snapshot format is now 2, so a store's first open after upgrading replays its frames once. An older build meets a cursor frame as a record it does not know, and refuses the store rather than misreading it.

## Correspondence

Builds on ADR 0016's `RecordJournal`.

## Verification

`tests/it/durable_stop_cli.rs` kills the binary after a request and a saved place. `tests/it/sim_journaled_cancel.rs` and `tests/it/sim_tenant_cursor.rs` hold both to models over seeded moves, with a saboteur, a full journal and two tenants.
