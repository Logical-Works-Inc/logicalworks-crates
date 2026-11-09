---
name: lgwks-red-gate
description: Use when a gate lane or a CI row is red, or a commit or pull request is refused. Maps each lane to the local command that reproduces it and to the repair its refusal asks for.
---

# A red gate

A refusal is an instruction: read it, repair what it names, and rerun that lane alone before the whole gate.

```sh
./scripts/ci-local.sh --list          # every lane id
./scripts/ci-local.sh --lane catalog  # reproduce one lane as CI runs it
```
A CI row's name is a step or job in `.github/workflows/ci.yml`, and `scripts/gate-lanes.toml` maps each step to its lane through `ci_step`.
To read one failed job, run `gh run view <run-id> --log-failed`.

## Lane to repair

| Lane | What it refuses | Repair |
|---|---|---|
| `catalog` | A row whose file or feature is gone, an owner with no row, or a stale render | Fix `contract/CATALOG.toml`, then run the `--write` form of the `catalog` binary |
| `doc-governance` | An ADR field missing, a page's shape, a long sentence or a dead link | Fix the page. It judges pages changed since `origin/main` |
| `llms-txt` | A page with no row, or a stale `llms.txt` | Add the row to `docs/README.md`, then run `cargo run --locked -p lgwks_deps --bin llms-txt` |
| `doc-citations` | A cited `path:line` whose text moved | Fix the guide, or run `cargo run --locked -p lgwks_deps --bin check-doc-citations -- --update` and read what it prints |
| `std-first` | A `pub` over its ceiling, a stale ceiling, a Rust doc block over its cap, a hand-rolled capability or an undeclared crate | Change `contract/PUBLIC_SURFACE.toml` or use the owner the refusal names |
| `deps-register` | An edge with no approval, an approval with no edge, or a missing exit | `skills/lgwks-dependency-admission/SKILL.md` |
| `lockfile` | A lock that is out of date | Update it, then move every nested lock too: `skills/lgwks-release/SKILL.md` |
| `scan` | A swallowed error, an unlogged `return Err`, a silenced lint, a long `?` chain or an empty docstring | Fix the code. Silencing is itself a finding |
| `examples`, `examples-held` | A new `pub` item with no fenced example | Add one in the item's doc |
| `invariants` | An `INVARIANTS.md` line that breaks the register or names a test that is gone | Fix the line or the test |
| `contract-drift` | A dependency missing from the `lgwks_std` README, or a crate README without its publisher block | Restore the bullet or the block (INV-DOC-4) |
| `requirements` | A changed sentence of `REQUIREMENTS.md` | Supersede it in the log. Never edit it |
| `simulation-evidence` | Simulations below half of the tests | Add real `sim_` tests, in a change that adds tests |
| `tests`, `tests-bot-full`, `tests-std-full`, `tests-linux` | A failing test | Reproduce with `cargo nextest run --locked -p <crate> -E 'test(<name>)'` |
| `gate-parity` | `ci.yml` and the lane table disagree, or a self-hosted runner is named | Fix the side that diverged (INV-CI-RUNNER-CLASS-1) |
| `readme-quickstart` | The root `README.md` snippet differs from `crates/lgwks-bot/examples/quickstart.rs` | Make them match |
| `target-matrix` | A feature that does not compile for a declared target | Gate it by target, or add a default exemption with its reason in ADR 0003 |
| `advisories` | A new RustSec advisory | Upgrade, or assess its id in `SECURITY.md` and `.cargo/audit.toml` (INV-SEC-AUDIT-1) |
| `format`, `lint`, `compile` | `cargo fmt`, clippy or a build | Fix the code. A silencing attribute needs a `reason` |

## When the cause is not the code

- **Flaky is a defect.** Find the race and fix it, and do not rerun until it passes. A test that passed twice proves nothing.
- **A green pull request can fail on a cold `main`.** A shard's cache hides a compile cost. Check the job's wall time against its cap, and size it for a cold build.
- **`failed to fill whole buffer` from cargo** points at a dead compiler cache server, not at the code.
- **Refused by a hook on a commit.** Read which rule it names and fix that. Never skip a hook.

## After the repair

Rerun the failing lane, read its output end to end, then push once. A pull request's whole check set reruns on each push, so batch every fix first.
