# ADR 0016: a durable journal over a consumer's own record

Status: **accepted**, 2026-11-08. Implements #875.

Stakeholders: the Director; agent-harness authors (Larry); `lgwks_bot` maintainers.
Concerns: a harness that keeps its own ordered history has no durable log to put it on; a second torn-tail rule beside the shipped one; the cost of the `script` tree for a consumer that only wants a log.
Requirements: R9, R10
Viewpoint: the durability viewpoint, mapping what a process acknowledges to what survives its kill.
Elements and relations: `lgwks_bot::journal::RecordJournal`, `RecordEntry`, `RecordReplay` and `Durable`; the shared frame grammar in `journal::frame`; the owner thread in `journal::owner`; the seal step `file::seal::seal_on_owner`; the `record-journal` binary.

## Context

`FileJournal` persists `EffectEvent`. Its fence checks the effect ladder, its open folds outcomes, and its seal carries settled attempts. A consumer with its own commands and events cannot put them there, and its in-memory journal loses a run to any crash.

Making `FileJournal` generic was the first reading of the gap. The ladder, the outcome index, recovery and the sealed encryption path are all statements about one vocabulary, so a generic parameter would leave most of the file behind a bound nothing else satisfies.

## Decision

1. **A third store on the shared parts.** `RecordJournal<R>` is new. The frame grammar, the owner thread, the chain function and the seal step are the existing ones, called through narrowed visibility, so there is one definition of a torn tail.
2. **A tag opens every payload.** `0x01` is a record and `0x02` is a seal. A consumer's archived bytes can spell anything, so no prefix inside them can ever read as a seal.
3. **Offsets in memory, records on disk.** The handle holds one `u64` per frame, at most 800 KB. Paging and replay read from their own descriptor and verify the chain as they go.
4. **The bytes decide authority.** The seal frame is written identically into both files. A successor holding it whole is the journal. A seal with no such successor was never acknowledged and is taken back on the next open.
5. **The name decides what a first seal is.** A generation path with no carried seal is refused, since reading it as empty would start a second chain at the genesis. In a base file a first-frame seal is its own, from an empty journal sealing, and must name the genesis.
6. **A cursor behind the generation is refused.** Answering from the checkpoint would let a consumer folding by cursor skip sealed history silently, so `CursorBehindGeneration` names the first entry it can rebuild from.
7. **A continuing handle keeps one frame in hand.** It stops appending while one largest frame still fits, so filling the journal can never leave a history it cannot seal.
8. **Built under `wire` alone.** `Durable` moves from `script::run_store` to `journal`, with the old path re-exported, so a consumer does not pay for the script tree.

## Alternatives rejected

- **A generic `FileJournal`.** Most of its fields and its recovery are about effects, so the generic form would carry them unused or split the type anyway.
- **SQLite or redb behind the same interface.** A second storage engine brings a second recovery rule, and the frame grammar already answers every case the gap names.
- **A magic prefix instead of a tag.** A record that began with the prefix would read as a seal.
- **Decoding every record on open and holding them.** That costs 64 MiB of retained history per tenant at the ceiling, for a fold the consumer runs once.

## Consequences

- A consumer's restart is a fold over `replay`, and the crash semantics are the journal's rather than its own.
- A consumer pays for `wire` alone: `cargo tree -e normal` prints 36 unique crates, and a cold dev build took 17.4 s at 582 MB peak (macOS arm64, no compiler cache, 2026-11-08). The `script` tree it avoids was measured by that consumer at 140 crates, 31.0 s and 604 MB.
- `record-journal bench <path> 10000 256` printed p50 3,003 µs, p95 3,987 µs and p99 4,175 µs per synced 256-byte append at 3.1 MB peak RSS. A sync per record is the floor, so `compare_and_append_all` exists: 100,000 records in one flush took 3.36 s. Replaying them took 0.07 s at 3.2 MB.
- A byte-level hand who rewrites the file and every later head goes undetected. This is the same limit `FileJournal` states.
- Moving out `EffectJournal`, `Corruption` and the durable step split three files that were over the size law, with every public path unchanged.

## Correspondence

Refines ADR 0005, whose event-driven core this log can feed. Unblocks the broker adoption, the ECS projection and the scope cancellation record that land after it.

## Verification

- `journal::records::sim_records` holds the adversarial sims: `sim_records_pulled_plug_every_cut_keeps_every_whole_frame`, `sim_records_weird_bytes_any_flipped_byte_is_refused_and_untouched`, `sim_records_pulled_plug_each_seal_boundary_leaves_one_authority` and `sim_records_monkey_random_ops_match_the_model_and_replay_exactly` among them.
- `journal::records::sim_records_crash` ports a consumer's twenty crash-and-restore scenarios onto a test record and a fold.
- `tests/it/record_journal_cli.rs` kills the shipped binary mid-append with a real signal and checks every printed acknowledgment against the disk.
