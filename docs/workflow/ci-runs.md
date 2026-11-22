# Workflow §12: the CI run table

Moved from `WORKFLOW.md` §12 on 2026-10-22, when every Markdown file in a
repository was held to 120 lines (Director, 2026-10-10). Every row is a run of the
self-hosted layout retired on 2026-11-03 (province#9). The table is that layout's
record, and a phrase like "§12 fact 3" means the specification of its day.
The current specification is [`ci-layout.md`](ci-layout.md). The run log was
condensed on 2026-11-22: each row keeps its run id, commit, wall clock and the
cause or lesson, and drops the per-job timings that no later rule cites.

## The self-hosted layout, 2026-10 to 2026-11

| Run | Commit | Layout | Wall clock | Result and lesson |
|---|---|---|---|---|
| 37579691890 | 3c008cbf | 35 jobs, per-job targets, SSD temp | 21.4 min | pass, 7,500 job-seconds |
| 37607930551 | 3f331527 | 14 jobs, per-instance cold targets, HFS+ RAM temp | cancelled | six instances compiled every dependency at once; four jobs timed out |
| 37613720499 | 3a2a855f | + one shared sccache on a job's RAM disk | cancelled | the next job ejected the server's temp dir; HFS+ decomposed names broke two walk simulations |
| 37616318637 | 4993b33d | + APFS RAM temp, sccache over every target | cancelled | sccache hit 0 of 195 Rust compiles across target directories, so it was removed and jobs pinned |
| 37619560219 | 7b48f3f1 | 14 jobs pinned to 6 instances | 20.5 min cold, 10.0 min warm | cold: the feature matrix hit its timeout. Warm: both Linux jobs found no engine through the Docker context |
| 37624988459 | b9eadc91 | 11 instances, one per building job | 17.2 min cold, 8.5 min warm | both bot-full shards timed out beside the saturation tier's 50,000 process starts. A 100 ms parse deadline read 392 ms on the host clock (fixed: driven clock) |
| 37630747929 | 99670c70 | bot-full in three shards; seeded-clock deadlines | 13.0 min | four doc citations into `ecs.rs` moved with the source; five jobs compiled the same feature sets at once |
| 37633462471 | 436b9c9e | one job per feature set | 9.0 min | an artifact pattern missed the unsharded name; runners' `CI=true` turned incremental compilation off |
| 37635420955 | 89ba090b | + walk-complexity bound at the min window | fail | two timing flakes under load. Fixed, not weakened |
| 37651284179 | 627e4629 | + doc-citation re-anchor | 10.3 min | the hostile byte-ceiling tiling timed out at 88 s under load (fixed: tile past any deadline). 37650313690 was superseded by this run |
| 37654108625 | 6d792999 | + tiling fix; thirteen instances | 10.2 min | **pass.** Six jobs over the 5 min budget. Warm compiles took seconds and suites ran 3-10x slow: the machine was swapping (35 GiB), not compiling. The fix is §12 fact 3, not another layout |
| 37661085639 | f34c581d | + test opt-level 2, merged doc lanes | cancelled | a transition run: every instance rebuilt at the new profile, so it is not a verdict on the layout. Gate 279 to 112 s, workspace tests 390 to 165 s. Timeouts moved 15 to 20 min as a hang bound |
| 37663658069 | f380fb4a | + 20 min hang bounds | 8.7 min | **pass.** Down from 10.2. Six jobs still over budget, mostly compile-bound at opt-2 |
| 37666041415 | 40a3175d | + shard fusion, dudect 250k | fail | bot-full at ten threads broke INV-BOT-156's drain test (two cleanups Aborted past the grace under the process-table storm). Reverted to eight |
| 37668304200 | a2b50ff6 | + bot-full back to eight | 9.4 min | **pass.** Timing 343 to 237 s, nodefault 518 to 220 s. stdmatrix rose 129 to 561 s (12 cold opt-0 compiles serialized under the storm) |
| 37669895633 | e0ace416 | + test opt-level 3 | fail | the S4 scaling test read 6.5x growth on an unchanged path: 200k-call trials at opt-0 span about 14 ms and the storm hit every one. Fixed with 50k calls, fastest of nine, same bound. Build timed out on the 808 s opt-3 rebuild |
| 37673046041 | 57aad743 | + S4 fastest-of-nine | fail | the drain test again, and Linux-workspace on a stale partial unpack a timeout-killed job left in the shared registry volume (`File exists`, tree-sitter-nix). The script now removes partials first |
| 37675264254 | 448127f0 | + registry pre-flight | fail | six jobs died within 30 s of each other at about 15 min, swap at 96% (37 of 39 GB). One opt-3 rustc holds about 0.9 GiB, so two slots per job overflowed 24 GiB. Fix: one compiler slot per job, 30 min timeouts on the six compiler jobs for transitions |
| 37678318088 | 4f800797 | + one slot per job | 6.7 min, fail | only bot-full failed, on the drain test. Fix: shard-c rides the saturation instance, whose work is spawns, not compiles |
| 37679490381 | 306a1632 | + shard-c to saturation | 7.0 min | **pass.** Every added cold compile taxes the whole run, not its job. Suite thread shares then cut about 40% |
| 37680643459 | 1628b64d | + thread cuts | 8.7 min | **pass.** The cuts slowed every suite 20-30%: the suites are throughput-bound, so the shares were restored |
| 37681940177 | d601807f | + shares restored | 7.3 min | **pass, all 19 green including the drain.** 21.4 min fell to about 7 min warm. The wall moves by 100 s on identical code. The floor is total suite CPU (about 6,000 s) over fifteen cores, about 400 s. Sub-300 needs coverage cuts or hardware: the Director's call |

## Cold-compile cancellations at the 5 min cap

Runs 39909442106, 39944605346, 39956154743, 39960738059, 39964264885, 39966970016,
39969556595, 39971728446 and 39972898976 were all cancelled at the 5m0s cap while
runners compiled cold. None is a verdict on the code. The causes and routes:

- **A source change in `lgwks_bot`** (39909442106, 39944605346, 39956154743): the
  first run after the change compiled cold. Route: a docs-only push, which changes
  no cargo input, so each runner keeps its build. Run one PR at a time, because a
  second PR's run sends a runner back to cold. In 39956154743 the job printed
  `5131 tests run: 5131 passed (1 leaky), 3 skipped` and the cap cancelled it
  24 s later, during cleanup.
- **A toolchain pin move, Rust 1.99.0 to 1.99.1** (39960738059 on PR 738): 23 jobs
  were cancelled while every instance rebuilt every unit. Route: push after main
  is merged in, with nothing else on the runners until every job has compiled its set.
- **PR 738 at 1.99.1, runs two to six** (39964264885 to 39972898976): the jobs
  cancelled fell from 20 to 16, 10, 3 and 2 as each run resumed from the units the
  last one compiled. The finished jobs rose from 5 to 9, 15, 22 and 28. The last two were
  the saturation tiers, which start only after the earlier jobs pass, so this was
  their first cold window. Route: another push that changes no cargo input.
