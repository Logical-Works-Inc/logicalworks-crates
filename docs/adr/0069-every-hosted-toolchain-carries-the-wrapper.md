# ADR 0069: every hosted toolchain carries the wrapper

Status: **accepted**, 2026-11-20.

Stakeholders: the Director; a reader of the project page; any job that pins the stable release toolchain.
Concerns: from 227b0738 (#1090) every Pages run on main failed. The step `cargo run -q` exited 1 in 0.1 s and
printed nothing.
Requirements: INV-CI-TOOLCHAIN-1, INV-CI-TOOLCHAIN-2.
Viewpoint: the hosted runner, as a job installs its toolchain.
Elements and relations: `.cargo/config.toml`, `.github/actions/hosted-rust/action.yml`, `check-gate-parity` rule 10.

## Context

`.cargo/config.toml` sets `rustc-workspace-wrapper = "clippy-driver"`, so every build of a workspace member runs
clippy's driver. The hosted action passed the caller's `components` to rustup and nothing else. Every nightly job
named `clippy`; the stable Pages job, added by ADR 0066, named none. With no driver, cargo stops before the first
compile. A fresh target on 1.100.0 without clippy reproduces the silent exit 1 on this Mac.

## Decision

1. **The action's `Pin Rust` step opens `components` with the literal `clippy`**, then the caller's list. No input
   can drop it.
2. **`check-gate-parity` rule 10 (`wrapper.rs`) refuses an action a caller can leave without the component** of the
   wrapper the cargo config names. A fallback such as `inputs.components || 'clippy'` is refused, because a caller
   that names `rustfmt` alone drops clippy.

## Alternatives rejected

- **Add `components: clippy` to `pages.yml` only.** The next stable job breaks the same silent way and no gate sees
  it. Reverse this if the config ever stops wrapping builds; then rule 10 owes nothing and says so.
- **Drop the wrapper from `.cargo/config.toml` for stable jobs.** The wrapper is how a plain build enforces the lint
  floor; a stable build without it ships code the floor never read.

## Consequences

- Every hosted toolchain installs clippy. A job that already named it installs it once; rustup accepts the repeat.
- Pages renders and publishes again on main.

## Correspondence

Refines ADR 0066, which added the stable pin.

## Verification

`crates/lgwks-deps/src/bin/check_gate_parity/sim_pins.rs`: `the_hosted_action_installs_the_workspace_wrapper`,
`a_component_fallback_a_caller_can_override_is_refused`,
`a_leading_component_literal_is_accepted_alone_or_before_the_input`,
`a_pin_step_with_no_components_is_refused_and_the_next_step_does_not_count` and
`a_config_without_a_component_wrapper_owes_the_action_nothing`.
