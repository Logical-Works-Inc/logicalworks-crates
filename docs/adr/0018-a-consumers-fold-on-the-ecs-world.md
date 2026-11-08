# ADR 0018: a consumer's fold on the ECS world

Status: **accepted**, 2026-11-08. Implements #877.

Stakeholders: the Director; agent-harness authors (Larry); `lgwks_bot` maintainers.
Concerns: a read model that drifts from its log; rows read in arrival order; half a fold passing as whole; a view with no ceiling.
Requirements: R9, R10
Viewpoint: the state viewpoint, mapping a durable log to the view a restart recovers.
Elements and relations: `lgwks_bot::RecordProjection`, `RecordFold` and `ProjectionError`, private to `ecs/projection.rs`; `RecordJournal` from ADR 0016; the `record-projection` binary.

## Context

A consumer that keeps its history in a `RecordJournal` still folds it by hand, and its view decides what a restart means. It should be able to put that view on the ECS world without its reducer naming the world.

## Decision

1. **The journal is the truth.** `rebuild` is a constructor that folds the live generation, so a restart and a rebuild are one operation and nothing half built comes back.
2. **The world is private.** A row is a component on an entity, and consumers see rows through `row` and `rows`. Only `ecs/` names the ECS library.
3. **Key order, not entity order.** A `BTreeMap` from key to entity orders every read, because an entity id records when a row was made.
4. **A failure poisons.** After a refused `catch_up` every later one answers `Poisoned`, since the fold may have changed its own state before refusing.
5. **A bound the caller names.** `rebuild` takes `max_rows`. A new key past it is `RowLimit`, and a key already held is not refused.
6. **Order is checked.** A record that is not the one after the last applied is `Diverged`.

## Alternatives rejected

- **A generic journal method that folds.** The journal would learn about rows and the ECS library, which `core_boundary` keeps out of it.
- **A plain `BTreeMap` of rows.** It is simpler, and it would leave the consumer's order for a view on the world unanswered.
- **Retrying a failed record.** A fold's refusal is about the record's meaning, so a retry cannot change it and hides the failure.

## Consequences

- A continued journal is read from its checkpoint, so a fold must keep what it needs in that record. `sim_record_projection_pulled_plug_a_continued_journal_rebuilds_from_its_checkpoint` shows the shape.
- A record that a fold refuses stays in the journal, so a rebuild refuses it too until the consumer's fold changes.
- A rebuild costs one fold of the whole generation.

## Correspondence

Builds on ADR 0016's `RecordJournal`.

## Verification

`tests/it/record_projection_cli.rs` kills the binary and compares digests. `tests/it/sim_record_projection.rs` holds the view against a reference model over 48 seeds, and against a saboteur fold and a thousand tenants.
