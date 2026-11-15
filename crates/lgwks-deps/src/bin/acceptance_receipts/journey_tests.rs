//! The CI journey end to end: JUnit shards in, a receipt in the database, the
//! spec's table rewritten from it, and the check that holds the table honest.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use crate::fixture::Fixture;
use crate::{Streams, execute};

type TestResult = Result<(), Box<dyn Error>>;

/// The map: two rows, one with a gap.
const MAP: &str = "schema = 1\nspec = \"docs/spec.md\"\n\n[[row]]\nid = \"T01\"\nrequires = \"DX-01\"\nstate = \"exercised\"\ngap = \"\"\ntests = [\"it::a::one_t01\", \"it::a::two_t01\"]\n\n[[row]]\nid = \"T02\"\nrequires = \"DX-02\"\nstate = \"present\"\ngap = \"the second half is not asserted\"\ntests = [\"it::b::one_t02\"]\n";

/// The spec, with an empty generated block.
const SPEC: &str =
    "# Spec\n\n<!-- acceptance-table: start -->\nold\n<!-- acceptance-table: end -->\n";

/// A JUnit report holding the given `(name, passes)` cases.
fn report(cases: &[(&str, bool)]) -> String {
    let body: Vec<String> = cases
        .iter()
        .map(|entry| {
            let child = if entry.1 { "" } else { "<failure message=\"failed\"/>" };
            format!("<testcase classname=\"lgwks_bot::it\" name=\"{}\" time=\"0.010\">{child}</testcase>", entry.0)
        })
        .collect();
    format!(
        "<?xml version=\"1.0\"?><testsuites><testsuite name=\"lgwks_bot\">{}</testsuite></testsuites>",
        body.join("")
    )
}

/// Runs the command line and returns the code with both streams.
fn run(arguments: &[String]) -> Result<(u8, String, String), Box<dyn Error>> {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = execute(
        arguments,
        &mut Streams {
            out: &mut out,
            err: &mut err,
        },
    )?;
    Ok((
        code,
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    ))
}

/// The arguments every step of the journey shares.
fn common(root: &Path, revision: &str) -> Vec<String> {
    [
        "--root",
        &root.display().to_string(),
        "--db",
        &root.join("receipts.sqlite").display().to_string(),
        "--revision",
        revision,
        "--head",
        revision,
        "--platform",
        "linux-x86_64",
    ]
    .iter()
    .map(|argument| (*argument).to_owned())
    .collect()
}

/// A fixture holding the map, the spec and the two shard reports.
fn journey(passing: bool) -> Result<(Fixture, Vec<String>), Box<dyn Error>> {
    let fixture = Fixture::claim(&std::env::temp_dir(), "lgwks-acceptance-test")?;
    let root = fixture.path();
    fs::create_dir_all(root.join("docs/acceptance"))?;
    fs::write(root.join("docs/acceptance/t-rows.toml"), MAP)?;
    fs::write(root.join("docs/spec.md"), SPEC)?;
    fs::write(
        root.join("shard-1.xml"),
        report(&[("a::one_t01", true), ("a::two_t01", passing)]),
    )?;
    fs::write(
        root.join("shard-2.xml"),
        report(&[("b::one_t02", true), ("unrelated::elsewhere", true)]),
    )?;
    let mut arguments = common(
        root,
        if passing {
            "rev-journey"
        } else {
            "rev-failing"
        },
    );
    arguments.push("--from-junit".to_owned());
    arguments.push(root.join("shard-1.xml").display().to_string());
    arguments.push(root.join("shard-2.xml").display().to_string());
    Ok((fixture, arguments))
}

#[test]
fn a_green_journey_records_renders_exports_and_checks() -> TestResult {
    let (fixture, mut arguments) = journey(true)?;
    arguments.extend([
        "--spec".to_owned(),
        "docs/spec.md".to_owned(),
        "--write-table".to_owned(),
        "--export".to_owned(),
        fixture
            .path()
            .join("out/receipt.json")
            .display()
            .to_string(),
    ]);
    let (code, out, err) = run(&arguments)?;
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.contains("rewrote the generated table in docs/spec.md"),
        "{out}"
    );
    assert!(
        out.contains("tests_passed=3/3 rows_still_partial=1 partial=T02"),
        "{out}"
    );
    let table = fs::read_to_string(fixture.path().join("docs/spec.md"))?;
    assert!(table.contains("| T01 | exercised | 2/2 | — |"), "{table}");
    assert!(
        table.contains("| T02 | present | 1/1 | the second half is not asserted |"),
        "{table}"
    );
    let exported = fs::read_to_string(fixture.path().join("out/receipt.json"))?;
    assert!(
        exported.contains("\"observed_state\": \"exercised\""),
        "{exported}"
    );
    assert!(exported.ends_with("}\n"), "the export ends with a newline");
    Ok(())
}

