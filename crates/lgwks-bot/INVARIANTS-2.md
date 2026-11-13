# `lgwks_bot` invariants, part 2

The second `lgwks_bot` part of the invariant register, one short line per rule: rule · why · enforced by. Part 1
is [`INVARIANTS.md`](INVARIANTS.md), part 3 is [`INVARIANTS-3.md`](INVARIANTS-3.md), and the rest is in the root
[`INVARIANTS.md`](../../INVARIANTS.md).

- **INV-BOT-110** A child's framed output is read through the one `journal::frame` grammar, charged before allocation. · why: #87 T05 · enforced by: `tests/it/sim_process_output.rs`
- **INV-BOT-111** A captured stream reports head, capacity, total and truncation from one drain bounded by its ceiling. · why: #87 T05 · enforced by: `tests/it/sys_process_binding.rs`
- **INV-BOT-112** Cleanup signals the live leader's descendants by pid in bounded rounds and names the survivors. · why: #263, #272 · enforced by: `tests/it/process_escape.rs`
- **INV-BOT-113** A callback that never yields is detected from outside by a timed child and a real `SIGKILL`. · why: #87 T03 · enforced by: `tests/t03_non_yielding.rs`
- **INV-BOT-114** A capture's own cut reads as `CeilingReached`, never as the child's truncation or a clean end. · why: #87 T05 · enforced by: `tests/it/sys_process_binding.rs`
- **INV-BOT-115** The sys verbs report `stdout_frames` from the bytes `stdout` decodes. · why: #87 T05 · enforced by: `a_binary_record_round_trips_through_frames_while_stdout_is_lossy`
- **INV-BOT-116** Every test store directory comes from `shared::Scratch`, so no run inherits another's files. · why: a stale store · enforced by: `tests/it/sim_repair.rs`
- **INV-BOT-150** A lying length prefix is `JournalError::Corrupt` with the file intact; only a cut append is trimmed. · why: #262 · enforced by: `tests/it/sim_journal_tail.rs`
- **INV-BOT-151** `Supervisor::with_tenancy` admits by deficit round robin, so no tenant passes its ceiling. · why: #268 · enforced by: `tests/it/sim_tenancy_model.rs`
- **INV-BOT-152** A continued journal loses and duplicates nothing, with one authority after a crash at any seal step. · why: #267 · enforced by: `tests/it/sim_continuation_seal.rs`
- **INV-BOT-153** A reading is admitted only when two reads agree on length, mtime and BLAKE3. · why: #278 · enforced by: `tests/it/sim_stability.rs`
- **INV-BOT-154** A credential lifetime runs on the declared clock, and a lapsed proof is `CredentialExpired`. · why: #278 · enforced by: `tests/it/sim_credential.rs`
- **INV-BOT-155** A source may skip its poll only when its revision equals the one committed with the value. · why: #279 · enforced by: `tests/it/sim_change_ticks.rs`
- **INV-BOT-156** Shutdown never reports a process that answered its token as aborted. · why: #263 · enforced by: `shutdown_reports_every_draining_cleanup_rather_than_aborting_it`
- **INV-BOT-157** `reap_orphaned_group` signals only a recorded pid whose start instant still matches. · why: #318 · enforced by: `tests/it/orphan_reap.rs`
- **INV-BOT-158** A cleanup that starts after its leader exited reads no process table and claims the group only. · why: #347 · enforced by: `a_leader_that_exited_on_its_own_reads_no_table_and_claims_no_tree`
- **INV-BOT-163** On Windows a supervised process joins a job object, and cleanup is confirmed only by an empty job. · why: #263 · enforced by: `rt::supervise::job_cleanup::tests`, `tests/process_job.rs`
- **INV-BOT-164** A thousand trees cancelled at 1,000 in flight leave no descendant in `/proc`. · why: #263 · enforced by: `tests/it/process_fire.rs`
- **INV-BOT-40** `FileJournal::replay` streams the acknowledged history holding at most one event. · why: #122 · enforced by: `tests/it/sim_journal_liveness.rs`
- **INV-BOT-41** A durable journal reserves room for every rung before the first one. · why: #156 · enforced by: `ecs::tests::an_external_handoff_reserves_settlement_capacity_before_any_rung`
- **INV-BOT-42** Applied-key membership is answered by index, never by a quadratic scan. · why: #156 · enforced by: `ecs::tests::applied_membership_answers_at_scale_where_a_scan_would_be_quadratic`
- **INV-BOT-43** The durable write runs on the storage owner thread, so a parked device leaves the runtime free. · why: #122 · enforced by: `tests/it/journal_liveness.rs`
- **INV-BOT-44** A real `SIGKILL` during an unanswered append leaves a clean journal, and the retry lands once. · why: #122 · enforced by: `tests/it/durable_crash_observation.rs`
- **INV-BOT-45** Tenant appends over separate files stay isolated at 10,000 journals with bounded tails. · why: #122 · enforced by: `tests/it/journal_scale.rs`
- **INV-BOT-46** A registry identifier declared twice in one role is refused with both positions. · why: #122 · enforced by: `tests/it/registry.rs`
- **INV-BOT-47** A committed append with a lost reply is settled by readback, never resent. · why: #118 · enforced by: `tests/it/ambiguous_commit.rs`
- **INV-BOT-70** `Host::run` never exceeds its admission ceiling at 10,000 runs and releases every permit on drop. · why: #203 · enforced by: `tests/it/sim_task_axes.rs`
- **INV-BOT-80** A GitHub publication is reported only from an independent read-back at the pinned head. · why: #151 · enforced by: `tests/it/pr_review_journey.rs`, `tests/it/sim_review_pr.rs`
- **INV-BOT-81** A review reads the named repository's bounded diff as data, and its outcomes stay distinct. · why: #151 · enforced by: `tests/it/gh_binding.rs`
- **INV-BOT-90** Model output is untrusted input admitted by a bounded decoder against the host's `Surface`. · why: #87 T26 · enforced by: `tests/it/sim_proposal.rs`
- **INV-BOT-91** A completion claim is `Admitted` only with its evidence and full coverage. · why: #87 T27 · enforced by: `tests/it/proposal.rs`
- **INV-BOT-92** A durable `Checkpoint` round-trips through the run store, bounded. · why: #87 T27 · enforced by: `a_checkpoint_round_trips_through_the_run_store`
- **INV-BOT-93** Artifacts are keyed by `(tenant, digest)`, and writes to one key are serialized and idempotent. · why: #87 T28 · enforced by: `tests/it/sim_proposal.rs`
- **INV-BOT-94** Repeated unchanged failure reaches a finite `Intervention`, and new evidence never refunds spend. · why: #87 T29 · enforced by: `repeated_unchanged_failure_reaches_a_finite_intervention_t29`
- **INV-BOT-95** Untrusted output reaches a run only through `script::admit`, whose refusals are typed and located. · why: #87 · enforced by: `an_injected_instruction_is_refused_at_its_step_on_a_real_run`
- **INV-BOT-96** Each review refusal arm is its own seeded property. · why: #231 · enforced by: `tests/sim_review_families/subject_arms.rs`
- **INV-BOT-97** Saturation tiers reach a verified `Published` per run, one create per receiver. · why: #151 · enforced by: `tests/sim_review_path.rs`
- **INV-BOT-100** `Host::submit` reattaches a repeated request and refuses a changed payload as `Conflict`. · why: #87 T30 · enforced by: `tests/it/request_key.rs`
- **INV-BOT-101** A dropped waiter or a host stop leaves the request on disk as `Submission::InFlight`. · why: #87 T17 · enforced by: `tests/it/request_key.rs`
- **INV-BOT-102** A host stop is never a request's recorded outcome. · why: #87 T30 · enforced by: `tests/it/sim_request_key.rs`
- **INV-BOT-120** A baseline marked unsound by a `RefreshReason` is polled until a committed read spends the mark. · why: #87 T08 · enforced by: `tests/it/sim_observe_refresh.rs`
- **INV-BOT-121** A value replaced before any generation took it is reported as superseded. · why: #87 T09 · enforced by: `tests/it/observe_refresh.rs`
- **INV-BOT-122** A chain held open does not starve an independent chain. · why: #87 T06 · enforced by: `tests/it/observe_refresh.rs`
- **INV-BOT-123** A poll pending past its wall deadline is dropped and reported while its neighbours act. · why: #87 T06 · enforced by: `ecs::tests::an_expired_wave_still_reads_a_source_ready_on_its_next_turn`
- **INV-BOT-124** One lazy watchdog serves each observation wave, and a wave whose sources all answer starts none. · why: #87 T06 · enforced by: `tests/it/observe_refresh.rs`
- **INV-BOT-130** Concurrent appends share one `sync_all` per batch, and a failed batch acknowledges none. · why: #152 · enforced by: `a_failed_batch_flush_acknowledges_nobody_and_folds_nothing`
- **INV-BOT-131** Group-commit evidence is read from a reopened store, never the acknowledging handle. · why: #152 · enforced by: `tests/it/durable_crash_group_commit.rs`
- **INV-BOT-132** One storage owner serves every durable store with `Settled`, `Unsynced` and `Committed` answers. · why: #152 · enforced by: `tests/it/sim_group_commit.rs`
- **INV-BOT-140** An awaited answer registers its waker before reading the slot, so no wakeup is lost. · why: #239 · enforced by: `an_answer_published_during_registration_still_wakes_the_poll`
