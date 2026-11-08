//! Integration tests for `lgwks_bot`, linked as one binary (#272).
//!
//! One link and one test-binary start per crate instead of one per file.
//! Each former `tests/<name>.rs` is the module `<name>` here, so a filter on
//! `test(/^<name>::/)` selects what `--test <name>` used to, and the
//! `sim_*` module paths are what the simulation-evidence lane counts.
//! Tests that must own their process stay separate binaries beside this
//! directory; the crate's Cargo.toml or the PR names each and why.
//!
//! Each test module belongs to one group: `scale`, `surface`, `refusals` or
//! `g1` to `g8`. A build with no `lwc_it_group` cfg compiles every module, and
//! that build is what clippy, the local gate and a contributor run. A build
//! with `--cfg lwc_it_group --cfg lwc_it_group="<group>"` (the bare name too,
//! which `not(lwc_it_group)` reads) compiles that group's modules and every
//! fixture, which is what one CI job builds. Compiling this binary whole took
//! about 110 s of a 4-vCPU job's 300 s (68bef2d12), so each job builds one
//! group, and the groups are balanced by each module's measured test time.
//! Module paths do not change, so `test(/^<name>::/)` and the `sim_*` count
//! hold in either build.

// Fixtures more than one module uses, declared here once: a file loaded as
// a module twice is two copies of every type in it, and clippy refuses it.
// Each is compiled when any module that uses it is, and those modules
// `use crate::<fixture>` under the name they already gave it.
#[path = "../sim/bands.rs"]
mod band_family;

#[cfg(any(
    all(
        unix,
        feature = "rt",
        feature = "time",
        feature = "sync",
        feature = "process"
    ),
    feature = "script"
))]
#[path = "../support/compile.rs"]
mod compile;

#[cfg(all(feature = "ecs", feature = "wire"))]
#[path = "../support/declarable.rs"]
mod declarable;

#[path = "../support/effects.rs"]
#[cfg(all(feature = "ecs", feature = "wire"))]
mod effects;

#[cfg(all(
    unix,
    feature = "script",
    feature = "process",
    feature = "time",
    feature = "sync",
    feature = "ephemeral"
))]
#[path = "../support/fake_gh.rs"]
mod fake_gh;

#[cfg(all(unix, feature = "script", feature = "process", feature = "ephemeral"))]
#[path = "../support/fake_gh_issues.rs"]
mod fake_gh_issues;

#[cfg(all(feature = "inspect", feature = "script"))]
#[path = "../inspect_support/mod.rs"]
mod inspect_support;

#[path = "../support/journal.rs"]
#[cfg(feature = "wire")]
mod journal_fixtures;

#[cfg(feature = "script")]
#[path = "../support/load.rs"]
mod load;

#[path = "../support/lock.rs"]
mod lock;

#[cfg(all(feature = "ecs", feature = "wire"))]
#[path = "../support/poll.rs"]
mod poll;

#[cfg(all(
    unix,
    feature = "rt",
    feature = "time",
    feature = "sync",
    feature = "process"
))]
#[path = "../support/process.rs"]
mod process_probe;

#[cfg(any(feature = "script", all(feature = "ecs", feature = "wire")))]
#[path = "../../../lgwks-std/tests/support/prop.rs"]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod prop;

#[cfg(feature = "script")]
#[path = "../support/effect_policy.rs"]
mod effect_policy;

#[cfg(all(feature = "script", feature = "ephemeral", feature = "time"))]
#[path = "../support/approval_error.rs"]
mod approval_error;

#[cfg(feature = "script")]
#[path = "../support/proposal.rs"]
mod proposal_fixtures;

#[cfg(all(feature = "script", feature = "ephemeral"))]
#[path = "../support/repair.rs"]
mod repair_fixtures;

#[cfg(all(feature = "script", feature = "ephemeral"))]
#[path = "../support/request.rs"]
mod request_fixtures;

#[cfg(all(feature = "script", feature = "ephemeral"))]
#[path = "../support/resume.rs"]
mod resume_fixtures;

#[cfg(feature = "rt")]
#[cfg(feature = "wire")]
#[path = "../support/liveness.rs"]
mod liveness_fixtures;

#[cfg(feature = "script")]
#[path = "../support/run_world.rs"]
mod run_world;

#[path = "../support/scratch.rs"]
mod scratch;

#[cfg(all(feature = "edge", feature = "script"))]
#[path = "../support/apps_harness.rs"]
mod apps_harness;

#[cfg(feature = "sealed")]
#[path = "../support/sealed_harness.rs"]
mod sealed_harness;

#[cfg(feature = "edge")]
#[path = "../support/serve_edge.rs"]
mod serve_edge;

#[cfg(feature = "testing")]
#[path = "../../src/bin/store_conformance/memory_records.rs"]
mod memory_records;

