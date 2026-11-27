//! Deterministic simulation of the raw-process detector (#1160 G3).
//!
//! Source: issue #1160, where the tests of two consumer repositories started
//! `logical_ci serve` and `larry acp` with a raw `Command`, a SIGKILL ended the
//! test, and the children held a 24 GB Mac for hours; and the supervision
//! skill, which says a `ProcessSpec` handed to the `Supervisor` is the one way
//! to start a child. Requirement: INV-SCAN-RAW-PROCESS-1, every raw start of a
//! `std` or `tokio` process `Command` is found, in test code too, and the
//! finding names the owner to use.
//!
//! One seed draws a file: an import of `Command` (from `std::process`,
//! `tokio::process`, a group, an alias, a local type or none) and calls of
//! several spellings, each placed in a plain function, a `#[test]`, a closure,
//! a macro's arguments or a `#[cfg(test)]` module. An independent model,
//! written from the rule's contract, names the line of every raw start; the
//! scan must agree line for line, name the `Supervisor` on every finding and
//! the lifeline's `hold_device` on a disk tool, and replay to one trace.

#![cfg(feature = "scan")]

use lgwks_ast::scan::{Rule, scan_source, scan_source_with};

use crate::sim::{Rng, Trace, receipt};

/// What a test returns.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Seeds the agreement family draws, each one whole file.
const SEEDS: u64 = 2_048;

/// The most calls one file holds.
const MAX_CALLS: u32 = 8;

/// The rule's name.
const RULE: &str = "raw-process";

/// How a file imports `Command`: the line, whether a bare `Command` is then the
/// process one, and whether `Cmd` is.
const IMPORTS: [(&str, bool, bool); 9] = [
    ("", false, false),
    ("use std::process::Command;\n", true, false),
    ("use tokio::process::Command;\n", true, false),
    ("use std::process::{self, Command, Stdio};\n", true, false),
    ("use std::process::Command as Cmd;\n", false, true),
    ("use crate::app::Command;\n", false, false),
    ("use std::process;\n", false, false),
    // The owner (ADR 0092): its start holds the group, so no spelling of it
    // is raw.
    ("use lgwks_std::process::Command;\n", false, false),
    (
        "use lgwks_std::process::{Command as Cmd, Stdio};\n",
        false,
        false,
    ),
];

/// When a call spelling is a raw start.
#[derive(Clone, Copy, Debug)]
enum Raw {
    /// Raw when the file imports `Command` from a process module.
    Bare,
    /// Raw when the file imports the process `Command` as `Cmd`.
    Alias,
    /// Raw by its path alone.
    Always,
    /// Never a raw start.
    Never,
}

/// Each call spelling, with `@` for the program it starts.
const CALLS: [(&str, Raw); 11] = [
    ("Command::new(\"@\")", Raw::Bare),
    ("Cmd::new(\"@\")", Raw::Alias),
    ("std::process::Command::new(\"@\")", Raw::Always),
    ("tokio::process::Command::new(\"@\")", Raw::Always),
    ("process::Command::new(\"@\")", Raw::Always),
    ("::std::process::Command::new(\"@\")", Raw::Always),
    ("Command::default()", Raw::Never),
    ("builder::Command::new(\"@\")", Raw::Never),
    ("Supervisor::new(\"@\")", Raw::Never),
    ("lgwks_std::process::Command::new(\"@\")", Raw::Never),
    ("::lgwks_std::process::Command::new(\"@\")", Raw::Never),
];

/// Where a call sits, with `~` for the item's index and `%` for the call.
const PLACES: [&str; 5] = [
    "fn plain_~() { let _child = %; }",
    "#[test]\nfn test_~() { let _child = %; }",
    "fn closure_~() { run(|| %); }",
    "fn macro_~() { let _all = vec![%]; }",
    "#[cfg(test)]\nmod tests_~ {\n    fn helper() { let _child = %; }\n}",
];

/// The programs a call starts; the disk tools also owe a lifeline hold.
const PROGRAMS: [(&str, bool); 5] = [
    ("cargo", false),
    ("hdiutil", true),
    ("mount", true),
    ("diskutil", true),
    ("sh", false),
];

/// One drawn file and what the model says the scan must find in it.
struct Drawn {
    /// The source text.
    source: String,
    /// Each raw start: its line and whether it names a disk tool.
    expected: Vec<(usize, bool)>,
}

/// Draws one file from `seed`, recording every draw in `trace`.
fn draw(seed: u64, trace: &mut Trace) -> Result<Drawn, Box<dyn std::error::Error>> {
    let mut rng = Rng::new(seed);
    let &(import, bare, alias) = rng.pick_named("imports", &IMPORTS)?;
    trace.record(import);
    let mut source = format!("{import}\n");
    let mut expected = Vec::new();
    for index in 0..rng.between(1, MAX_CALLS) {
        let &(call, raw) = rng.pick_named("calls", &CALLS)?;
        let place = rng.pick_named("places", &PLACES)?;
        let &(program, disk) = rng.pick_named("programs", &PROGRAMS)?;
        trace.record(&format!("{index}:{call}:{place}:{program}"));
        let call = call.replace('@', program);
        let item = place.replace('~', &index.to_string()).replace('%', &call);
        let offset = item.find(&call).ok_or("a place holds its call")?;
        let head = item
            .get(..offset)
            .ok_or("the call starts inside its place")?;
        let line = source
            .lines()
            .count()
            .saturating_add(head.matches('\n').count())
            .saturating_add(1);
        source.push_str(&item);
        source.push('\n');
        let is_raw = match raw {
            Raw::Bare => bare,
            Raw::Alias => alias,
            Raw::Always => true,
            Raw::Never => false,
        };
        if is_raw {
            expected.push((line, disk));
        }
    }
    Ok(Drawn { source, expected })
}

