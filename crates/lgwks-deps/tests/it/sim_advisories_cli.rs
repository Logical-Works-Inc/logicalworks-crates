//! `lgwks-deps advisories` end to end (#799): the shipped binary, a lock and a
//! database on disk, judged by a reference model in plain integers.
//!
//! RustSec's advisory format (rustsec/advisory-db, `[versions] patched`) writes
//! one requirement per array item with commas meaning AND inside an item, as in
//! `[">= 1.0.1, < 1.0.5", ">= 1.2.0"]`. Each sim names the bug it catches.

use std::path::Path;
use std::process::Command;

use lgwks_std::json::{self, Value};

use crate::lock_fixture::{Package, Scratch, render};
use crate::replay::{Failure, refuse, replay_sweep};

/// One run of the binary.
struct Run {
    /// The exit code, or `None` when a signal ended the process.
    code: Option<i32>,
    /// What it wrote to stdout.
    stdout: String,
    /// What it wrote to stderr.
    stderr: String,
}

/// Runs `lgwks-deps advisories` with `args`.
fn advisories(args: &[&str]) -> Result<Run, Failure> {
    let binary = std::env::var_os("CARGO_BIN_EXE_lgwks-deps")
        .ok_or("Cargo did not provide the lgwks-deps test binary")?;
    let output = Command::new(binary).arg("advisories").args(args).output()?;
    Ok(Run {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout)?,
        stderr: String::from_utf8(output.stderr)?,
    })
}

/// Writes a lock holding `package` at `version`, and one advisory beside it.
fn world(
    root: &Path,
    package: &str,
    version: &str,
    advisory: &str,
) -> Result<(String, String), Failure> {
    let lock = root.join("Cargo.lock");
    std::fs::write(&lock, render(&[Package::registry(package, version, &[])])?)?;
    let database = root.join("db");
    let holder = database.join("crates").join(package);
    std::fs::create_dir_all(&holder)?;
    std::fs::write(holder.join("RUSTSEC-2026-0001.md"), advisory)?;
    Ok((
        lock.to_string_lossy().into_owned(),
        database.to_string_lossy().into_owned(),
    ))
}

/// An advisory for `package` whose `[versions]` block is `versions`.
fn advisory_text(package: &str, versions: &str) -> String {
    format!(
        "```toml\n[advisory]\nid = \"RUSTSEC-2026-0001\"\npackage = \"{package}\"\ndate = \"2026-01-01\"\n\n[versions]\n{versions}\n```\n\n# A flaw\n"
    )
}

/// Catches the fail-open this command shipped with: splitting a requirement on
/// its comma made `>= 1.0.1` an alternative of its own, so every later vulnerable
/// version read as patched. The reference model is the interval arithmetic.
#[test]
fn sim_advisories_weird_bytes_a_comma_is_a_conjunction_never_an_alternative() -> Result<(), Failure>
{
    replay_sweep("conjunction", 16, |rng, trace| {
        let scratch = Scratch::new("advisories-comma")?;
        let (low, high) = (rng.between(0, 4), rng.between(5, 9));
        let major_fix = rng.between(1, 3);
        let installed = rng.between(0, 12);
        let minor = rng.between(0, major_fix);
        let version = format!("1.{minor}.{installed}");
        let versions =
            format!("patched = [\">= 1.0.{low}, < 1.0.{high}\", \">= 1.{major_fix}.0\"]");
        let (lock, database) = world(
            scratch.path(),
            "vuln",
            &version,
            &advisory_text("vuln", &versions),
        )?;
        let run = advisories(&[&lock, &database])?;
        let in_band = minor == 0 && installed >= low && installed < high;
        let patched = in_band || minor >= major_fix;
        let expected = if patched { 0 } else { 1 };
        if run.code != Some(expected) {
            return refuse(format!(
                "{version} against {versions}: expected exit {expected}, got {:?}: {}{}",
                run.code, run.stdout, run.stderr
            ));
        }
        trace.record_number("verdict", expected);
        Ok(())
    })
}

/// Catches: a missing or broken database reading as a clean audit, which is the
/// one answer this command may never give when it could not look.
#[test]
fn sim_advisories_fat_finger_refusals_are_never_a_clean_report() -> Result<(), Failure> {
    let scratch = Scratch::new("advisories-refusal")?;
    let (lock, database) = world(
        scratch.path(),
        "vuln",
        "1.0.0",
        &advisory_text("vuln", "patched = [\"not a version\"]"),
    )?;
    let absent = scratch.path().join("nothing");
    let absent = absent.to_string_lossy();
    let cases: [&[&str]; 6] = [
        &[],
        &[lock.as_str()],
        &[lock.as_str(), database.as_str(), "third"],
        &[lock.as_str(), database.as_str(), "--verbose"],
        &[lock.as_str(), absent.as_ref()],
        &[lock.as_str(), database.as_str()],
    ];
    for case in cases {
        let run = advisories(case)?;
        if run.code != Some(2) {
            return refuse(format!("{case:?} exited {:?}", run.code));
        }
        if run.stderr.is_empty() {
            return refuse(format!("{case:?} refused without saying why"));
        }
    }
    Ok(())
}

