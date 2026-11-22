# ADR 0073: gpui builds from Zed main until crates.io holds the fix

Status: **accepted**, 2026-11-22.

Stakeholders: the Director; drishti, whose native ink lab host runs on gpui; a consumer that selects the
`lgwks_deps/gpui` feature.
Concerns: gpui 0.2.2 is the newest gpui on crates.io. It pulls `block 0.1.6` and `proc-macro-error2 2.0.1`, and rustc
reports both as future-incompatible (rust#74840, rust#127909). The Director's order is 0 lint findings, always.
Requirements: INV-SEC-AUDIT-2.
Viewpoint: the resolved dependency graph of the workspace and its fixtures.
Elements and relations: `lgwks_deps` features `gpui`, `contract/PATCHES.toml`, `contract/APPROVED.toml`,
`drishti::native::run`.

## Context

Zed main moved to `block2` and `stacksafe` 1.0.3 (zed#62468), so main resolves neither crate. No release since 0.2.2
holds that fix. On main the platform backends also left gpui for `gpui_platform`, and the call
`Application::new()` is gone. `gpui_platform` and its backend crates are `publish = false` in Zed's workspace.
The request to publish all of them is zed#67338.

## Decision

1. **`gpui` and `gpui_platform` build from Zed main at `f126950f`.** Each manifest line names `git` and `rev` with a
   `version` beside it, the same shape as the candle pin. The pin is an `upstream-pin` row in `contract/PATCHES.toml`.
2. **drishti starts its window with `gpui_platform::application()`.** It reaches the crate through
   `lgwks_deps::gpui_platform`, behind the same `gpui` feature.
3. **No lock resolves a future-incompatible crate.** A test reads the workspace `Cargo.lock` with the shipped reader.

## Alternatives rejected

- **Keep gpui 0.2.2 and wait.** The two crates stay in every build that selects the feature, and the lint count is
  not 0. Reverse this if Zed main stops building on the pinned nightly.
- **`[patch.crates-io]`.** The deps register refuses it, and a patch reaches no consumer.
- **Republish gpui under our name.** That is a fork of about 40 Zed crates. A third-party republish is a supply-chain
  risk the register does not admit.

## Consequences

- The workspace, its fixtures and CI resolve neither crate.
- A publish of `lgwks_deps` names `gpui_platform`, which crates.io does not hold, so that publish waits on zed#67338.
  Reverse this ADR when it ships: drop `git` and `rev`, delete the PATCHES row and run `cargo update -p gpui`.
- The Zed history is 418 MB, so `.cargo/config.toml` sets cargo's `shallow_deps`: a depth-1 fetch is 26 MB in 4 s.
  Without it every cold hosted job spent 23 s fetching, and the first run walled at 306 s.
- `pathfinder_simd` 0.5.6 stays in the tree through Zed's font-kit fork, so #1089 is unchanged.

## Correspondence

Answers #1122. Asks zed#67338.

## Verification

`crates/lgwks-deps/tests/it/workspace_lock.rs`: `the_workspace_lock_resolves_no_future_incompatible_crate` fails on
the 0.2.2 lock and names both crates. `cargo run -p drishti --features ui --example ink_lab -- --frames 600 --json`
opens the window on Zed main's gpui.
