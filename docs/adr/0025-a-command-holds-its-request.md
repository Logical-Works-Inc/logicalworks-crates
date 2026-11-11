# ADR 0025: a command holds its request

Status: **accepted**, 2026-11-09. Implements #898.

Stakeholders: the Director; consumers whose route must answer with what a durable write did; `lgwks_bot` maintainers.
Concerns: a blocking write stalling every tenant on a worker; a client hanging on an answer that was dropped; a retry writing twice; a rebuild running a write again; one tenant filling a shared queue.
Requirements: R9, R10
Viewpoint: the effect viewpoint, mapping a route's request to the service call it waits on and the answer it hears.
Elements and relations: `Effect::Command`, `Provider::command`, `CommandLimits`, `App::command_limits`, the `app.command` cap; the `app::command` module beside the cell host; the `command-journal` binary.

## Context

An app's route answered from its handler or not at all. A route whose answer is what a durable write did had no way to wait: `Effect::Ask` answers the client first and feeds the service's answer back later, and running the write in the handler blocks the worker every tenant on it shares. An answer refused by a full mailbox was also dropped with a debug line, while whoever waited on it hung.

## Decision

1. **A handler returns a command, and the request stays open.** With no `Effect::Reply` in the same batch, the first `Effect::Command` holds the request's reply slot until an `on` handler answers it. A reply beside it answers at once, and later commands in the batch fold without an answer.
2. **A command runs off every worker.** A bounded pool of owned threads serves `Provider::command`, joined when the host closes. A cell's rebuild never re-runs one, since effects are not run on replay.
3. **Three bounds, each with its answer.** Past the threads a command queues. Past the host's or the tenant's ceiling the client hears `503` at once, and nothing ran. A request held past the deadline hears `504`, and a command queued past it is not run.
4. **Every waiter is answered.** A ticket dropped on any path answers `503` from its own `Drop`, a refused answer settles the waiter with a `503` that names a retry, and a held request nothing answered is a `500`.
5. **A command is whole or not run.** `bound` does not clip its body, because a cut command is a different act. A body past `MAX_MESSAGE_BYTES` answers `Unavailable` unrun, and an answer past it is a `502`.
6. **The asker travels with the chain.** The principal reaches the provider, and an `on` message carries it, so a command an `on` handler returns acts for the same identity.
7. **Idempotency is the service's.** A kill between the write and the reply fails the request and leaves the write. Only the service can tell a retry from a new act, so `command-journal` shows the pattern: a key per write, its table rebuilt from the journal, and a group commit.

## Alternatives rejected

- **A route kind that runs on the host's state outside the cell.** It needs a second path for state, and a second definition of rebuild, to save one trip through the mailbox.
- **Blocking in the handler.** It is the failure this answers: every tenant on the worker waits on one tenant's sync.
- **Running a command on the driver as a task.** A blocking service would stall the driver, which also carries the edge and its liveness.
- **One global ceiling.** A noisy tenant would fill it and refuse its neighbours.
- **Clipping a command as `Ask` clips a prompt.** A shortened prompt is a worse question, and a shortened write is a wrong one.

## Consequences

- A service that never returns holds its thread and the host's close, as it would hold a worker if served inline. The client still hears its `504`.
- A late answer still folds after its client heard `504`, and its second completion of the slot is dropped.
- The default limits (32 threads, 1,024 held, 256 per tenant, 30 s) are estimates, not measurements of any consumer's service.
- Each app granted `app.command` costs its threads for the host's life, and an app with no grant starts none.
- `Effect` and `Provider` grew: a provider that does not serve commands answers `Unavailable`.

## Correspondence

Relates to ADR 0016, whose journal `command-journal` writes, ADR 0019, and ADR 0020, whose principal a command carries.

## Verification

- `crates/lgwks-bot/src/app/sim_command.rs` and `sim_command_faults.rs` hold each decision against seeded fools, among them `sim_command_neighbour_a_tenant_at_its_limit_leaves_the_next_served`, `sim_command_pulled_plug_a_rebuild_never_runs_a_command_again` and `sim_command_monkey_every_answer_matches_the_model_and_a_seed_replays`.
- `crates/lgwks-bot/tests/it/app_command.rs`: `a_hung_service_answers_504_and_its_late_answer_still_folds` over a real socket.
- `crates/lgwks-bot/tests/it/command_journal_cli.rs`: `a_kill_between_the_write_and_its_reply_leaves_one_write_and_the_retry_finds_it` kills the shipped binary with a real signal.
- INV-COMMAND-1 in `INVARIANTS.md`.
