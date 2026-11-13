# ADR 0037: the it group rows share one dependency build

Status: **accepted**, 2026-11-13. The Director's orders were "WHY IS CACHING SO SHIT" and
"HOW MANY TIMES HAVE I FUCKING SAID UNDER 90 SECOND TESTING AND SUB 5 MINUTE CI".

Stakeholders: the Director; every pull request whose jobs restore main's cache.
Concerns: main's cache past GitHub's 10 GB repository cap, and the entries its eviction drops.
Requirements: R13
Viewpoint: the gate viewpoint, one row per CI job and the cache entry it saves.
Elements and relations: `ci.yml`'s `tests_linux` group rows, `scripts/it_group_rustc.rs`, the
`hosted-rust` action's rust-cache key, and `logical_ci.yml`'s rust-cache entry.

## Context

On 2026-11-13 main held 52 cache entries, 18.3 GB, against GitHub's 10 GB repository cap.
LRU eviction dropped `v0-rust-logical-ci-8e2f36847f12-Darwin-arm64` minutes after main's run
43340723109 restored it. That run hit its key exactly, and rust-cache saves only on a miss, so
nothing wrote the entry back. The next pull request's logical_ci compiled cold: its prebuild
took 7 m 46 s, and the job's 10 minutes cancelled the lane (run 43340940282).

Eleven of the 52 entries were one build. Each Linux `it` group row (scale, surface, refusals,
g1 to g8) builds `-p lgwks_bot --test it --features full --cargo-profile ci-bot`, and passed
its group cfg in `RUSTFLAGS`. RUSTFLAGS reaches every dependency, so each row compiled and
saved its own copy of the dependency graph, 450 to 550 MB each and 5.4 GB in all. The macOS
rows had already moved the cfg into `scripts/it_group_rustc.rs`, a workspace wrapper.

## Decision

The Linux group rows pass `LWC_IT_GROUP` to the same wrapper. The job compiles it with `rustc`
alone before any cargo call, and sets `RUSTC_WORKSPACE_WRAPPER` to its one path. Only workspace
crates see the cfg, so every row compiles one identical dependency build under one
`bot-it` cache key. `logical_ci.yml`'s timeout is 15 minutes, so a cold job finishes both its
compile and its lane.

## Alternatives rejected

- **A larger cache cap.** The setting is billed to the enterprise account, and the API refuses
  it without a payment method (HTTP 402). It would also hold 11 copies of one build.
- **Saving on a hit.** Saving an entry that already exists changes nothing in GitHub's LRU,
  and eviction would drop it again.
- **A Cargo feature per group.** That adds test-only features to a published crate.

## Consequences

- Main's cache drops by about 5 GB (estimated from the entry sizes, not yet measured), and
  eleven rows restore one entry, which the first row to finish saves.
- The group rows' workspace crates still compile per group, as before.
- Main still holds about 13 GB of entries. Eviction can still drop an entry, and the job whose
  entry goes builds cold until main's next miss saves it again.

## Correspondence

Extends the macOS rows' wrapper decision to `tests_linux`. ADR 0036's prefix restore for
`bot-it` also matches the old per-group entries, which hold the same paths and rebuild by
fingerprint. Described in `docs/workflow/ci-layout.md`.

## Verification

- `check_gate_parity::workflow_calls::tests::the_it_group_rows_share_one_dependency_build`
  holds the committed ci.yml, and names a group cfg in RUSTFLAGS, a per-group key, or a row
  without the wrapper.
- After the first main run, `gh api repos/Logical-Works-Inc/logicalworks-crates/actions/caches`
  lists one `v0-rust-bot-it-<hash>-Linux-arm64` entry in place of eleven.
