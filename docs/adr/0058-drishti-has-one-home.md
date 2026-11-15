# ADR 0058: drishti has one home, `crates/drishti`

Status: **accepted**, 2026-11-14. Folds the private Canvas code into the public crate, on the Director's order "Do it".

Stakeholders: the Director; the Logic OS Canvas maintainers; consumers of the `drishti` crate; `lgwks_deps` maintainers.
Concerns: two repositories for one crate; private code that no public gate reads; third-party crates declared outside the storefront; a second SQLite migration copy.
Requirements: R11
Viewpoint: the module viewpoint, following each moved module to the storefront edge it reaches.
Elements and relations: `crates/drishti`, `drishti::canvas_protocol`, `drishti::canvas_scene`, `drishti::motion`, `drishti::native_host`, `drishti::preview`, `drishti::cli`, `lgwks_deps` features `p256`, `libm` and `desktop-preview`, `lgwks_deps::sqlite::migrate`.

## Context

`crates/drishti` held the design publication, the ink lab and the GPU residency model. The Logic OS Canvas kernel row 15 (kernel#850) lived in a private repository: `canvas_protocol`, `canvas_scene`, `motion`, `native_host`, `preview`, five diagnostic binaries, the `assets/` conformance fixtures and about 280 tests. That repository declared `p256`, `libm`, `rusqlite`, `winit`, `softbuffer`, `cosmic-text` and `proptest` itself, and migrated its own SQLite file with a local `user_version` routine.

## Decision

1. **The crate is the one home.** The private code moves in unchanged in behaviour. The base crate wins every document where the two disagree. The private repository is archived.
2. **Third-party edges go through the storefront.** `lgwks_deps` gains `p256` (#840), `libm` (#837) and `desktop-preview` (#836: `winit`, `softbuffer`, `cosmic-text`), each optional, registered with `owner = "lgwks_deps"`. SQLite (#839) reuses the `sqlite` feature. Drishti declares no third-party crate except the test-only `proptest` (#838).
3. **The governance store migrates through the estate.** `GovernanceStore::install_schema` calls `lgwks_deps::sqlite::migrate`. A first-release file still upgrades in place, and a file from a newer build is still refused.
4. **Decisions renumber.** The private decisions D15, D16 and D17 become D17, D18 and D19 in `docs/decisions.md`.
5. **One simulation target.** The base sims and the private sims share `tests/sim.rs`. The ink-lab generator keeps its name, `SplitMix64`, beside `SimRng`, so no recorded trace changes.

## Alternatives rejected

- **Keep the private repository as the source.** It leaves the code outside every public gate and the dependency register.
- **Publish a second crate.** It splits one owner of the canvas concern in two.
- **Keep the local migration routine.** It is a second copy of what `lgwks_deps::sqlite::migrate` owns.

## Consequences

- The public tree now carries code written in a private repository. The scrub found no host name, home path, secret or token in the tree or its history.
- `kernel#850` references stay as provenance. The private issues are written `drishti#8`, `drishti#9` and `drishti#10` and name the former private repository.
- The `desktop` feature compiles 65 more crates than the base. The storefront costs are in `contract/APPROVED.toml`.
- The governance file keeps its plain open. It does not yet use `lgwks_deps::sqlite::open` (WAL, busy timeout); that change is separate.

## Correspondence

Relates to ADR 0003, which keeps every storefront group default-off, and to ADR 0014, which splits capability gaps between elimination and storefront boundaries.

## Verification

- `crates/drishti/tests/bundle.rs`: `decision_ids_are_unique_and_the_adr_cites_live_ones` holds the decision ids and this record's citations.
- `crates/drishti/tests/sim/native_governance_scope.rs`: `sim_governance_legacy_database_migrates_in_place` holds the in-place upgrade through `migrate`.
- The `deps-register` lane holds the five new approvals and their lifecycle rows.