#[test]
fn the_check_passes_on_a_rendered_table_and_fails_on_an_edited_one() -> TestResult {
    let (fixture, arguments) = journey(true)?;
    let mut write = arguments;
    write.extend([
        "--spec".to_owned(),
        "docs/spec.md".to_owned(),
        "--write-table".to_owned(),
    ]);
    let (code, out, err) = run(&write)?;
    assert_eq!(code, 0, "{out}{err}");
    let mut check = common(fixture.path(), "rev-journey");
    check.extend([
        "--spec".to_owned(),
        "docs/spec.md".to_owned(),
        "--check".to_owned(),
    ]);
    let (code, out, err) = run(&check)?;
    assert_eq!(code, 0, "a rendered table checks: {out}{err}");
    let spec = fixture.path().join("docs/spec.md");
    let edited = fs::read_to_string(&spec)?.replace("| T01 | exercised |", "| T01 | accepted |");
    fs::write(&spec, edited)?;
    let (code, _, err) = run(&check)?;
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("is not what the map and the receipt render"),
        "{err}"
    );
    Ok(())
}

#[test]
fn a_failing_named_test_fails_the_journey_and_is_named() -> TestResult {
    let (_fixture, arguments) = journey(false)?;
    let (code, _, err) = run(&arguments)?;
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("named tests not passing: 1"), "{err}");
    assert!(err.contains("T01::it::a::two_t01=failed"), "{err}");
    Ok(())
}

#[test]
fn a_named_test_no_shard_reported_is_not_run() -> TestResult {
    let (fixture, arguments) = journey(true)?;
    fs::write(
        fixture.path().join("shard-2.xml"),
        report(&[("unrelated::elsewhere", true)]),
    )?;
    let (code, _, err) = run(&arguments)?;
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("T02::it::b::one_t02 was not run"), "{err}");
    Ok(())
}

/// The file that holds the module `name`, declared in `file` (`mod name;` with an
/// optional `#[path]`, or `mod name {` inline), as rustc resolves it.
fn module_file(file: &Path, name: &str) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let source = fs::read_to_string(file)?;
    let lines: Vec<&str> = source.lines().map(str::trim).collect();
    let dir = file.parent().ok_or("a source file has a parent")?;
    for (at, line) in lines.iter().enumerate() {
        let bare = match line.strip_prefix("pub") {
            Some(rest) if rest.starts_with(' ') || rest.starts_with('(') => {
                match rest.split_once(' ') {
                    Some((_, tail)) => tail,
                    None => continue,
                }
            }
            _ => line,
        };
        if bare == format!("mod {name} {{") {
            return Ok(Some(file.to_path_buf()));
        }
        if bare != format!("mod {name};") {
            continue;
        }
        for attribute in lines
            .iter()
            .take(at)
            .rev()
            .take_while(|above| above.starts_with("#["))
        {
            if let Some(path) = attribute
                .strip_prefix("#[path = \"")
                .and_then(|rest| rest.strip_suffix("\"]"))
            {
                return Ok(Some(dir.join(path)));
            }
        }
        let owner = matches!(
            file.file_name().and_then(|stem| stem.to_str()),
            Some("mod.rs" | "main.rs" | "lib.rs")
        );
        let base = if owner {
            dir.to_path_buf()
        } else {
            dir.join(file.file_stem().ok_or("a source file has a stem")?)
        };
        let flat = base.join(format!("{name}.rs"));
        return Ok(Some(if flat.is_file() {
            flat
        } else {
            base.join(name).join("mod.rs")
        }));
    }
    Ok(None)
}

/// Whether `source` holds `name` as a whole identifier, which a `fn` or a
/// test-generating macro's argument both are.
fn holds_identifier(source: &str, name: &str) -> bool {
    let word = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    source.match_indices(name).any(|(at, _)| {
        let before = source.get(..at);
        let after = source.get(at..).and_then(|rest| rest.strip_prefix(name));
        let (Some(before), Some(after)) = (before, after) else {
            return false;
        };
        !before.chars().next_back().is_some_and(word) && !after.chars().next().is_some_and(word)
    })
}

