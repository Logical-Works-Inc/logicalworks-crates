//! The probe child: this test binary re-invoked to run one named test.

/// This test binary re-invoked to run the one test named `test` as a probe child.
///
/// One builder for every kill and crash probe, so the executable and the
/// argument shape cannot drift between them. `test` is the libtest name: the
/// test's path below the crate root, which is what a test's own thread is named
/// and what [`probe_test`] builds from a `module_path!()`.
pub(crate) fn probe_command(
    test: &str,
) -> Result<std::process::Command, Box<dyn std::error::Error>> {
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.args([test, "--exact", "--nocapture"]);
    Ok(command)
}

/// The libtest name of `test` declared in `module` (its `module_path!()`).
///
/// libtest names a test by its path below the crate root; the crate's own
/// segment is what `module_path!()` adds and an `--exact` filter must not carry.
pub(crate) fn probe_test(module: &str, test: &str) -> String {
    match module.split_once("::") {
        Some((_crate, path)) => format!("{path}::{test}"),
        None => test.to_owned(),
    }
}

/// A probe child is selected by `--exact`, so a name carrying the crate's own
/// segment selects nothing: the child runs zero tests, exits 0, and the parent
/// reads a kill probe that never ran as one that passed.
///
/// This file is compiled into every `lwc_it_group` shard, and the acceptance
/// receipts refuse a test name two shards report, so its tests run in one group.
#[test]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
fn a_probe_name_drops_only_the_crate_segment() {
    assert_eq!(
        probe_test("it::journal_continuation", "a_kill"),
        "journal_continuation::a_kill",
        "the crate segment is what module_path!() adds and libtest does not name"
    );
    assert_eq!(
        probe_test("it::sim::clock::kill", "probe"),
        "sim::clock::kill::probe",
        "only the first segment goes; nested modules stay in the name"
    );
    assert_eq!(
        probe_test("it", "root_probe"),
        "root_probe",
        "a test at the crate root is named by itself"
    );
    assert_eq!(
        probe_test("", "bare"),
        "bare",
        "an empty module path names the test alone"
    );
}

/// The child command is this binary, filtered to exactly one test, with its
/// output left uncaptured so the parent sees what the probe printed.
#[test]
#[cfg(any(not(lwc_it_group), lwc_it_group = "g3"))]
fn a_probe_command_runs_this_binary_on_one_exact_test() -> Result<(), Box<dyn std::error::Error>> {
    let command = probe_command("journal_continuation::a_kill")?;
    assert_eq!(
        command.get_program(),
        std::env::current_exe()?.as_os_str(),
        "the probe is this same test binary"
    );
    let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
    assert_eq!(
        args,
        ["journal_continuation::a_kill", "--exact", "--nocapture"],
        "one exact test, output uncaptured"
    );
    Ok(())
}
