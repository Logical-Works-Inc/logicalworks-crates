//! Integration tests for `lgwks_deps`, linked as one binary (#272).
//!
//! One link and one test-binary start per crate instead of one per file.
//! Each former `tests/<name>.rs` is the module `<name>` here, so a filter on
//! `test(/^<name>::/)` selects what `--test <name>` used to, and the
//! `sim_*` module paths are what the simulation-evidence lane counts.
//! Tests that must own their process stay separate binaries beside this
//! directory; the crate's Cargo.toml or the PR names each and why.

// Fixtures more than one module uses, declared here once: a file loaded as
// a module twice is two copies of every type in it, and clippy refuses it.
// The modules that use them `use crate::<fixture>`.
#[path = "../support/deps_sim.rs"]
mod deps_sim;

#[path = "../support/sim.rs"]
mod sim;

#[path = "../support/replay.rs"]
mod replay;

/// A `Cargo.lock` writer and the scratch directory to put it in.
#[path = "../support/lock_fixture.rs"]
mod lock_fixture;

/// A fixture's `git`, with none of a hook's variables that redirect it.
#[path = "../support/git_env.rs"]
mod git_env;

/// The one list of those variables, shared with the crate and `churn-slot`.
#[path = "../../src/redirecting_env.rs"]
mod redirecting_env;

/// A sparse-index registry on a loopback port, for driving `freshness`.
#[path = "../support/stub_index.rs"]
mod stub_index;

/// The estate's one seed substrate: the generator and the replay trace every
/// workspace simulation draws from, so a seed means one stream in every crate.
#[path = "../../../lgwks-bot/tests/sim/seed.rs"]
mod seed;
/// The weighted coin and the trace's emptiness query, over the same core.
#[path = "../../../lgwks-bot/tests/sim/seed_helpers.rs"]
mod seed_helpers;

/// The pages an agent starts from, held to the tree.
mod agent_start_page;
/// `CHANGELOG.md`'s release rows, held to the manifests.
mod changelog_rows;
/// A page for people, held to one statement per sentence with nothing sold.
mod doc_voice;
/// Every invariant id the tree cites, held to the registers that define it.
mod invariant_citations;
// The runner's journeys need `kill`, `ps` and the process groups its watchdog
// stops, which every Unix host has and Windows does not.
#[cfg(unix)]
mod ai_authoring;
/// Every cfg arm held to the lint floor, including those this host never builds.
mod cfg_arm_silencers;
mod check_cli;
/// The `Classify the change` step of ci.yml, run under bash with a fake `gh`.
#[cfg(unix)]
mod ci_changes;
/// Every compiling CI job runs rustc through kache and its one shared store.
mod ci_compile_cache;
/// The shipped `ci-receipt` binary, driven on real failed-job logs.
mod ci_receipt_cli;
/// The pull-request and merge-queue speeds of ci.yml, and the verdict between them.
#[cfg(unix)]
mod ci_two_speed;
mod contract_schema_compat;
mod default_run;
/// A citation a page adds, refused by name until `--update` pins it.
mod doc_citations_cli;
mod embedding;
mod estate_edges;
mod feature_policy;
// The `lgwks` binary is built only with its feature (#787), so its journeys
// run where it exists.
#[cfg(feature = "lgwks")]
#[path = "../support/lgwks_scratch.rs"]
mod lgwks_scratch;

mod gate_consumer;
mod gate_ownership;
/// The doc tests held off the one critical path, by the shipped parity binary.
mod gate_parity_doc_path;
/// The `logical_ci` job's runner, held to where the engine contains lanes.
mod gate_runner;
mod identity_binding;
#[cfg(feature = "lgwks")]
mod lgwks_e2e;
#[cfg(feature = "lgwks")]
mod lgwks_json;
#[cfg(feature = "lgwks")]
mod lgwks_tls;
mod metadata_dimensions;
mod metadata_public_api;
mod origin_binding;
mod prop_parsers;
mod sdk_surface;
/// The ADR index held to the record directory under seeded faults.
mod sim_adr_index;
// The wrapper's journeys run `/bin/sh` and a program that reads a pipe.
mod sim_advisories_audit;
mod sim_advisories_cli;
mod sim_agent_start_page;
/// The runtime census held to its own totals and to the source tree.
mod sim_census_runtime;
/// `check`'s report layer held fail-closed in both output modes.
mod sim_check_report;
#[cfg(unix)]
mod sim_churn_slot;
mod sim_dependency_policy;
mod sim_freshness_cli;
mod sim_invariant_register;
mod sim_job_limits;
#[cfg(feature = "lgwks")]
mod sim_lgwks_check;
#[cfg(feature = "lgwks")]
mod sim_lgwks_expand;
#[cfg(feature = "lgwks")]
mod sim_lgwks_id;
#[cfg(feature = "lgwks")]
mod sim_lgwks_legacy;
mod sim_license_policy;
mod sim_lock_dependencies;
mod sim_lock_spellings;
mod sim_lockfile;
mod sim_metadata_dimensions;
mod sim_origin;
/// The crates ADR 0052 plans, held to CODEBOOK.md and the workspace members.
mod sim_planned_crates;
mod sim_policy_properties;
mod sim_register_policy;
mod sim_requirements_pages;
#[cfg(feature = "lgwks")]
mod sim_retire;
mod sim_seed_draws;
/// The `skill-hygiene` lane, driven through `ci-local` on seeded skill trees.
#[cfg(unix)]
mod sim_skill_hygiene;
#[cfg(feature = "sqlite")]
mod sim_sqlite_first_open;
mod sim_vendor_coverage;
#[cfg(feature = "sqlite")]
mod sqlite_defaults;
/// No workflow step idles: a `sleep` in CI spends the run's 300 s wall on nothing.
mod workflow_idle_sleeps;
mod workspace_lock;
mod workstream_c;
