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

1. **kache is the compile cache of every hosted job.** `hosted-rust` installs kache 2.3.0 from its release and checks each asset against the SHA-256 the release published. It makes kache the `RUSTC_WRAPPER`.
2. **One store, in GHCR.** The remote is `ghcr.io/logical-works-inc/kache-cache`, an OCI store with no 10 GiB cap. A `main` push writes it by kache's protected-branch policy, and the merge queue by `KACHE_GITHUB_WRITES=merge_group`. A pull request only reads it (`KACHE_REMOTE_READONLY=1`), so no change under review can put an output under another commit's key.
3. **Each compiling job ends with `kache-report`.** It stops the daemon, which drains the upload queue, and prints the hit rate to the log and the job summary.
4. **Two speeds.** A pull request runs the Linux inner loop: the gate, every lint leg and every Linux suite. The merge queue and `main` also run the 13 jobs a pull request skips: macOS, Windows, the GPUI, Metal and AppCUI legs, the target, feature and timing matrices, Miri, the release docs and the package smoke.
5. **The required row judges every job by name.** A pull request may skip only the `QUEUE_ONLY` jobs. In the queue and on `main`, a skipped job fails the row like a failed one.

## Alternatives rejected

- **kunobi-ninja/kache-action.** It has no OCI inputs, and its default store is the GitHub cache, the same 10 GiB.
- **An S3 store on R2 or on our own box.** Each needs a write secret in the repository. GHCR takes the job's own token.
- **Drop the rust-cache target now.** kache does not push build-script runs, so every job would run the tree-sitter and other C builds again. The target stays until the hit rate below says it can go.
- **Every job on every push.** That is the 310 s wall above.

## Consequences

- A pull request runs 21 fewer jobs and no macOS job. A defect only macOS or Windows sees is found in the merge queue, before it lands, not on the pull request.
- The merge queue must be on in the ruleset, or the queue jobs run only after the merge, on `main`.
- The first runs read an empty store. It fills from `main` and the queue.
- Reverse decision 2 if the hit rate on pull requests stays under 70 % after five `main` runs.

## Correspondence

Refines ADR 0041 (the cache budget) and ADR 0042 (the platform gates). Keeps INV-CI-RUNNER-CLASS-1: every job is still hosted.

## Verification

`every_compiling_job_compiles_through_kache` holds the action, the report and the workflow to decisions 1 to 3. It names each way of losing them. `the_queue_runs_what_a_pull_request_skips` holds `QUEUE_ONLY` to the jobs that skip a pull request. It runs the verdict step under bash for a pull request and for a queue run.
