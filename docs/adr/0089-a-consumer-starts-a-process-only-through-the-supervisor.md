# ADR 0089: a consumer starts a process only through the Supervisor

Status: **accepted**, 2026-11-27.

Stakeholders: the Director; every repository that runs `lgwks-deps gate`.
Concerns: a consumer's code, its tests above all, starts a child that outlives a SIGKILLed parent.
Requirements: INV-SCAN-RAW-PROCESS-1.
Viewpoint: the source-rule view of `lgwks_ast::scan` and the consumer gate.
Elements and relations: the opt-in rule `raw-process` in `lgwks_ast::scan::raw_process`; the `raw-process` leg of
`lgwks-deps gate`, which runs `lgwks-ast scan --rule raw-process` over the consumer root.

## Context

On 2026-11-26 the tests of logical_ci and larry started `logical_ci serve` and `larry acp` with a raw `Command` (#1160).
A SIGKILL ended each test, and the children held a 24 GB Mac for hours. The crates' own spawn bans read only this
repository, and no rule read test code. A `ProcessSpec` handed to the `lgwks_bot` `Supervisor` owns the group, its
deadline and its temp dir, and holds the group on the lifeline (ADR 0087).

## Decision

1. `raw-process` reports every `Command::new` on a path that ends `process::Command`, or on the bare name or alias a `use`
   of such a path binds. Test functions, `#[cfg(test)]` modules and macro arguments are read like any code. A path that
   starts `crate`, `self` or `super` is the file's own type.
2. Each finding names the `Supervisor`. A start of `hdiutil`, `mount`, `diskutil` or `umount` also names the lifeline's
   `lgwks_std::process::hold_device`.
3. The rule is opt-in. The crates that own process starts (`lgwks_std::process`, the `Supervisor`) pass the zero gate.
4. `lgwks-deps gate` runs it over the whole consumer root as its `raw-process` leg. Exit 1 fails the gate. A scan that
   cannot read the tree errs, and is never a pass.
5. The gate's `--json` carries each failed leg's transcript, so an agent sees the file and the line.

## Alternatives rejected

- **The rule inside `lgwks-deps check`.** It needs the `lgwks_ast` `scan` feature in `lgwks_deps`: its normal graph goes
  from 43 to 68 packages (`cargo tree --locked -p lgwks_deps -e normal -F lgwks_ast/scan`, 2026-11-27), tree-sitter's C
  included. Every consumer already installs `lgwks-ast` for the gate.
- **Flagging `.spawn()`.** A builder bound to a variable hides its receiver, and `.output()` and `.status()` start a
  process too. `Command::new` is where every start begins.
- **A text match in `check-std-first`.** It reads only this repository, and text cannot tell an imported `Command` from
  a local type.

## Consequences

- A consumer with a raw start fails the gate until it moves to the `Supervisor`. On 2026-11-27 the rule found 22 in
  logical_ci and 69 in larry, the leaking `tests/it/support.rs` and `tests/support/serve_cli.rs` among them. The scans
  took 0.14 s and 0.37 s, under 7 MiB peak RSS (`/usr/bin/time -l`). logical_ci and larry move with the release.
- A start through a function pointer or a re-export under another module name is not seen.

## Correspondence

Relates to ADR 0087 (the lifeline) and ADR 0012 (opt-in rules). Part of #1160 G3.

## Verification

`sim_raw_process_finds_exactly_the_raw_starts` sweeps 2,048 seeded files against a model. The test
`sim_raw_process_reads_code_not_text` holds comments and strings out. The test
`sim_raw_process_sees_through_hidden_imports` reads nested groups, globs, raw names and macro groups. The test
`sim_a_raw_process_in_test_code_fails_the_gate` runs the shipped gate over a consumer whose test starts a raw process.