#[cfg(all(feature = "rt", feature = "time", feature = "sync", feature = "script"))]
#[path = "../support/tenancy_harness.rs"]
mod tenancy_harness;

#[cfg(all(
    feature = "script",
    feature = "ephemeral",
    feature = "time",
    feature = "net"
))]
#[path = "../support/outside.rs"]
mod outside_support;

#[path = "../sim/mod.rs"]
mod sim;

#[path = "../support/sweep.rs"]
mod sweep_fixtures;

/// This test binary re-invoked to run the one test named `test` as a probe child.
///
/// One builder for every kill and crash probe, so the executable and the
/// argument shape cannot drift between them. `test` is the libtest name: the
/// test's path below the crate root, which is what a test's own thread is named
/// and what [`probe_test`] builds from a `module_path!()`.
#[cfg(feature = "wire")]
fn probe_command(test: &str) -> Result<std::process::Command, Box<dyn std::error::Error>> {
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.args([test, "--exact", "--nocapture"]);
    Ok(command)
}

/// The libtest name of `test` declared in `module` (its `module_path!()`).
///
/// libtest names a test by its path below the crate root; the crate's own
/// segment is what `module_path!()` adds and an `--exact` filter must not carry.
#[cfg(feature = "wire")]
fn probe_test(module: &str, test: &str) -> String {
    match module.split_once("::") {
        Some((_crate, path)) => format!("{path}::{test}"),
        None => test.to_owned(),
    }
}

