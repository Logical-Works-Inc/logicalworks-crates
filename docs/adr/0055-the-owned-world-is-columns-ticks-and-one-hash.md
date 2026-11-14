# ADR 0055: the owned world is columns, ticks and one hash

Status: **accepted**, 2026-11-14, for #1002 under #985.

Stakeholders: the Director; the agent of W3 (#1003), W4 (#1004) and W5; an app author on the host.
Concerns: a tenant state that a cell can clone, log, replay and snapshot; bounded memory per tenant; one proof of
equality for two worlds.
Requirements: R1, R3, R7, R11
Viewpoint: the runtime-state viewpoint of ADR 0053: boxes are worlds, logs and snapshots, and arrows are who may
write whose bytes.
Elements and relations: `crates/lgwks-bot/src/world/` (`state`, `component`, `tx`, `codec`, `snapshot`, `digest`,
`error`), its simulations in `src/sim_world/`, the `world-bench` bin; relations: `owns`, `logs`, `replays`.

## Context

ADR 0053 made the world the private heap of one tenant. This record is the first world, W2 of #985.

- `lgwks_std::wire` states that rkyv bytes are not a content identity. A world hash over them would change with the
  rkyv version.
- `lgwks_std::id` has no v7 issuer that takes a clock and entropy. `V7Generator::issue_at` is `pub(super)`. The
  world cannot mint ids and stay deterministic.
- `cargo tree --locked -p lgwks_bot -i bevy_ecs -e normal --depth 0` printed `bevy_ecs v0.20.1` in ADR 0053.

## Decision

1. **Entities.** A slot is a dense `u32` index and a `u32` generation. The stable name is a v7 `Uuid` the caller
   supplies. The world reads no clock and draws no entropy. A non-v7 id is `NotV7`. The generation saturates and
   only helps a caller spot a stale handle.
2. **Columns.** One column per component type, set at build. A `Component` gives the canonical bytes of a value.
   `WorldBuilder::column` proves the bytes are a fixed point of the codec on a probe, or answers `CodecDrift`.
3. **Ceilings.** Each binds, and a step past one is a typed `Ceiling` with the limit and the ask.

   | What | Bound |
   |---|---|
   | columns | 64 (one `u64` mask) |
   | live entities | 16,777,216 |
   | live value bytes plus cell tables | 2 GiB (each slot costs one cell in each column) |
   | one value | 65,536 bytes |
   | operations in one transaction | 65,536 |
   | one transaction payload | 8 MiB, at 64 bytes of overhead per operation |
   | one change set on the wire | 16 MiB |
   | tenant name | 128 bytes |
   | one snapshot | `Ceilings::snapshot_bytes()` |

   A configured ceiling of zero, or past its maximum, is `CeilingConfig`.
4. **Ticks.** The world tick rises by one for each commit that holds an operation. `changed_since(tick)` is the
   only change query. It walks slots in index order and yields the entity, its id, whether it lives, and a mask of
   the columns that moved. A dead slot is a tombstone with no columns.
5. **Transactions.** A `Tx` holds `&mut World` and buffers operations. It checks every ceiling at the write, against
   an overlay of what the earlier writes did. A dropped `Tx` changes nothing. `commit` encodes the change set,
   applies it and returns the bytes. `replay` reads those bytes on another world of the same tenant and layout. It
   refuses a change set built on another tick as `Stale`, so a duplicate, a gap and a reorder are each refused.
6. **One hash.** `World::hash` is BLAKE3 over an explicit byte form, version 1, written in `world/digest.rs`. It
   covers the tenant, the layout, the tick, the slots, the free list and every cell in column and index order. It
   never reads rkyv output. The ceilings are not part of it.
7. **Snapshots.** `snapshot_since(tick)` writes the rows that moved and the whole free list. `restore` reads a
   chain on a clone and swaps the clone in only when every snapshot read clean. It checks the chain, the tenant,
   the layout, the order of rows and cells, the tick spans, the generation, the canonical form of every value and
   the free list. It checks the slot count a snapshot claims against the rows and free entries that name them
   before the table grows. A snapshot carries a format number, a history link and the world hash it must restore
   to, and a restore that misses it is `HashMismatch`.
   A change set carries a format number, the link of the history it continues and a BLAKE3 digest of its content
   (`Diverged`, `DigestMismatch`, `UnsupportedFormat`). The digest finds damage. It does not prove who wrote it.
8. **Errors.** `WorldError` is a layered enum with `Limit` and `Defect`. It is not a variant of `BotError`. Every
   refusal logs at `warn` before it returns.
9. **Visibility.** The module is private. It compiles under `cfg(all(test, feature = "wire"))` and into the
   `world-bench` bin by path. Both callers use every item, so the dead-code lint stays on. #1004 removes the `cfg`
   and the include together.
10. **The bench is a bin.** It prints sample counts and p50, p95 and p99 per operation at 1,000, 100,000 and
    1,000,000 rows. A criterion target would add a dependency.

## Alternatives rejected

- **A hash over the rkyv bytes.** It is cheaper to write. It breaks when rkyv changes, and the wire module says so.
- **Mint the ids in the world.** It needs a clock and entropy inside the world. A test could not seed it.
- **A tick for each operation.** It gives every operation its own tick. A transaction is the unit of change, so
  the tick follows the commit.
- **Public now.** A `pub` world has no caller outside the crate until #1004, and the guard refuses it.

## Consequences

- `changed_since` walks every slot. At 1,000,000 rows it took p50 376 us. A tick index would cost memory and
  is not built.
- The world holds no lock. The cell owns it (ADR 0053 decision 5).
- The sims mint their own v7 ids. The gap in `lgwks_std::id` stays open until an injectable issuer exists.
- `bevy_ecs` stays as the tick engine until W3 (#1003) races the two. The world replaces it only if it wins.
- Measured with `/usr/bin/time -l target/release/world-bench` on one run: maximum resident set 919,928,832 bytes
  for the whole run, with 1,000,000 rows live. The table is one run, not a rival comparison. W3 does that.

| Rows | get p50 | write-commit(1 op) p50 | batch(100 ops) p50 | changed-since p50 | hash | snapshot |
|---|---|---|---|---|---|---|
| 1,000 | 41 ns | 458 ns | 22 us | under 1 us | under 1 ms | under 1 ms |
| 100,000 | 250 ns | 791 ns | 49 us | 32 us | 18 ms | 25 ms |
| 1,000,000 | 458 ns | 1,041 ns | 72 us | 361 us | 146 ms | 232 ms |

The table shows p50. The same run gives p95 and p99 at 1,000, 100,000 and 1,000,000 rows:
- `get` (ns): 42 and 42; 500 and 625; 1,084 and 1,500.
- `write-commit` (ns): 500 and 542; 959 and 1,042; 1,709 and 1,958.
- `batch-commit` (us): 23 and 25; 71 and 122; 88 and 103.
- `changed-since` (us): 0 and 0; 32 and 107; 436 and 467.
Samples: 10,000 for `get` and `write-commit`; 1,000 for `batch-commit`; 100 for `changed-since`; one each for the rest.

## Correspondence

- Refines ADR 0053 decisions 1, 3, 4 and 6 with a world that holds them. It builds on ADR 0010 for the v7 id.
- ADR 0052 plans a `lgwks_world` crate. This module moves there with its tests.

## Verification

- The `doc-governance` lane holds this record's fields.
- ADR 0053 decision 1 and 6: `sim_world_same_seed_same_hash`, `prop_snapshot_roundtrip`, `sim_world_replay_equals_live`.
- ADR 0053 decision 3: `sim_world_two_tenants_same_ids`.
- ADR 0053 decision 4: `sim_world_dropped_tx_leaves_bytes`.
- The ceilings: `sim_world_refuses_past_ceiling`. Hostile bytes: `sim_world_hostile_bytes_refused`. Each refusal has
  its variant in `restore_meets_each_defect_with_its_variant` and `replay_meets_each_defect_with_its_variant`.
- Integrity: `sim_world_saboteur_flipped_byte_in_snapshot_refused`, `sim_world_liar_upstream_snapshot_claims_huge_slots_refused_before_alloc`, `sim_world_giant_and_empty_zero_byte_values_hit_a_ceiling`, `sim_world_foreign_history_at_the_same_tick_refused`.
- `tests/it/core_boundary.rs` holds that `world` names no driver and no `bevy`.
- Decision 2 of ADR 0053, the shared subscription, is `review only` until W5, owner the agent of W5, dated
  2026-11-14.
