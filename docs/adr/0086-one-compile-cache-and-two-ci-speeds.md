# ADR 0086: one compile cache, and two CI speeds

Status: **accepted**, 2026-11-24.

Stakeholders: the Director; an agent whose pull request waits on CI.
Concerns: a run past its 300 s wall with every job green; lanes that compile cold on every pull request; macOS queue waits.
Requirements: R13
Viewpoint: the gate viewpoint, one row per CI job and the cache it reads.
Elements and relations: `.github/actions/hosted-rust`, `.github/actions/kache-report`, the jobs and the `Build and workspace tests` row of `.github/workflows/ci.yml`.

## Context

The Director, 2026-11-24: "DO YOU THINK GOOGLE IS RUNNING 10+ MINUTES OF CI?" and "Crates NEEDS to use Kache".
Gemini's review for the Director named the cause: agents push many pull requests, and each push ran 70 jobs on three operating systems.

Two facts from the runs:

- Run 45434287873 (PR 1146): every job passed, and the required row failed at 310 s. The macOS `bot-it-darwin` row ran 278 s and ended last.
- GitHub holds 10 GiB of cache per repository. The rust-cache entries fill 9 GiB (ADR 0041), so the clippy, docs, timing and feature lanes save nothing and compile cold on every pull request. sccache was off in every job.

kache keys each rustc output by its inputs. It caches bins, test binaries, build scripts and clippy runs, which sccache cannot. Measured on the Mac at a new checkout path: kache served 224 of 226 crates in 34 s; sccache served 141 of 200 in 112 s.

## Decision

1. **kache is the compile cache of every hosted job.** `hosted-rust` installs kache 2.3.0 from its release and checks each asset against the SHA-256 the release published. It makes kache the `RUSTC_WRAPPER`. A build saved through kache has `-kache-` in its rust-cache key. A job that restored a build without the mark runs no wrapper and reuses that build, because cargo rebuilds every unit when the wrapper changes. In run 45471907504 such a job compiled 399 of 399 crates through the empty store, 130 to 196 s a shard against main's 76 to 95 s.
2. **One store, in GHCR.** The remote is `ghcr.io/logical-works-inc/kache-cache`, an OCI store with no 10 GiB cap. A `main` push writes it by kache's protected-branch policy, and the merge queue by `KACHE_GITHUB_WRITES=merge_group`. A pull request only reads it (`KACHE_REMOTE_READONLY=1`), so no change under review can put an output under another commit's key. A job gets 15 minutes on a `main` push and 5 elsewhere. In run 45492643765 every main job was cancelled at 5 minutes while it compiled cold, so it uploaded nothing.

   kache's scheduler is off, because a hosted job runs one cargo. With it on, cargo ran half its jobs in run 45504674831: 125 s against 76 s for the same 8 crates.

   One job writes each build. In run 45627908295 GHCR answered 429 to almost every upload ("0 ok, 48 failed" on workspace slice 1). Every slice of a sliced build compiles the same crates, so slices 2 and up only repeated the first slice's writes. Now only a job with no slice, or slice 1, writes, with at most 4 remote operations at once (`KACHE_S3_CONCURRENCY=4`). Every other job only reads. This is reversed if a main run with one writer per build still reports a 429.
3. **Each compiling job ends with `kache-report`.** It stops the daemon, which drains the upload queue, and prints the hit rate to the log and the job summary.
4. **Two speeds.** A pull request runs the Linux inner loop: the gate, every lint leg and every Linux suite. The merge queue and `main` also run the 14 jobs a pull request skips: the build of every target, macOS, Windows, the GPUI, Metal and AppCUI legs, the target, feature and timing matrices, Miri, the release docs and the package smoke. A pull request type-checks every target in the workspace clippy leg. The build that links them all ran cold on a pull request, 0 of 573 crates from kache, which only `main` and the queue write (run 45462381296).
5. **The required row judges every job by name.** A pull request may skip only the `QUEUE_ONLY` jobs, and the `BOT_ONLY` jobs when decision 6 routed them off. In the queue and on `main`, a skipped job fails the row like a failed one.
6. **A pull request runs the suites its change can break.** The `changes` job sorts each changed path. The lgwks_bot rows skip when no path is a bot input: a reader page, drishti, or a test, example or bench of a crate lgwks_bot builds on, outside `tests/support/`. They skip only when `main` cached its bot listings under the hash of the bot inputs, because the simulation lane counts them. Every unknown path is a bot input.
7. **Pre-flight is the commit door.** An agent's push comes only from the post-commit hook, after `rust-guard: GO COMMIT`. The commit door needs a green `design-gate` receipt for the staged tree, and pre-commit runs the repository's parity lane. The suites run on the persistent boxes through `lgwks-box`, never on a hosted runner first.
8. **A failed run ends with one failure receipt.** The `ci-receipt` binary reads each failed job's log. It writes each failure's job, kind, file and line, diagnostic code, seed and repro command. The receipt goes to the job summary, an artifact, and one pull request comment that each failed run edits.

## Alternatives rejected

- **kunobi-ninja/kache-action.** It has no OCI inputs, and its default store is the GitHub cache, the same 10 GiB.
- **An S3 store on R2 or on our own box.** Each needs a write secret in the repository. GHCR takes the job's own token.
- **Drop the rust-cache target now.** kache does not push build-script runs, so every job would run the tree-sitter and other C builds again. The target stays until the hit rate below says it can go.
- **Every job on every push.** That is the 310 s wall above.
- **A matrix built from JSON in the `changes` job.** Rule 9 of `check-gate-parity` reads the rows in `ci.yml`. A matrix `exclude` cannot remove `include` rows. So each step of a bot row reads the route.
- **Skip the bot rows with no listings.** The workspace suite alone is 1,676 simulations of 3,777 tests (44 %), so the simulation lane would refuse every routed pull request.

## Consequences

- A pull request runs 22 fewer jobs and no macOS job. A defect only macOS or Windows sees is found in the merge queue, before it lands, not on the pull request.
- The merge queue must be on in the ruleset, or the queue jobs run only after the merge, on `main`.
- The first runs read an empty store. It fills from `main` and the queue.
- Reverse decision 2 if the hit rate on pull requests stays under 70 % after five `main` runs.
- A routed bot row still starts a runner and its container, then skips every step.
- A bot test that reads another crate's file at run time, not through `#[path]` or `include_str!`, is not seen by the route. The merge queue runs every row, so such a break stops in the queue.

## Correspondence

Refines ADR 0041 (the cache budget) and ADR 0042 (the platform gates). Keeps INV-CI-RUNNER-CLASS-1: every job is still hosted.

## Verification

`every_compiling_job_compiles_through_kache` holds the action, the report and the workflow to decisions 1 to 3. It names each way of losing them. `the_queue_runs_what_a_pull_request_skips` holds `QUEUE_ONLY` to the jobs that skip a pull request. It runs the verdict step under bash for a pull request and for a queue run. The four `ci_changes` tests run the classify step under bash. They hold the route table to `cargo metadata` and to every file lgwks_bot includes. They also show the key moves only with a bot input. `a_failed_row_writes_one_failure_receipt` holds decision 8 and refuses each way of losing the receipt. The `ci_receipt` tests and sims hold the binary to the two real failed jobs and to their seeded variants.
