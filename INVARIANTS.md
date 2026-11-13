# Invariants

The enforced rules, one short line each: rule · why · enforced by. The `invariants` lane reads this file and every
`crates/*/INVARIANTS.md` and `INVARIANTS-<n>.md` part as one register, and refuses a part past 120 lines counted at
120 columns (ADR 0040). The crate parts hold the rules of
[`lgwks_std`](crates/lgwks-std/INVARIANTS.md), [`lgwks_deps`](crates/lgwks-deps/INVARIANTS.md),
[`lgwks_ast`](crates/lgwks-ast/INVARIANTS.md) and `lgwks_bot` ([part 1](crates/lgwks-bot/INVARIANTS.md),
[part 2](crates/lgwks-bot/INVARIANTS-2.md), [part 3](crates/lgwks-bot/INVARIANTS-3.md)). Each test file named here
lists its own cases, and the reasoning is in [`docs/invariants-long-form.md`](docs/invariants-long-form.md).

- **INV-DOC-1** A `//!` doc whose `mod` `lib.rs` also documents links through reference definitions with absolute `crate::` paths. · why: run 40206361299, #755 · enforced by: `scripts/doc-lanes.sh`
- **INV-DOC-2** A doc claim is stated at the level its evidence supports and names the observing test, or the issue that owns the gap. · why: #155, #170 · enforced by: the `doc-citations` lane
- **INV-DOC-3** Every ADR holds the ISO/IEC/IEEE 42010 fields and every PRD requirement the 29148 attributes; a record is superseded, never rewritten. · why: ADR 0004 · enforced by: the `doc-governance` lane
- **INV-DOC-4** Every crate README carries the publisher block, and the front page links `llms.txt`. · why: #718, #719 · enforced by: the `contract-drift` lane, `ci_local::readme_block::sim_readme_block`
- **INV-DOC-5** A touched page opens with a `# ` title and a sentence, names each block's language, skips no heading level and links only to files. Added sentences keep to 45 words. · why: ADR 0011 · enforced by: the `doc-governance` lane, `check_docs::prose::tests`
- **INV-DOC-6** Every plain `pub` library item a change adds shows a fenced example, and lgwks_ast and lgwks_macros are held whole. · why: ADR 0012 · enforced by: the `examples` lane, the `examples-held` lane, `sim_scan_examples`
- **INV-DOC-7** Skills, guides and living pages name only paths, lanes, binaries, commands and versions the tree holds, in 120 lines. · why: ADR 0022, ADR 0023 · enforced by: `tests/it/agent_start_page.rs`, `tests/it/sim_agent_start_page.rs`
- **INV-DOC-8** Each register part keeps to 120 lines counted at 120 columns, one short line per rule; a test's cases stay in its file. · why: ADR 0040, Director 2026-11-13 · enforced by: the `invariants` lane, `a_register_past_the_law_is_refused_by_its_wrapped_height`
- **INV-DEFAULT-1** Each crate's default features carry every capability it owns, unless ADR 0003 records the cost or the target that denies one. · why: #732 · enforced by: the `catalog` lane
- **INV-CATALOG-1** Every capability has one `contract/CATALOG.toml` row, `docs/catalog.md` is its fresh render, and every owner the std-first gate names has a row. · why: #732 · enforced by: the `catalog` lane
- **INV-SURFACE-1** A library file declares no more `pub` items than `contract/PUBLIC_SURFACE.toml` allows, none when unlisted, and a stale ceiling is refused. · why: #526, ADR 0021 · enforced by: the `std-first` lane, `check_std_first::surface::tests`
- **INV-EXAMPLE-1** Direct-use Rust examples without `script!` only shrink: a doc file holds no more than its `[examples]` ceiling. · why: #526 · enforced by: the `std-first` lane, `check_std_first::examples::tests`
- **INV-FRONT-1** The rustdoc front page of each `lgwks_*` crate opens with the lexicon rendered from `LEXICON`. · why: #526 · enforced by: `lexicon_docs::every_front_page_opens_with_the_lexicon`
- **INV-GOV-1** The gate is defined once in `scripts/gate-lanes.toml`, and CI runs the same lanes. · enforced by: the `gate-parity` lane
- **INV-GOV-2** A requirement is superseded, never edited, and `REQUIREMENTS.md` with `docs/requirements/` is one spine where an id appears once. · enforced by: the `requirements` lane, `a_requirement_declared_twice_is_refused`
- **INV-SEC-AUDIT-1** Every change audits `Cargo.lock` against RustSec, and only ids SECURITY.md assesses as unreachable, each with a `reopen if`, are silenced. · why: #635 · enforced by: the `advisories` lane
- **INV-CI-DOCS-ONLY-1** A pull request of only docs pages runs the docs gate and skips the suites; any other file runs everything. · why: ADR 0011, #861 · enforced by: the `changes` lane, the `docs-only-verdict` lane, the `gate-parity` lane
- **INV-CI-SLOT-1** A macOS test holds one of `LWC_CHURN_SLOTS` host-wide locks through `churn-slot`, and a default `TMPDIR` moves to the shared RAM volume. · why: #272, #901, ADR 0026 · enforced by: `tests/it/sim_churn_slot.rs`
- **INV-CI-RECEIPT-1** The receipts reader skips a JUnit case whose `classname` starts `@setup-script:`; a real test reported twice is refused. · why: #272 · enforced by: `acceptance_receipts::junit::tests`
- **INV-CI-RUNNER-CLASS-1** Every job of `ci.yml` and `pages.yml` runs on a GitHub-hosted runner, never `self-hosted`, because the repository is public. · why: province#9 · enforced by: the `gate-parity` lane
- **INV-CI-NEUTER-1** No gate step of `ci.yml` ends green after its command failed: a swallowed exit, `if: false` and `continue-on-error` are refused. · why: #911 · enforced by: the `gate-parity` lane, `check_gate_parity::neuter::tests`
- **INV-CI-CACHE-1** Main saves a job's build only past 150 s cold, and `cache-budget.yml` deletes what main's last runs left untouched. · why: ADR 0041 · enforced by: `the_cache_holds_only_the_builds_that_set_the_wall`
- **INV-SCAN-GATED-1** A gate exempts only what does not ship: `cfg(test)` spans with comments blanked, and vendor and build trees only where they belong. · why: #911 · enforced by: `ci_local::test_gated::tests`, `a_vendor_or_third_party_directory_inside_a_crate_is_scanned`
