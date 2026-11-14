# `lgwks_bot` invariants, part 1

The first `lgwks_bot` part of the invariant register, one short line per rule: rule · why · enforced by. Part 2
is [`INVARIANTS-2.md`](INVARIANTS-2.md), part 3 is [`INVARIANTS-3.md`](INVARIANTS-3.md), and the rest is in the root
[`INVARIANTS.md`](../../INVARIANTS.md).

- **INV-BOT-1** A live settlement is journaled before it is acknowledged. · why: cef8059b (#112)
- **INV-BOT-2** A settlement binds to its generation, a transition to its payload, and a dispatch digest to its input. · why: ac2fbc11, 3534e9e4 (#114)
- **INV-BOT-3** An abandoned entry and a recovered unknown are barriers, never skipped past. · why: fe1a7fde, 98872468 (#113)
- **INV-BOT-4** A known effect outcome is kept even when its record fails to land. · why: bb3e152e (#111)
- **INV-BOT-5** Observation fingerprints commit only with their values. · why: 78fa72d8 (#110)
- **INV-BOT-6** Stale effect controllers are fenced by generation and lease. · why: d0610245 (#137)
- **INV-BOT-7** A journal read failure is an error, never `NoSuchWork`, and an unreadable successor is never an absent one. · why: 7f3dd387 (#139), #910 · enforced by: `journal::sim_successor_fault`
- **INV-BOT-8** A state watch that returns to a value it landed before fires again (edge, not level). · why: 3291773c (#138)
- **INV-BOT-9** Process ownership covers descendants, cleanup reaps the tree, and a deadline counts from enable time. · why: bd9a372b, 2f88bad5 (#132)
- **INV-BOT-10** External handoffs are admitted on the real dispatch path, and locator-ladder eligibility is enforced. · why: fff9d335, da0a3471 (#116)
- **INV-BOT-11** Builders that can be dropped unused are `#[must_use]` and state their authority. · why: bb2ee99f (#94)
- **INV-BOT-12** A process-group signal is sent only while the unreaped leader pins the group id. · why: #143 · enforced by: `tests/it/process_ownership.rs`
- **INV-BOT-13** Cleanup pending after its task ends moves to the bounded cleanup owner, and only observed absence frees capacity. · why: #143 · enforced by: `rt::supervise::tests`
- **INV-BOT-14** Shipped journals refuse appends and opens past their ceilings without deleting evidence. · why: #143 · enforced by: `file::tests::open_refuses_an_over_limit_file_without_truncating_it`
- **INV-BOT-15** One owner serializes journal writes, and a dropped waiter poisons the handle. · enforced by: `tests/it/sim_journal.rs`
- **INV-BOT-16** A clamped scale measurement records the level requested, the level reached and the ceiling. · enforced by: `tests/it/sim_scale::tier_r*`
- **INV-BOT-17** A `BotSpec` builds only through `assemble`/`build` with authority from the caller's `GrantSet`, all or nothing. · why: #87, #388 · enforced by: `tests/it/sim_spec_materialize.rs`
- **INV-BOT-18** A supervised process reports only what it observed: bounded capture, exit, deadline and cleanup receipt. · why: #151 · enforced by: `tests/it/sim_process.rs`
- **INV-BOT-19** An `EPERM` from `killpg` on a present, unreaped group means present, and cleanup stays pending. · enforced by: `rt::supervise::tests::an_unsignalable_present_group_stays_pending_rather_than_failed`
- **INV-BOT-20** A `Host` task takes one permit per host for its tree; waiting is cancellable and the trail ring is bounded. · why: #87, #386 · enforced by: `tests/it/sim_cancel_under_load.rs`
- **INV-BOT-21** Structural inspection parses through `lgwks_ast` and never executes the subject. · why: #150 · enforced by: `tests/it/inspect_non_execution.rs`, `tests/it/sim_inspect.rs`
- **INV-BOT-30** One declared `Clock` governs every deadline, and a paused logical clock never disables the wall watchdog. · why: #152 · enforced by: `tests/it/sim_clock.rs`
- **INV-BOT-31** `Supervisor::snapshot` reads the owner's own admission state, and its capped listing counts what it dropped. · why: #152 · enforced by: `tests/it/inspect_contract.rs`
- **INV-BOT-32** `Scope::require` refuses a missing capability at the reaching step as one `FlowError::Blocked`. · why: #87 T23 · enforced by: `tests/it/repair.rs`
- **INV-BOT-33** A repair resumes one run once for exactly its ticket's needs, and the host grant never widens. · why: #87 T23 · enforced by: `tests/it/sim_repair.rs`
- **INV-BOT-34** A repair ticket's identity is its content; a repeat or an old epoch is refused and leaves the ledger as it was. · why: #87 T24 · enforced by: `tests/it/sim_repair.rs`
- **INV-BOT-35** A run's root budget is charged by every attempt and repair and never refilled. · why: #87 T13 · enforced by: `a_spent_budget_refuses_every_later_attempt`
- **INV-BOT-36** Each repair refusal arm is observable alone and leaves the file byte-identical. · why: #87 T24 · enforced by: `a_mixed_decision_order_pins_each_arm`
- **INV-BOT-37** Durable repair state is read back by a second host from the file, never a handle. · why: #87 T23 · enforced by: `a_repaired_run_survives_a_reopened_host`
- **INV-BOT-38** A first-step reach declared with `Task::requiring` is `Blocked` before its body at no cost. · why: #87 T23 · enforced by: `the_step_that_reaches_is_the_step_that_blocks`
- **INV-BOT-50** A durable record is synced on the store's owner thread, so no executor thread waits in a flush. · why: #122 · enforced by: `tests/it/resume_liveness.rs`
- **INV-BOT-51** The journal and the run store share one frame grammar in `journal::frame`. · why: #87 · enforced by: `journal::frame::tests`
- **INV-BOT-52** A durable step's cost is measured against a plain step and a journal append at one payload. · why: #87 · enforced by: `crates/lgwks-bot/examples/resume_cost.rs`
- **INV-BOT-53** The run store holds every record at 10,000 concurrent runs of two tenants, read back after a reopen. · why: #87 · enforced by: `tests/it/sim_store_scale.rs`
- **INV-BOT-54** A host with a run store fsyncs each `remember` step before returning it. · why: #87 · enforced by: `tests/it/task_resume.rs`, `tests/it/sim_task_resume.rs`
- **INV-BOT-55** A recorded step replays only under the definition identity that produced it; drift is typed by axis. · why: T15 · enforced by: `tests/it/sim_replay_drift.rs`
- **INV-BOT-56** `Broker::adopt` claims the generation after the journal's committed history, never a fresh 1. · why: T16 · enforced by: `tests/it/sim_epoch_identity.rs`
- **INV-BOT-57** Each durable-ladder boundary recovers its own answer, and a kill during recovery changes no byte. · why: T14 · enforced by: `tests/it/durable_crash_observation.rs`
- **INV-BOT-58** Each identity field is refused by its own check as a typed variant, and counters never wrap. · why: T12 · enforced by: `tests/it/wrong_identity_evidence.rs`
- **INV-BOT-59** A run-store read failure reaches the caller as `FlowError::Store`, never as a drift. · why: T15 · enforced by: `tests/it/store_read_failure.rs`
- **INV-BOT-60** A readiness is a typed, generation-bound fact released without sleeping or polling. · why: #87 T18 · enforced by: `tests/it/sim_ready.rs`
- **INV-BOT-61** Service readiness is read from the child's own stdout in the read that captures it, with no timer. · why: #87 T18 · enforced by: `tests/it/ready.rs`
- **INV-RT-BOUNDED** No channel is unbounded: `clippy.toml` disallows tokio's `unbounded_channel` and `std::sync::mpsc`, and `rt::sync::mpsc::channel` replaces them. · why: RUST-AP-022, #911 · enforced by: the `clippy-feature-bot` lane
