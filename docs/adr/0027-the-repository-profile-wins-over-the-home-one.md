# ADR 0027: the repository's profile wins over the home one

Status: **accepted**, 2026-11-11.

Stakeholders: the Director; every session building this repository on a shared Mac; CI.
Concerns: a disk filled by one repository's build output; a compile cache that cannot hold workspace crates; a profile the manifest declares and no build uses; app journeys that are the last tests of their CI slice.
Requirements: R9
Viewpoint: the build viewpoint, mapping a cargo profile to the compiles it caches and the bytes it leaves.
Elements and relations: `.cargo/config.toml`, the `dev` and `test` profiles of `Cargo.toml`, sccache through the home `rustc-wrapper`, the scratch `.cargo/config.toml` that `tests/support/lgwks_scratch.rs` writes beside every scaffolded app.

## Context

A cargo config beats the manifest. A developer's home config set `[profile.dev] incremental = true` and `debug = 1`, so the manifest's `debug = 0` never built here. `test` inherits from `dev`, so every test build was incremental too. sccache refuses an incremental compile. On 2026-11-11 it had refused 33,032 of 142,357 calls, a 33.66 % hit rate, and `target/debug/incremental` held 12 GB.

A scaffolded app lives outside the repository, so no repository config reaches it. Each app journey compiled the whole estate cold with full debuginfo. On main's run 42632155761 the two app journeys were the last tests of Linux slice 9 (110.6 s and 73.1 s), and on #916 that slice ran past its five-minute limit.

## Decision

The repository's `.cargo/config.toml` sets `[profile.dev] incremental = false` and `debug = 0`. The scratch config beside every scaffolded app copies that table from it, so one table governs both.

## Alternatives rejected

- **Changing the home config.** Every repository and session on the machine shares it, and they would all rebuild at once.
- **Setting `CARGO_INCREMENTAL=0` in each command.** A variable is forgotten, and sccache hashes `CARGO_*` variables.
- **Writing the profile into the scratch config as its own text.** A second copy drifts from the first.
- **Caching the app build directory in CI.** It needs a workflow edit, and a cold run still pays the full debuginfo build.

## Consequences

- An edit recompiles its crate whole, rather than reusing its incremental state.
- Measured on this Mac, the app journeys from an empty app build directory: `new_check_expand_run_serves_one_request` 59.8 s and `expanded_app_passes_clippy` 56.5 s, 70.1 s wall and 642 MB peak RSS. The CI timings above came from different hardware, so these numbers are not matched; the PR's CI run is the comparison on the same runners.

## Correspondence

Relates to WORKFLOW.md on local CI, which runs each lane group in its own target directory.

## Verification

- `tests/it/lgwks_e2e.rs` (`a_scaffolded_app_builds_under_the_workspace_dev_profile`) requires the scratch config to carry `incremental = false`, `debug = 0` and the patch. The copy fails the journeys if the repository config loses the table.
