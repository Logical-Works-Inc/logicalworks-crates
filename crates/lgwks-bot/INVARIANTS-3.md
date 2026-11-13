# `lgwks_bot` invariants, part 3

The third `lgwks_bot` part of the invariant register, one short line per rule: rule · why · enforced by. Part 1
is [`INVARIANTS.md`](INVARIANTS.md), part 2 is [`INVARIANTS-2.md`](INVARIANTS-2.md), and the rest is in the root
[`INVARIANTS.md`](../../INVARIANTS.md).

- **INV-BOT-159** A child's environment is the in-order fold of its spec's deltas, and `env_clear` makes it an allowlist. · why: #337 · enforced by: `tests/it/sim_process_env.rs`
- **INV-BOT-160** A lease minted on one clock is judged on another only within a declared skew bound. · why: #278 · enforced by: `tests/it/sim_skew.rs`
- **INV-BOT-161** Two editors of one record give one winner and a typed `Conflict`, never a lost update. · why: #278 · enforced by: `tests/it/sim_cas.rs`
- **INV-BOT-162** A retrying proxy reuses each key and payload byte for byte, so the upstream applies each key once. · why: #278 · enforced by: `tests/it/sim_retry_proxy.rs`
- **INV-BOT-170** `keyed::open_once` keeps one effect per caller key on a receiver with no idempotency key. · why: #676 · enforced by: `tests/it/sim_keyed.rs`, `tests/it/gh_issue.rs`
- **INV-BOT-171** A host's runs share one retry budget whose credit expires and never pools past twice the reserve. · why: #647 · enforced by: `src/script/sim_policy.rs`
- **INV-BOT-172** An `Estate` drops without parking its thread, and case-variant tenants never share a store file. · why: #735 · enforced by: `tests/it/estate.rs`
- **INV-BOT-173** Every placed task yields exactly one terminal outcome. · why: #703 · enforced by: `tests/it/sim_supervise_reports.rs`
- **INV-BOT-174** `EcsBot::replace_grants` swaps grants between ticks without touching another bot. · why: #716 · enforced by: `tests/it/grant_replace.rs`
- **INV-BOT-175** The ECS world names no driver: only `ecs/` names bevy, and no core module names tokio or axum. · why: ADR 0006 · enforced by: `tests/it/core_boundary.rs`
- **INV-BOT-176** A gated identified spawn is recorded before it runs and execs only on `release`. · why: #691 · enforced by: `tests/it/rt_process.rs`
- **INV-BOT-177** A run enters the world through the journal, and each settled run fires once across a restart. · why: ADR 0006 · enforced by: `tests/it/sim_run_events.rs`
- **INV-BOT-178** A `RequestFeed` hands a request only its tenant's cell and sheds a full mailbox as `503`. · why: ADR 0006 · enforced by: `src/ecs/requests/sim_requests.rs`
- **INV-BOT-179** A world ticks only when a tick is due, and one wake polls exactly one source. · why: ADR 0006 · enforced by: `src/ecs/wake/sim_wake.rs`
- **INV-BOT-180** A panicking message rebuilds only its tenant, without the message, and is answered `500`. · why: #790 · enforced by: `src/cell/sim_cell.rs`
- **INV-BOT-181** An `App` decodes a body before its handler, and an effect needs a granted cap. · why: #791 · enforced by: `src/app/sim_app.rs`
- **INV-BOT-182** Every durable step reaches the store through one `RunRecords` door that `conformance::check` holds. · why: #781 · enforced by: `tests/it/run_records.rs`
- **INV-BOT-183** An `await approval` parks holding no task, and a gated `act` runs only on an approval of its bytes. · why: #784 · enforced by: `tests/it/sim_approval.rs`
- **INV-BOT-184** `fetch`, `ask` and `send` each reach outside through one typed word under declared budgets. · why: #783 · enforced by: `tests/it/sim_outside_bounds.rs`
- **INV-BOT-185** `serve` takes the tenant from the authenticated path and runs each request key once. · why: #782 · enforced by: `tests/it/sim_serve.rs`
- **INV-BOT-186** A `RunStore` gives one recorded answer per step key, even inside one group-commit batch. · why: #784 · enforced by: `tests/it/sim_approval_extra.rs`
- **INV-BOT-187** A sealed record is ciphertext under its own key, and shredding the key keeps the chain verifiable. · why: #785 · enforced by: `tests/it/sim_sealed.rs`
- **INV-BOT-188** A model-written plan is admitted against the tenant's `Surface` and runs as compiled `script!` does. · why: #786 · enforced by: `tests/it/sim_apps.rs`
- **INV-BOT-189** A dead durable sink refuses the start before anything is served. · why: #820 · enforced by: `tests/it/start_gate.rs`
- **INV-BOT-190** The OpenAPI 3.1 document is emitted from the serve route registry. · why: #830 · enforced by: `tests/it/serve_openapi.rs`
- **INV-BOT-191** Capability tokens verify offline and attenuate only narrower. · why: #779 · enforced by: `tests/it/cap_tokens.rs`
- **INV-BOT-192** `IdempotentPost` is applied only on a 2xx, never follows a redirect, and leaves a 5xx or a failure after sending unsettled. · why: #910 · enforced by: `idempotent::sim_answer`, `tests/it/retry_proxy.rs`
- **INV-BOT-193** A whole-document store replace syncs the file before its rename and the directory after it. · why: #910 · enforced by: `durable::sim_durable`, `tests/it/sim_cas_disk.rs`
- **INV-BOT-JOURNAL-LOCK** A second `FileJournal::open` of a held file is `JournalError::Locked` before a byte is read. · why: #641 · enforced by: `journal::file::tests::refusals`
- **INV-KERNEL-1** `lgwks_bot::kernel` is the supported path to the runtime modules, and a removal there is a major release. · why: #845 · enforced by: `tests/it/kernel_surface.rs`
- **INV-EPOCH-1** A `RecordedInstant` archives as eight little-endian bytes, and a seeded `EpochClock` reads no real clock. · why: #845 · enforced by: `tests/it/sim_recorded_instant.rs`
- **INV-RECORD-1** A `RecordJournal` acknowledges a frame only after it is synced, and corrupt bytes are refused. · why: #875 · enforced by: `journal::records::sim_records`
- **INV-FENCE-1** A broker refuses a reach in one fixed order: closed, revoked, stale, spent. · why: #876 · enforced by: `tests/it/sim_broker_fence.rs`
- **INV-PROJECTION-1** A `RecordProjection` folds in sequence within its row limit until a refusal poisons it. · why: #877 · enforced by: `tests/it/sim_record_projection.rs`
- **INV-STOP-1** A journaled stop moves forward only and survives a kill. · why: #878 · enforced by: `tests/it/sim_journaled_cancel.rs`
- **INV-EDGE-1** Both edges answer `/healthz` themselves, and authentication runs before the verb. · why: #879 · enforced by: `tests/it/sim_app_edge.rs`
- **INV-COMMAND-1** A request held across a command is answered on every path, and a rebuild never reruns it. · why: #898 · enforced by: `app::sim_command`