/// Catches: a withdrawn advisory or an informational one failing the audit, and
/// a local package being judged by a crates.io advisory that shares its name.
#[test]
fn sim_advisories_neighbour_a_withdrawn_or_informational_advisory_does_not_fail()
-> Result<(), Failure> {
    let scratch = Scratch::new("advisories-withdrawn")?;
    let withdrawn = "```toml\n[advisory]\nid = \"RUSTSEC-2026-0001\"\npackage = \"vuln\"\ndate = \"2026-01-01\"\nwithdrawn = \"2026-02-01\"\n\n[versions]\npatched = [\">= 9.0.0\"]\n```\n";
    let (lock, database) = world(scratch.path(), "vuln", "1.0.0", withdrawn)?;
    let run = advisories(&[&lock, &database])?;
    if run.code != Some(0) {
        return refuse(format!("a withdrawn advisory exited {:?}", run.code));
    }
    let local = scratch.path().join("Local.lock");
    std::fs::write(&local, render(&[Package::local("vuln", &[])])?)?;
    let vulnerable = advisory_text("vuln", "patched = [\">= 9.0.0\"]");
    let holder = scratch.path().join("db").join("crates").join("vuln");
    std::fs::write(holder.join("RUSTSEC-2026-0001.md"), vulnerable)?;
    let run = advisories(&[&local.to_string_lossy(), &database])?;
    if run.code != Some(0) {
        return refuse(format!("a workspace member exited {:?}", run.code));
    }
    Ok(())
}

/// Catches: the text report swapped for JSON when `--json` is absent, or the
/// matched line losing its advisory, version or lockfile. A person at a terminal
/// reads this form (#942).
#[test]
fn sim_advisories_text_report_names_the_match_and_the_counts() -> Result<(), Failure> {
    let scratch = Scratch::new("advisories-text")?;
    let (lock, database) = world(
        scratch.path(),
        "vuln",
        "2.0.0-alpha.1",
        &advisory_text("vuln", "patched = [\">= 1.2.3\"]"),
    )?;
    let run = advisories(&[&lock, &database])?;
    if run.code != Some(1) {
        return refuse(format!(
            "expected exit 1, got {:?}: {}",
            run.code, run.stderr
        ));
    }
    let text = run.stdout.trim_start();
    if text.starts_with('{') {
        return refuse(format!("the text form printed JSON: {}", run.stdout));
    }
    let named = text.contains("RUSTSEC-2026-0001 vuln 2.0.0-alpha.1 vulnerability")
        && text.contains(lock.as_str())
        && text.contains("packages audited in 1 lockfiles");
    if !named {
        return refuse(format!("the text report lost a field: {}", run.stdout));
    }
    Ok(())
}

/// Catches: a second lockfile dropped, or its match credited to the first. The
/// command takes every path but the last as a lockfile (#942).
#[test]
fn sim_advisories_two_lockfiles_each_carry_their_own_match() -> Result<(), Failure> {
    let scratch = Scratch::new("advisories-two-locks")?;
    let (clean, database) = world(
        scratch.path(),
        "vuln",
        "9.0.0",
        &advisory_text("vuln", "patched = [\">= 9.0.0\"]"),
    )?;
    let vulnerable = scratch.path().join("Vulnerable.lock");
    std::fs::write(
        &vulnerable,
        render(&[Package::registry("vuln", "2.0.0-alpha.1", &[])])?,
    )?;
    let vulnerable = vulnerable.to_string_lossy().into_owned();
    let run = advisories(&[&clean, &vulnerable, &database, "--json"])?;
    if run.code != Some(1) {
        return refuse(format!(
            "expected exit 1, got {:?}: {}",
            run.code, run.stderr
        ));
    }
    let parsed: Value = json::from_str(run.stdout.trim())?;
    if parsed.get("lockfiles").and_then(Value::as_u64) != Some(2) {
        return refuse(format!("two lockfiles not counted in {}", run.stdout));
    }
    let Some(rows) = parsed.get("matches").and_then(Value::as_array) else {
        return refuse(format!("no matches array in {}", run.stdout));
    };
    let labels: Vec<Option<&str>> = rows
        .iter()
        .map(|row| row.get("lockfile").and_then(Value::as_str))
        .collect();
    if labels != vec![Some(vulnerable.as_str())] {
        return refuse(format!(
            "the pre-release match is not the second lock's: {}",
            run.stdout
        ));
    }
    Ok(())
}

/// Runs last: the machine's answer is one parseable document naming what the
/// exit code decided, whatever bytes the advisory's requirement holds. Catches:
/// JSON broken by a quote, a tab or a comma inside a requirement.
#[test]
fn sim_advisories_saboteur_json_is_one_document_that_agrees_with_the_exit() -> Result<(), Failure> {
    let scratch = Scratch::new("advisories-json")?;
    let (lock, database) = world(
        scratch.path(),
        "vuln",
        "1.0.0",
        &advisory_text("vuln", "patched = [\">= 2.0.0, < 3.0.0\", \">= 4.0.0\"]"),
    )?;
    let run = advisories(&[&lock, &database, "--json"])?;
    if run.code != Some(1) {
        return refuse(format!("expected exit 1, got {:?}", run.code));
    }
    let parsed: Value = json::from_str(run.stdout.trim())?;
    let Some(rows) = parsed.get("matches").and_then(Value::as_array) else {
        return refuse(format!("no matches array in {}", run.stdout));
    };
    let Some(row) = rows.first().filter(|_| rows.len() == 1) else {
        return refuse(format!("expected one match in {}", run.stdout));
    };
    if row.get("kind").and_then(Value::as_str) != Some("vulnerability") {
        return refuse(format!("wrong kind in {}", run.stdout));
    }
    let patched = row.get("patched").and_then(Value::as_array);
    if patched.map(Vec::len) != Some(2) {
        return refuse(format!("a requirement was split in {}", run.stdout));
    }
    Ok(())
}