#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod ambiguous_commit;
#[cfg(all(feature = "edge", feature = "ephemeral"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod app;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod approval;
#[cfg(all(feature = "edge", feature = "script"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod apps;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod authority;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod bot_front_door;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod cap_tokens;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod cas;
#[cfg(all(feature = "edge", feature = "ephemeral"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod cell_edge;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod ceremony_reference;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod clock_restart;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod core_boundary;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod credential;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod dist_lease;
#[cfg(feature = "net")]
#[cfg(feature = "wire")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod dist_lease_tcp;
#[cfg(feature = "net")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod domain_values;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod durable_crash_group_commit;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod durable_crash_observation;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod durable_dispatch;
#[cfg(all(feature = "edge", feature = "ephemeral"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod edge;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod effect_identity;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod effect_journal;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod ephemeral;
#[cfg(feature = "script")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod estate;
#[cfg(all(feature = "ecs", feature = "wire", feature = "ron"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod flow_ron;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod gh_binding;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g2"))]
mod gh_issue;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod grant_replace;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod inspect;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod inspect_contract;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod inspect_non_execution;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod inspect_scale;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod inspect_wiring;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod journal_continuation;
#[cfg(feature = "wire")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod journal_liveness;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod journal_scale;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod journal_writer_fence;
#[cfg(all(feature = "script", feature = "legacy"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod legacy_script;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod locator_eligibility;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod net_split;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod no_default;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod observe_refresh;
#[cfg(feature = "wire")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod orphan_reap;
#[cfg(all(
    feature = "script",
    feature = "ephemeral",
    feature = "time",
    feature = "net"
))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod outside;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod owner_epoch_takeover;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod pr_review_journey;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod process_escape;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod process_fire;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod process_ownership;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod prop_each;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod prop_journal;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod proposal;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod readme_front_door;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod ready;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod registry;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod repair;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod request_key;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod resume_liveness;
#[cfg(feature = "net")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod retry_proxy;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod rt_async_tier;
#[cfg(all(feature = "rt", feature = "time", feature = "sync"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod rt_front_door;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod rt_joinset;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod rt_process;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod rt_process_files;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod rt_process_stdin;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod rt_runtime_stack;
#[cfg(feature = "script")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod run_events;
#[cfg(feature = "testing")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod run_records;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod script_domains;
#[cfg(any(not(lwc_it_group), lwc_it_group = "refusals"))]
mod script_flow;
#[cfg(feature = "script")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod script_front_door;
#[cfg(any(not(lwc_it_group), lwc_it_group = "refusals"))]
mod script_map_parity;
#[cfg(any(not(lwc_it_group), lwc_it_group = "refusals"))]
mod script_refusals;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod script_trail;
#[cfg(feature = "sealed")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sealed;
#[cfg(feature = "sealed")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sealed_disk_torn;
#[cfg(feature = "edge")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod serve;
#[cfg(all(feature = "edge", feature = "script"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod serve_openapi;
#[cfg(feature = "edge")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod serve_process;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod session;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod session_front_door;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_append_verdict;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_approval;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_approval_extra;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_approval_scale;
#[cfg(all(feature = "edge", feature = "script"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_apps;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_cancel_under_load;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_cap_tokens;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_cas;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_cas_corrupt;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_cas_disk;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_cas_execute;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod sim_change_ticks;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod sim_clock;
#[cfg(feature = "wire")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_clock_kill;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_clock_wiring;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sim_confinement;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod sim_continuation;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod sim_continuation_seal;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_credential;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_dispatch;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_dist_lease;
#[cfg(all(feature = "net", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_dist_lease_tcp;
#[cfg(feature = "ecs")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod sim_ecs_backoff;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_epoch_identity;
#[cfg(feature = "script")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod sim_estate;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sim_fan_out;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g2"))]
mod sim_gh_issue;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sim_group_commit;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_inspect;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod sim_inspect_wiring;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_journal;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_journal_liveness;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_journal_tail;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g2"))]
mod sim_keyed;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_landlock;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sim_language;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_network;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_observe_refresh;
#[cfg(all(
    feature = "script",
    feature = "ephemeral",
    feature = "time",
    feature = "net"
))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_outside;
#[cfg(all(
    feature = "script",
    feature = "ephemeral",
    feature = "time",
    feature = "net"
))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sim_outside_bounds;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_permit_release;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_process;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_process_env;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_process_orphans;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sim_process_output;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_progress_overflow;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod sim_proposal;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod sim_readme_front_door;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod sim_ready;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sim_repair;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod sim_repeat_ceiling;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod sim_replay_drift;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod sim_request_key;
#[cfg(feature = "net")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_retry_proxy;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_review_pr;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_review_wire;
#[cfg(all(feature = "rt", feature = "time", feature = "sync"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sim_rt_front_door;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod sim_run_boundaries;
#[cfg(feature = "script")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_run_events;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "scale"))]
mod sim_scale;
#[cfg(any(not(lwc_it_group), lwc_it_group = "refusals"))]
mod sim_script;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sim_script_domains;
#[cfg(any(not(lwc_it_group), lwc_it_group = "refusals"))]
mod sim_script_durable;
#[cfg(any(not(lwc_it_group), lwc_it_group = "refusals"))]
mod sim_script_fanout;
#[cfg(any(not(lwc_it_group), lwc_it_group = "refusals"))]
mod sim_script_feeds;
#[cfg(any(not(lwc_it_group), lwc_it_group = "refusals"))]
mod sim_script_steps;
#[cfg(any(not(lwc_it_group), lwc_it_group = "refusals"))]
mod sim_script_vocab;
#[cfg(any(not(lwc_it_group), lwc_it_group = "refusals"))]
mod sim_script_words;
#[cfg(feature = "sealed")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_sealed;
#[cfg(feature = "sealed")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_sealed_adversary;
#[cfg(feature = "sealed")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_sealed_disk;
#[cfg(feature = "sealed")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_sealed_shred;
#[cfg(feature = "edge")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_serve;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod sim_session_front_door;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod sim_skew;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sim_source_matrix;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sim_spec_materialize;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod sim_stability;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sim_step_key_runs;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_store_faults;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod sim_store_scale;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_substrate;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod sim_supervise_reports;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod sim_supervise_wait;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_t_rows;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod sim_t_rows_default;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_task;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_task_axes;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_task_resume;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod sim_tenancy;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g1"))]
mod sim_tenancy_flood;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod sim_tenancy_model;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sim_tenancy_scope;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod sim_tenant_delivery;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_tenant_journal_kill;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod sim_tenant_store;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod skew;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod spec_front_door;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod spec_materialize;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod stability;
#[cfg(feature = "script")]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod start_gate;
#[cfg(all(feature = "edge", feature = "ephemeral"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod start_gate_edge;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod store_read_failure;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g5"))]
mod sys_process_binding;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod sys_process_portable;
#[cfg(any(not(lwc_it_group), lwc_it_group = "surface"))]
mod t02_compile_surface;
#[cfg(any(not(lwc_it_group), lwc_it_group = "surface"))]
mod t22_process_surface;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod t_rows;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
mod t_rows_default;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod task_front_door;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g7"))]
mod task_million;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod task_resume;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g4"))]
mod tenancy;
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod tenancy_scale;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g6"))]
mod tenant_store;
#[cfg(all(feature = "ecs", feature = "wire"))]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g8"))]
mod wrong_identity_evidence;

#[cfg(all(feature = "wire", any(not(lwc_it_group), lwc_it_group = "g1")))]
mod record_journal_cli;

#[cfg(all(feature = "wire", any(not(lwc_it_group), lwc_it_group = "g2")))]
mod broker_fence_cli;

#[cfg(all(
    feature = "ecs",
    feature = "wire",
    any(not(lwc_it_group), lwc_it_group = "g2")
))]
mod broker_fence_dispatch;

#[cfg(all(feature = "wire", any(not(lwc_it_group), lwc_it_group = "g2")))]
mod sim_broker_fence;