/// The detector the rule name selects.
fn rule() -> Result<Rule, Box<dyn std::error::Error>> {
    Ok(Rule::from_name(RULE).ok_or("no raw-process detector")?)
}

/// Every seeded file agrees with the model line for line, every finding names
/// its owner, a whole scan never reports the opt-in rule, and the family
/// reaches both verdicts.
#[test]
fn sim_raw_process_finds_exactly_the_raw_starts() -> TestResult {
    let rule = rule()?;
    let (mut found, mut clean) = (0_usize, 0_usize);
    for seed in 0..SEEDS {
        let drawn = draw(seed, &mut Trace::new())?;
        let hits = scan_source_with(&drawn.source, "lib.rs", &[rule])?;
        let lines: Vec<usize> = hits.iter().map(|hit| hit.line()).collect();
        let want: Vec<usize> = drawn.expected.iter().map(|&(line, _)| line).collect();
        assert_eq!(lines, want, "seed {seed}: {hits:?}\n{}", drawn.source);
        for (hit, &(_, disk)) in hits.iter().zip(&drawn.expected) {
            assert_eq!(hit.rule(), RULE, "seed {seed}: {hit:?}");
            assert!(
                hit.snippet().contains("`lgwks_std::process::Command`")
                    && hit.snippet().contains("Supervisor"),
                "seed {seed}: {hit:?}"
            );
            assert_eq!(
                hit.snippet().contains("hold_device"),
                disk,
                "seed {seed}: {hit:?}"
            );
        }
        assert!(
            scan_source(&drawn.source, "lib.rs")?
                .iter()
                .all(|hit| hit.rule() != RULE),
            "seed {seed}: a whole scan ran the opt-in rule"
        );
        if lines.is_empty() {
            clean = clean.saturating_add(1);
        } else {
            found = found.saturating_add(1);
        }
    }
    assert!(found > 0 && clean > 0, "found {found}, clean {clean}");
    Ok(())
}

/// The same seed draws the same file to the same trace; distinct seeds diverge.
#[test]
fn sim_raw_process_the_same_seed_replays_to_one_trace() -> TestResult {
    let mut receipts = std::collections::BTreeSet::new();
    for seed in 0..SEEDS {
        let (mut first, mut second) = (Trace::new(), Trace::new());
        let drawn = draw(seed, &mut first)?;
        let again = draw(seed, &mut second)?;
        assert_eq!(drawn.source, again.source, "seed {seed}");
        assert_eq!(receipt(&first)?, receipt(&second)?, "seed {seed}");
        receipts.insert(receipt(&first)?);
    }
    assert!(receipts.len() > 1_000, "{} distinct traces", receipts.len());
    Ok(())
}

/// The rule is opt-in and selectable by name in any case, and a comment or a
/// string that spells a raw start is no start.
#[test]
fn sim_raw_process_reads_code_not_text() -> TestResult {
    let rule = rule()?;
    assert!(Rule::OPT_IN.contains(&rule), "raw-process is opt-in");
    assert_eq!(Rule::from_name("RAW-PROCESS"), Some(rule));
    let source = concat!(
        "use std::process::Command;\n",
        "// Command::new(\"hdiutil\") in a comment\n",
        "const DOC: &str = \"std::process::Command::new(x)\";\n",
        "fn real() { let _c = Command::new(\"hdiutil\").arg(\"attach\").spawn(); }\n",
    );
    let hits = scan_source_with(source, "lib.rs", &[rule])?;
    let lines: Vec<usize> = hits.iter().map(|hit| hit.line()).collect();
    assert_eq!(lines, [4], "{hits:?}");
    Ok(())
}

/// An author who hides the import: a process `Command` reached through a
/// nested group, a glob, a raw identifier and a macro's nested group is still
/// raw, while a crate's own `process::Command` and a `new` that is not called
/// are not. The disk tool is named by its full path, and the finding still
/// asks for the device hold.
#[test]
fn sim_raw_process_sees_through_hidden_imports() -> TestResult {
    let rule = rule()?;
    let cases: [(&str, &[usize]); 5] = [
        (
            "use std::{io, process::{Command as Run}};\nfn f() { let _c = Run::new(\"sh\"); }\n",
            &[2],
        ),
        (
            "use tokio::process::*;\nfn f() { let _c = Command::new(\"sh\"); }\n",
            &[2],
        ),
        (
            "use std::process::r#Command;\nfn f() { let _c = r#Command::new(\"sh\"); }\n",
            &[2],
        ),
        (
            "fn f() { run!({ [std::process::Command::new(\"/usr/bin/hdiutil\")] }); }\n",
            &[1],
        ),
        (
            "use crate::process::Command;\nfn f() { let _n = Command::new; let _c = crate::process::Command::new(\"sh\"); }\n",
            &[],
        ),
    ];
    for (source, want) in cases {
        let hits = scan_source_with(source, "lib.rs", &[rule])?;
        let lines: Vec<usize> = hits.iter().map(|hit| hit.line()).collect();
        assert_eq!(lines, want, "{source}: {hits:?}");
        if source.contains("hdiutil") {
            assert!(
                hits.iter()
                    .all(|hit| hit.snippet().contains("`lgwks_std::process::hold_device`")),
                "{hits:?}"
            );
        }
    }
    Ok(())
}
