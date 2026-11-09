# ADR 0024: a view fed ahead of its journal

Status: **accepted**, 2026-11-09. Implements #897.

Stakeholders: the Director; consumers that batch writes with a group commit; `lgwks_bot` maintainers.
Concerns: a view that can show a record a crash then loses; a second poison rule beside the one a catch-up keeps; a log kept only in memory having no way into a view.
Requirements: R9, R10
Viewpoint: the durability viewpoint, mapping what a view shows to what survives a kill.
Elements and relations: `RecordProjection::empty`, `RecordProjection::fold_one`, `RecordEntry::staged`; the `record-projection` binary's `stage` and `sync` commands.

## Context

`RecordProjection` could only fold what a `RecordJournal` file returned. A runtime that batches its writes reads its view right after staging a record, before the sync, and a determinism simulation keeps its log in memory with no file to read.

## Decision

1. **One public way to fold one entry.** `fold_one` has the rules a catch-up has, and `catch_up` now calls it, so there is one definition of poisoning.
2. **An entry a consumer can make.** `RecordEntry::staged` takes the sequence the record will have. No chain holds it, so its head is the genesis constant and it is never a checkpoint.
3. **The journal stays the truth.** A view fed ahead holds what the file does not until the sync lands. A restart rebuilds from the file, and what was never written is gone from both.
4. **The cursor stops a second fold.** A catch-up reads past the view's cursor, so the file reaching a fed record does not fold it again. Nothing compares the two, so a host that writes something else at that sequence must rebuild.

## Alternatives rejected

- **An in-memory `RecordJournal`.** A second backend under the chain and seal rules would have to match the file's on every one, to give a simulation what it can already do by feeding the view.
- **Requiring the chain head in `staged`.** The head is a hash over bytes that are not written yet, so a consumer could only invent it.
- **Comparing fed records against the file at catch-up.** It would need the view to retain every fed record until the sync, unbounded by anything the view owns.

## Consequences

- A consumer sees its own write at once and must say so honestly: what its view shows before the sync is not yet a fact.
- `max_rows` stays the only row bound, and a consumer maps its own limit onto it.
- A fold reading `position().head()` of a staged entry reads a constant.

## Correspondence

Refines ADR 0016, whose journal stays the truth.

## Verification

- `tests/it/sim_record_projection.rs`: `sim_record_projection_pulled_plug_a_group_commit_matches_the_model_and_replays` runs staged writes, syncs and crashes against a model of the file, and `sim_record_projection_clock_fool_a_view_ahead_of_the_file_does_not_refold` holds decision 4.
- `tests/it/record_projection_cli.rs`: `a_kill_before_the_sync_loses_what_the_view_showed_and_a_kill_after_keeps_it` kills the shipped binary with a real signal.
