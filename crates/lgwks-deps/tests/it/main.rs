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

// The runner's journeys need `kill`, `ps` and the process groups its watchdog
// stops, which every Unix host has and Windows does not.
#[cfg(unix)]
mod ai_authoring;
mod check_cli;
mod contract_schema_compat;
mod default_run;
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
// The wrapper's journeys run `/bin/sh` and a program that reads a pipe.
#[cfg(unix)]
mod sim_churn_slot;
mod sim_dependency_policy;
mod sim_freshness_cli;
mod sim_invariant_register;
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
mod sim_policy_properties;
mod sim_register_policy;
#[cfg(feature = "lgwks")]
mod sim_retire;
mod sim_seed_draws;
mod sim_vendor_coverage;
#[cfg(feature = "sqlite")]
mod sqlite_defaults;
mod workstream_c;
