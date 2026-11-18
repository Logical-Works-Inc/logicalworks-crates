# ADR 0066: CI builds on a dated nightly, releases build on stable

Status: **accepted**, 2026-11-18. The Director's orders were "Check for any new rust update and update rust version
as well" and "build on nightly with all optimizations and release on stable for slower version, Prioritize CI speed".

Stakeholders: the Director; a contributor whose pull request waits on CI; the person who publishes a release; every
crates.io consumer.
Concerns: CI wall time; a release built by a compiler no consumer runs; a nightly that breaks overnight.
Requirements: R13
Viewpoint: the build viewpoint, one toolchain per surface.
Elements and relations: `rust-toolchain.toml`, the `[toolchain]` table of `scripts/gate-lanes.toml`, the
`hosted-rust` action, every workflow's `toolchain:` pin, and rule 4 of `check-gate-parity`.

## Context

Every job built on stable 1.99.1, and stable 1.100.0 shipped. Stable has no parallel front end; nightly's
`-Zthreads` runs it. A crate a
consumer installs is still built by their stable compiler, so a release must prove itself there.

## Decision

1. `[toolchain] rust` is a dated nightly, `nightly-2026-11-12`. `rust-toolchain.toml` names it, so every local
   build and every pull request job uses it.
2. `[toolchain] release` is stable, `1.100.0`. Package smoke, the docs and release gates, Pages and the consumer
   gate pin it. `hosted-rust` exports `RUSTUP_TOOLCHAIN` for that pin, which rustup reads before
   `rust-toolchain.toml`. A release publishes with `cargo +1.100.0 publish --locked`.
3. On a nightly job, `hosted-rust` writes `-Zthreads` at the runner's core count into the runner's
   `$CARGO_HOME/config.toml`. The repository config holds no nightly key, so a stable build reads none.
4. The docs.rs lane runs `cargo doc` with no `+toolchain`, so it documents on the dated pin. `+nightly` outranks
   `rust-toolchain.toml`; the runner holds only the pin, so rustup fetched an undated nightly with no backend.
5. Rule 4 of `check-gate-parity` accepts a pin of `rust` or of `release`, and names any third version.
6. The Linux container images move to `rust:1.100.0-trixie`. They carry rustup, and `hosted-rust` installs the pin.
7. No test depends on what a toolchain bump moves. The nightly broke three that did. A test found `estate` by
   walking up from its own binary, and the build-dir layout moved it. A test held a `rust:1.99.1-trixie`
   literal. t02 picked the expected error code from the output it checked. Now the `std-first` lane refuses a
   path from `std::env::current_exe` navigated with `parent`, `with_file_name`, `pop` or `join`: read
   `NEXTEST_BIN_EXE_<name>` or `CARGO_BIN_EXE_<name>`. The platform test finds the suite rows by shape. t02 holds
   the `Evaluate` bound under any rustc code.

## Alternatives rejected

- **Stable 1.100.0 everywhere.** Supported, but no parallel front end: CI keeps a sequential front end.
- **A floating `nightly`.** A new nightly can red every pull request with no change here. The date moves in one
  commit.
- **Cranelift for workspace crates.** Tried in run 44384552848. On the arm64 runners a cranelift crate has
  `fp-armv8` off where LLVM's std has it on: rustc warned "mixing target features will cause an ABI mismatch" on
  every job. The test profile inherits `[profile.dev]`, so caught panics aborted (SIGABRT) in `sim_app_edge` and
  `sim_command_faults`. Reverse when a cranelift build on aarch64 prints no ABI-mismatch warning and the
  caught-panic sims pass.
- **A clippy ban on `std::env::current_exe`.** Nine tests re-run their own binary, the one path it names
  exactly, and the guard refuses an inline `expect`. A ban would push them to `argv[0]`, which is weaker.
  Reverse if clippy gains a lint for a navigated `current_exe` path.
- **Cranelift for dependencies.** They come from rust-cache and sccache, so it saves nothing, and some use
  intrinsics cranelift does not lower.

## Consequences

- The nightly lints more. Its first run found a `redundant_clone` in the COBOL reader and an unused `ExitCode`
  in `check_report.rs`; both are fixed in the change that lands this record.
- The date is 2026-11-12, not the newest. From rust-lang/rust#161922 (merged 2026-11-12) on, aarch64 codegen of
  pathfinder_simd 0.5.6, which gpui pulls in, ends in an ICE. Upstream's fix, servo/pathfinder `cf7c998`, is
  unreleased, and the deps register refuses a `[patch]`. The pin moves past it once a release carries the fix
  (#1089).
- A nightly bump is one commit: `rust-toolchain.toml`, `[toolchain] rust` and every nightly `toolchain:` move
  together, and rule 4 refuses a half-done one.
- The job times before and after are in the pull request that lands this record.

## Correspondence

Refines ADR 0028, the budget the toolchain serves, and ADR 0029, whose job now pins the nightly.

## Verification

- `check-gate-parity` accepts the release pin and names a third version:
  `a_release_pin_is_accepted_and_a_third_pin_is_refused`; a drifted nightly pin is named by
  `one_drifted_job_pin_fails` and `a_toolchain_mismatch_fails`.
- `hosted-rust` fails a job whose pin is neither `rust-toolchain.toml`'s channel nor `[toolchain] release`.
- The `std-first` lane refuses a guessed binary path and passes a binary that re-runs itself:
  `a_binary_path_guessed_from_current_exe_is_a_finding`.
- The docs.rs lane plans no `+toolchain`: `sim_docsrs_plans_name_no_toolchain_and_follow_the_manifest` and
  `sim_docsrs_plans_of_this_repository_name_no_toolchain`.
