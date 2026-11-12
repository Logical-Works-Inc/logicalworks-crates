//! The Linux leg runs the lane table's commands, nothing shorter: each suite is
//! held to the lane that defines it in `scripts/gate-lanes.toml` (INV-GOV-1).

use std::error::Error;

use crate::plan::{shell_line, suite_commands};
use crate::tests::repo_file;
use crate::toml_subset::{read_tables, text};

type TestResult = Result<(), Box<dyn Error>>;

/// The `command` of the lane called `id` in the lane table.
fn lane_command(lanes: &str, id: &str) -> Option<String> {
    read_tables(lanes)
        .iter()
        .filter(|table| table.path == "lane")
        .find(|table| text(table, "id") == Some(id))
        .and_then(|table| text(table, "command"))
        .map(str::to_owned)
}

/// `command` as a shell runs it with every `${...}` slot unset: a
/// `${NAME:-default}` slot reads as its default, and any other (`:+`, or a bare
/// `${NAME}`) as nothing. CI fills the slots (a test archive, an apps row's
/// filter, a partition, province#9); a local run leaves them unset.
fn with_slots_unset(command: &str) -> Result<String, &'static str> {
    let mut expanded = command.to_owned();
    while let Some((head, tail)) = expanded.split_once("${") {
        let (inner, rest) = tail.split_once('}').ok_or("an unclosed slot")?;
        let unset = match inner.split_once(":-") {
            Some((_, default)) => default,
            None => "",
        };
        expanded = format!("{head}{unset}{rest}");
    }
    Ok(expanded)
}

#[test]
fn the_commands_are_the_ones_the_lane_table_names() -> TestResult {
    let lanes = repo_file("scripts/gate-lanes.toml")?;
    for (suite, lane) in [
        ("workspace", "tests"),
        ("apps", "app-builds"),
        ("bot-full", "tests-bot-full"),
    ] {
        let command = lane_command(&lanes, lane).ok_or("a lane is missing from the table")?;
        // The leg single-quotes an argument that a lane double-quotes for its slot.
        let command = with_slots_unset(&command)?.replace('"', "'");
        let ours: Vec<String> = suite_commands(suite, None)
            .ok_or("a suite is unknown")?
            .iter()
            .map(|argv| shell_line(argv))
            .collect();
        assert!(
            command.starts_with(&ours.join(" && ")),
            "{suite} runs {ours:?} but the {lane} lane is {command}"
        );
    }
    Ok(())
}

/// Catches the clean-consumer gate journey running in the workspace suite, or
/// in no suite at all.
///
/// Source: run 43213006642, where the lane table had moved the journey to the
/// `app-builds` lane (#927) and this leg still ran it in `workspace`. The
/// workspace filter must exclude it under every partition and the apps filter
/// must select it, each exactly once.
#[test]
fn the_gate_journey_leaves_the_workspace_suite_for_the_apps_suite() -> TestResult {
    let journey = "test(=gate_consumer::sim_a_clean_consumer_passes_the_gate)";
    let filter_of = |suite: &str, partition: Option<&str>| -> Result<String, &'static str> {
        let commands = suite_commands(suite, partition).ok_or("a suite is unknown")?;
        let argv = commands.first().ok_or("a suite runs no command")?;
        let at = argv
            .iter()
            .position(|word| word == "-E")
            .ok_or("a suite has no filter")?;
        argv.get(at.saturating_add(1))
            .cloned()
            .ok_or("a filter flag has no value")
    };
    let whole = filter_of("workspace", None)?;
    let sliced = filter_of("workspace", Some("count:10/10"))?;
    assert_eq!(
        whole, sliced,
        "a partition never changes what the workspace suite excludes"
    );
    assert_eq!(
        whole.matches(journey).count(),
        1,
        "workspace names the journey once: {whole}"
    );
    assert!(
        whole.contains(&format!("or {journey}))")),
        "the journey sits inside the workspace suite's exclusion: {whole}"
    );
    let apps = filter_of("apps", None)?;
    assert_eq!(
        apps.matches(journey).count(),
        1,
        "apps selects the journey once: {apps}"
    );
    assert!(
        apps.starts_with("binary(it) & ("),
        "apps selects it from the it binary: {apps}"
    );
    Ok(())
}

/// Catches a slot reader that fails on the `:+` form the `tests` lane uses,
/// keeps a slot's braces, or reads past an unclosed one.
#[test]
fn every_slot_form_reads_as_an_unset_shell_reads_it() {
    let cases = [
        ("a ${X:-b c} d", Ok("a b c d")),
        ("a ${P:+--partition $P}", Ok("a ")),
        ("a ${BARE} b", Ok("a  b")),
        ("${A:-1}${B:-2}${C:+3}", Ok("12")),
        ("x ${D:-y:-z} w", Ok("x y:-z w")),
        ("no slots", Ok("no slots")),
        ("", Ok("")),
        ("a ${OPEN:-never closed", Err("an unclosed slot")),
    ];
    for (command, expected) in cases {
        assert_eq!(
            with_slots_unset(command),
            expected.map(str::to_owned),
            "{command:?}"
        );
    }
}