/// Every name the committed maps list is a test its module path reaches, so a test
/// file split that moves a test without its map entry fails here, before the CI
/// receipt step reports the test as not run (#1063's split left 68 Linux map names
/// stale that way).
#[test]
fn every_committed_map_name_reaches_a_declared_test() -> TestResult {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bot = repo.join("crates/lgwks-bot");
    let (mut named, mut lost) = (0_usize, Vec::new());
    for map in ["t-rows.toml", "t-rows-darwin.toml", "t-rows-windows.toml"] {
        let (_, rows) = crate::render::load_rows(&repo.join("docs/acceptance").join(map))?;
        for (row, test) in rows
            .iter()
            .flat_map(|row| row.tests.iter().map(move |test| (&row.id, test)))
        {
            named = named.saturating_add(1);
            let mut segments: Vec<&str> = test.split("::").collect();
            let (Some(leaf), Some(binary)) = (segments.pop(), segments.first().copied()) else {
                lost.push(format!("{map} {row}: {test} is not <binary>::<test>"));
                continue;
            };
            let flat = bot.join("tests").join(format!("{binary}.rs"));
            let mut file = Some(match binary {
                "lgwks_bot" => bot.join("src/lib.rs"),
                _ if flat.is_file() => flat,
                _ => bot.join("tests").join(binary).join("main.rs"),
            });
            for segment in segments.iter().skip(1) {
                file = match file {
                    Some(parent) if parent.is_file() => module_file(&parent, segment)?,
                    _ => None,
                };
            }
            let found = match file.as_ref() {
                Some(path) if path.is_file() => holds_identifier(&fs::read_to_string(path)?, leaf),
                _ => false,
            };
            if !found {
                lost.push(format!(
                    "{map} {row}: {test} is not declared where its path leads ({file:?})"
                ));
            }
        }
    }
    assert!(named > 500, "the three maps name {named} tests");
    assert!(
        lost.is_empty(),
        "{} map names reach no test:\n{}",
        lost.len(),
        lost.join("\n")
    );
    Ok(())
}

#[test]
fn a_module_path_resolves_through_path_attributes_inline_modules_and_mod_rs() -> TestResult {
    let fixture = Fixture::claim(&std::env::temp_dir(), "lgwks-acceptance-modules")?;
    let root = fixture.path();
    fs::create_dir_all(root.join("parent/deep"))?;
    fs::write(
        root.join("main.rs"),
        "#[cfg(test)]\n#[path = \"elsewhere/moved.rs\"]\nmod moved;\npub(crate) mod parent;\nmod inline {\n}\n",
    )?;
    fs::write(root.join("parent.rs"), "mod child;\nmod deep;\n")?;
    fs::write(root.join("parent/child.rs"), "fn a_case_t01() {}\n")?;
    fs::write(root.join("parent/deep/mod.rs"), "fn other() {}\n")?;
    let main = root.join("main.rs");
    assert_eq!(
        module_file(&main, "moved")?,
        Some(root.join("elsewhere/moved.rs"))
    );
    assert_eq!(module_file(&main, "parent")?, Some(root.join("parent.rs")));
    assert_eq!(module_file(&main, "inline")?, Some(main.clone()));
    assert_eq!(module_file(&main, "absent")?, None);
    let parent = root.join("parent.rs");
    assert_eq!(
        module_file(&parent, "child")?,
        Some(root.join("parent/child.rs"))
    );
    assert_eq!(
        module_file(&parent, "deep")?,
        Some(root.join("parent/deep/mod.rs"))
    );
    let child = fs::read_to_string(root.join("parent/child.rs"))?;
    assert!(holds_identifier(&child, "a_case_t01"));
    assert!(
        !holds_identifier(&child, "a_case"),
        "a prefix is not the identifier"
    );
    assert!(
        !holds_identifier(&child, "case_t01"),
        "a suffix is not the identifier"
    );
    Ok(())
}

#[test]
fn an_unknown_argument_and_a_missing_map_are_refusals() -> TestResult {
    let (fixture, _) = journey(true)?;
    let unknown = run(&["--bogus".to_owned()])
        .err()
        .map(|error| error.to_string());
    assert_eq!(
        unknown.map(|text| text.contains("unknown argument")),
        Some(true),
        "an unknown flag"
    );
    let mut missing = common(fixture.path(), "rev");
    missing.extend(["--map".to_owned(), "docs/absent.toml".to_owned()]);
    let refused = run(&missing).err().map(|error| error.to_string());
    assert_eq!(
        refused.map(|text| text.contains("row map not found")),
        Some(true),
        "a missing map"
    );
    Ok(())
}
