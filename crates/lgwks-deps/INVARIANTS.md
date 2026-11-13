# `lgwks_deps` invariants

The `lgwks_deps` part of the invariant register, one short line per rule: rule · why · enforced by. The rest is in
[`INVARIANTS.md`](../../INVARIANTS.md).

- **INV-DEP-1** Six members, from the register's `[policy] surfaces`; any other member or approval owner is `UnknownSurface`. · why: #207 · enforced by: `a_sixth_workspace_member_is_refused_by_name`
- **INV-DEP-2** Every authored external edge is a `lgwks_deps` feature registered in `contract/APPROVED.toml` with its owner. · why: #732 · enforced by: the `deps-register` lane
- **INV-DEP-3** No new direct edge to a name the AGENTS.md list bans; the gate refuses an unregistered edge, not a name. · why: #787 · enforced by: the `deps-register` lane
- **INV-DEP-4** `lgwks_std` never routes through `lgwks_deps`, a cycle Cargo refuses when it resolves. · enforced by: the `lockfile` lane
- **INV-DEP-5** A registry edge is a caret requirement and `Cargo.lock` holds the release; an exact pin needs a live `upstream-pin` row. · why: ADR 0031, #922 · enforced by: `ledger::tests::the_committed_register_pins_no_registry_edge_exactly`
- **INV-DEP-6** `lgwks_std::random` is a default feature, so feature-matrix code may rely on it. · why: #732 · enforced by: the `target-matrix` lane
- **INV-DEP-7** A path dependency is internal only when its resolved manifest directory is a member's. · why: #143 · enforced by: `lgwks_deps::metadata::edges_tests`
- **INV-DEP-8** Cargo metadata collection is bounded in time and bytes, and an incomplete collection is a refusal. · why: #159, #193 · enforced by: `lgwks_deps::metadata::capture_tests`
- **INV-DEP-9** Each selected Bevy feature exposes its promised import path, and a deselected one is absent. · why: #169 · enforced by: `crates/lgwks-deps/tests/storefront_consumers.rs`
- **INV-DEP-10** Every member resolves to one package record and manifest directory before classification. · why: #158 · enforced by: `lgwks_deps::metadata::edges_tests`
- **INV-DEP-11** Both registers share one fail-closed TOML subset that refuses a repeated key at its position. · why: #157 · enforced by: `contract::tests::invariant_register_uses_the_shared_duplicate_key_refusal`
- **INV-DEP-12** Admission compares the exact Cargo origin, so a substitution inside an approved class is `OriginDrift`. · why: #158 · enforced by: `tests/it/sim_origin.rs`
- **INV-DEP-13** An approval's authored features, optionality and target are enforced exactly, as typed drift refusals. · why: #158 · enforced by: `tests/it/sim_dependency_policy.rs`
- **INV-DEP-14** Package identity is byte-exact: only the name or an explicit, collision-checked alias is admitted. · why: #158 · enforced by: `tests/it/identity_binding.rs`
- **INV-DEP-15** A `check` receipt binds subject, register digest, metadata digest, mode and scope under stable `--json` keys. · why: #158 · enforced by: `tests/it/check_cli.rs`
- **INV-DEP-16** The gate holds no repository's policy: licences, surfaces and freezes are each register's `[policy]`. · why: Director 2026-10-05 · enforced by: `tests/it/sim_license_policy.rs`
- **INV-DEP-17** Registry bytes come from `Cargo.lock` under `--locked`, and every `[patch]`, `[replace]` or source override is refused. · why: #266 · enforced by: `lgwks_deps::overrides::tests`, the `lockfile` lane
- **INV-DEP-18** A bare `cargo run -p lgwks_deps -- <command>` runs the `lgwks-deps` binary through `default-run`. · why: #537 · enforced by: `tests/it/default_run.rs`
- **INV-DEP-19** Every approved edge has a `contract/LIFECYCLE.toml` row naming its replacement, removal trigger and review date. · why: #722 · enforced by: `lifecycle::tests::an_unmortgaged_edge_is_refused_by_name`
- **INV-DEP-20** No product crate imports a third-party crate past the storefront. · why: #722 · enforced by: `ledger::tests::the_committed_register_names_no_product_owner`
- **INV-DEP-21** Every approval carries an `[approved.retire]` table: replacement, blocker, measured cost and review date. · why: #788 · enforced by: `tests/it/sim_retire.rs`
- **INV-FRESH-1** `freshness` holds at most `--jobs` lookups, ranks by SemVer, never offers a yanked version, and exits 0, 1 or 2. · why: #702, #754 · enforced by: `tests/it/sim_freshness_cli.rs`, `freshness_cmd::fetch::sim_fetch`
- **INV-DEFERRAL-1** A crate the deferral catalog defers is refused while approved, naming the owner to fix first. · why: #725 · enforced by: `deferral::tests::an_approved_edge_the_catalog_defers_is_refused`
- **INV-PATCH-1** An upstream pin expires and never becomes a fork; an expired pin is refused with its upstream. · why: #723 · enforced by: `patches::tests::an_expired_pin_is_refused_with_its_upstream`
- **INV-TLS-1** Outbound TLS has one provider, rustls on ring behind `tls-ring`, and no C-toolchain provider resolves. · why: #728, #869 · enforced by: `patches::tests::an_uncovered_provider_is_refused_with_its_chain`
- **INV-SEC-AUDIT-2** `advisories::audit` reads a requirement exactly as Cargo's `VersionReq` does, pre-release gate included. · why: #942 · enforced by: `sim_advisories_semver_agrees_with_version_req`, `sim_advisories_reports_every_row_cargo_audit_reports`
- **INV-SEC-AUDIT-3** A match carries the advisory's first `# ` heading as its title, and every skipped pin is named. · why: #955, ADR 0043 · enforced by: `sim_advisories_title_is_the_first_heading`, `sim_advisories_reports_every_row_cargo_audit_reports`
- **INV-LGWKS-NEW-1** `lgwks new` writes exactly `Cargo.toml` and `app.rs` and refuses a non-empty directory. · why: #787 · enforced by: `tests/it/lgwks_e2e.rs`
- **INV-LGWKS-CHECK-1** `lgwks check` runs the one parser and the plan bounds without building, with located refusals. · why: #787 · enforced by: `tests/it/sim_lgwks_check.rs`
- **INV-LGWKS-EXPAND-1** `lgwks expand` prints byte for byte what the macro compiles, and the expansion passes clippy. · why: #787 · enforced by: `tests/it/sim_lgwks_expand.rs`
- **INV-LGWKS-EMIT-1** One emitter, and `lgwks_macros` is a thin shell over parse plus that function. · why: #787 · enforced by: `lgwks_macros::tests::expand`
- **INV-LGWKS-RUN-1** `lgwks run` builds twice in one target directory and prints the cold build, the warm rebuild and the start. · why: #787 · enforced by: `tests/it/lgwks_e2e.rs`
- **INV-LGWKS-JSON-1** Every command takes `--json` with the same facts as the human text. · why: #787 · enforced by: `tests/it/lgwks_json.rs`
