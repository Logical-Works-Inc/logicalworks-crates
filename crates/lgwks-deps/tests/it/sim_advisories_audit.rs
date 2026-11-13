//! `lgwks_deps::advisories::audit`, the library entry logical_ci's advisory
//! gate calls (#942), against databases written to disk.
//!
//! The reference is cargo-audit 0.22.2 itself: `cargo audit --db <fixture>
//! --no-fetch --json` over the fixture below printed the rows in
//! [`CARGO_AUDIT_ROWS`] on 2026-11-12. The estate reports every one of them and
//! exactly two more, each named in [`ESTATE_ONLY_ROWS`] with why (ADR 0038).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use lgwks_deps::advisories::{AdvisoryError, AdvisoryReport, Lockfile, audit};

use crate::lock_fixture::{CRATES_IO, Package, Scratch, render};
use crate::replay::{Failure, refuse, replay_sweep};

/// The sparse spelling of the crates.io index.
const SPARSE_CRATES_IO: &str = "sparse+https://index.crates.io/";

/// `[affected]` of RUSTSEC-2020-0071 as advisory-db writes it: a multi-line
/// `os` array, then function requirements that are not version bounds.
const TIME_AFFECTED: &str = "[affected]\n# any Unix-like OS\nos = [\n    \"linux\",\n    \"redox\",\n    \"solaris\",\n    \"android\",\n    \"ios\",\n    \"macos\",\n    \"netbsd\",\n    \"openbsd\",\n    \"freebsd\",\n]\n[affected.functions]\n\"time::at\" = [\"^0.1\"]\n\"time::UtcOffset::local_offset_at\" = [\"< 0.2.23\"]\n";

/// The fixture advisories: id, package, extra `[advisory]` lines, the
/// `[affected]` block and the `[versions]` block.
const FIXTURE: [(&str, &str, &str, &str, &str); 11] = [
    (
        "RUSTSEC-2099-0001",
        "gated",
        "",
        "",
        "patched = [\">= 1.2.3\"]",
    ),
    (
        "RUSTSEC-2099-0002",
        "ranked",
        "",
        "",
        "patched = [\">= 1.0.0-rc.10\"]",
    ),
    (
        "RUSTSEC-2099-0003",
        "windowed",
        "",
        "",
        "patched = [\">= 0.7.1\"]\nunaffected = [\"< 0.5\"]",
    ),
    (
        "RUSTSEC-2099-0004",
        "specified",
        "",
        "",
        "patched = [\"^0.3.2\", \"~1.4\", \"=3.1.0-beta.2\", \">= 4\"]",
    ),
    (
        "RUSTSEC-2099-0005",
        "retracted",
        "withdrawn = \"2099-01-06\"\n",
        "",
        "patched = []",
    ),
    (
        "RUSTSEC-2099-0006",
        "abandoned",
        "informational = \"unmaintained\"\n",
        "",
        "patched = []",
    ),
    (
        "RUSTSEC-2099-0007",
        "narrow",
        "",
        "[affected]\nos = [\"windows\"]\narch = [\"x86_64\"]\n",
        "patched = [\">= 1.0.1, < 1.1.0\", \">= 1.1.3\"]",
    ),
    (
        "RUSTSEC-2021-0034",
        "office",
        "informational = \"unmaintained\"\n",
        "",
        "patched = []",
    ),
    (
        "RUSTSEC-2025-0153",
        "hexchat",
        "informational = \"unsound\"\n",
        "",
        "patched = []",
    ),
    (
        "RUSTSEC-2026-0382",
        "rustls-webpki",
        "",
        "",
        "patched = [\">= 0.103.16, < 0.104.0-alpha.1\", \">= 0.104.0-alpha.8\"]",
    ),
    (
        "RUSTSEC-2020-0071",
        "time",
        "",
        TIME_AFFECTED,
        "patched = [\">= 0.2.23\"]\nunaffected = [\"=0.2.0\", \"=0.2.1\", \"=0.2.2\", \"=0.2.3\", \"=0.2.4\", \"=0.2.5\", \"=0.2.6\"]",
    ),
];

/// The crates.io pins of the fixture lock, by package.
const PINS: [(&str, &[&str]); 11] = [
    ("gated", &["1.0.0", "1.2.3", "2.0.0-alpha.1", "2.0.0"]),
    (
        "ranked",
        &["0.9.0", "1.0.0-rc.9", "1.0.0-rc.10", "1.0.0-rc.11", "1.0.0"],
    ),
    ("windowed", &["0.4.9", "0.5.0", "0.7.0", "0.7.1"]),
    (
        "specified",
        &[
            "0.3.1",
            "0.3.5",
            "0.4.0",
            "1.4.9",
            "1.5.0",
            "2.7.0",
            "3.0.0",
            "3.1.0-beta.2",
            "3.1.0-beta.3",
        ],
    ),
    ("retracted", &["1.0.0"]),
    ("abandoned", &["0.1.0"]),
    ("narrow", &["1.0.0", "1.0.5", "1.1.0", "1.1.3"]),
    (
        "rustls-webpki",
        &["0.103.15", "0.103.16", "0.104.0-alpha.7", "0.104.0-alpha.8"],
    ),
    ("time", &["0.1.0", "0.2.7", "0.2.22", "0.2.23"]),
    ("hexchat", &["0.3.0"]),
    ("office", &["0.5.0"]),
];

/// Every row cargo-audit 0.22.2 printed for the fixture: vulnerabilities, then
/// the unmaintained and unsound warnings.
const CARGO_AUDIT_ROWS: [(&str, &str, &str); 21] = [
    ("RUSTSEC-2099-0001", "gated", "1.0.0"),
    ("RUSTSEC-2099-0002", "ranked", "0.9.0"),
    ("RUSTSEC-2099-0002", "ranked", "1.0.0-rc.9"),
    ("RUSTSEC-2099-0003", "windowed", "0.5.0"),
    ("RUSTSEC-2099-0003", "windowed", "0.7.0"),
    ("RUSTSEC-2099-0004", "specified", "0.3.1"),
    ("RUSTSEC-2099-0004", "specified", "0.4.0"),
    ("RUSTSEC-2099-0004", "specified", "1.5.0"),
    ("RUSTSEC-2099-0004", "specified", "2.7.0"),
    ("RUSTSEC-2099-0004", "specified", "3.0.0"),
    ("RUSTSEC-2099-0004", "specified", "3.1.0-beta.3"),
    ("RUSTSEC-2099-0007", "narrow", "1.0.0"),
    ("RUSTSEC-2099-0007", "narrow", "1.1.0"),
    ("RUSTSEC-2026-0382", "rustls-webpki", "0.103.15"),
    ("RUSTSEC-2026-0382", "rustls-webpki", "0.104.0-alpha.7"),
    ("RUSTSEC-2020-0071", "time", "0.1.0"),
    ("RUSTSEC-2020-0071", "time", "0.2.7"),
    ("RUSTSEC-2020-0071", "time", "0.2.22"),
    ("RUSTSEC-2099-0006", "abandoned", "0.1.0"),
    ("RUSTSEC-2021-0034", "office", "0.5.0"),
    ("RUSTSEC-2025-0153", "hexchat", "0.3.0"),
];

/// The rows the estate adds to cargo-audit's, both fail-closed by decision:
/// `2.0.0-alpha.1` is not patched by `>= 1.2.3` under Cargo's `VersionReq`
/// pre-release gate, which cargo-audit does not apply; and `1.1.0` is pinned
/// from the sparse crates.io index, which cargo-audit does not match.
const ESTATE_ONLY_ROWS: [(&str, &str, &str); 2] = [
    ("RUSTSEC-2099-0001", "gated", "2.0.0-alpha.1"),
    ("RUSTSEC-2099-0001", "gated", "1.1.0"),
];

/// The text of one advisory file.
fn advisory(id: &str, package: &str, extra: &str, affected: &str, versions: &str) -> String {
    format!(
        "```toml\n[advisory]\nid = \"{id}\"\npackage = \"{package}\"\ndate = \"2026-01-01\"\n{extra}\n{affected}\n[versions]\n{versions}\n```\n\n# A flaw in {package}\n"
    )
}

/// Writes `text` at `db/crates/<package>/<id>.md`.
fn write_advisory(db: &Path, package: &str, id: &str, text: &str) -> Result<(), Failure> {
    let holder = db.join("crates").join(package);
    std::fs::create_dir_all(&holder)?;
    std::fs::write(holder.join(format!("{id}.md")), text)?;
    Ok(())
}

/// Writes the fixture database under `db`.
fn write_fixture(db: &Path) -> Result<(), Failure> {
    for (id, package, extra, affected, versions) in FIXTURE {
        write_advisory(
            db,
            package,
            id,
            &advisory(id, package, extra, affected, versions),
        )?;
    }
    Ok(())
}

/// The fixture lock: the crates.io pins, a workspace member, and a git, an
/// alternate-registry and a path pin of advised names, and a sparse pin.
fn fixture_lock() -> Result<String, Failure> {
    let mut packages = vec![Package::local("subject", &[])];
    for (name, versions) in PINS {
        for version in versions {
            packages.push(Package::registry(name, version, &[]));
        }
    }
    let git = "git+https://github.com/example/time.git#0123456789abcdef0123456789abcdef01234567";
    packages.push(Package::from_source("time", "0.1.40", git, &[]));
    packages.push(Package::from_source(
        "gated",
        "1.1.0",
        SPARSE_CRATES_IO,
        &[],
    ));
    let other = "registry+https://example.invalid/index";
    packages.push(Package::from_source("windowed", "0.6.0", other, &[]));
    packages.push(Package::local("narrow", &[]));
    render(&packages)
}

/// The report's rows as a set of (id, package, version).
fn rows(report: &AdvisoryReport) -> BTreeSet<(String, String, String)> {
    report
        .matches()
        .iter()
        .map(|found| {
            (
                found.advisory_id().to_owned(),
                found.package().to_owned(),
                found.installed_version().to_owned(),
            )
        })
        .collect()
}

/// Commits everything under `db` as one commit dated 2026-11-01, with none of
/// the environment a hook sets that would point git at another repository.
fn commit_fixture(db: &Path) -> Result<(), Failure> {
    let steps: [&[&str]; 3] = [
        &["init", "--quiet"],
        &["add", "--all"],
        &["commit", "--quiet", "--message", "fixture"],
    ];
    for step in steps {
        let status = Command::new("git")
            .arg("-C")
            .arg(db)
            .args([
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(step)
            .env("GIT_AUTHOR_DATE", "2026-11-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-11-01T00:00:00Z")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
            .status()?;
        if !status.success() {
            return refuse(format!("git {step:?} exited {status}"));
        }
    }
    Ok(())
}

/// Catches the #942 fail-open and any drift from cargo-audit: the estate's rows
/// on the fixture are exactly cargo-audit's plus the two named extras, every
/// match carries its lockfile and `[affected]` platforms, and a non-crates.io
/// pin of an advised name is counted as skipped, never judged.
#[test]
fn sim_advisories_reports_every_row_cargo_audit_reports() -> Result<(), Failure> {
    let scratch = Scratch::new("advisories-differential")?;
    let db = scratch.path().join("advisory-db");
    write_fixture(&db)?;
    let lock = fixture_lock()?;
    let report = audit(&[Lockfile::new("subject/Cargo.lock", &lock)], &db)?;
    let reference: BTreeSet<(String, String, String)> = CARGO_AUDIT_ROWS
        .iter()
        .chain(&ESTATE_ONLY_ROWS)
        .map(|&(id, package, version)| (id.to_owned(), package.to_owned(), version.to_owned()))
        .collect();
    let found = rows(&report);
    if found != reference {
        let missing: Vec<_> = reference.difference(&found).collect();
        let extra: Vec<_> = found.difference(&reference).collect();
        return refuse(format!("missing {missing:?}, unexplained {extra:?}"));
    }
    if (report.packages_audited(), report.packages_skipped()) != (39, 4) {
        return refuse(format!(
            "audited {} and skipped {}, expected 39 and 4",
            report.packages_audited(),
            report.packages_skipped()
        ));
    }
    if report.database().advisories() != FIXTURE.len() || report.database().commit().is_some() {
        return refuse(format!("database identity {:?}", report.database()));
    }
    for found in report.matches() {
        let platforms = match found.package() {
            "narrow" => (vec!["windows"], vec!["x86_64"]),
            "time" => (
                vec![
                    "linux", "redox", "solaris", "android", "ios", "macos", "netbsd", "openbsd",
                    "freebsd",
                ],
                vec![],
            ),
            _ => (vec![], vec![]),
        };
        let (os, arch): (Vec<&str>, Vec<&str>) = (
            found.os().iter().map(String::as_str).collect(),
            found.arch().iter().map(String::as_str).collect(),
        );
        if (os, arch) != platforms || found.lockfile() != "subject/Cargo.lock" {
            return refuse(format!("{found:?} lost its platforms or its lockfile"));
        }
        let informational = matches!(found.package(), "abandoned" | "office" | "hexchat");
        if found.match_kind().is_vulnerability() == informational {
            return refuse(format!("{found:?} has the wrong kind"));
        }
        let title = format!("A flaw in {}", found.package());
        if found.title() != Some(title.as_str()) {
            return refuse(format!("{found:?} lost its title {title:?}"));
        }
    }
    let skipped: Vec<(&str, &str, &str, Option<&str>)> = report
        .skipped()
        .iter()
        .map(|pin| (pin.name(), pin.version(), pin.lockfile(), pin.source()))
        .collect();
    let git = "git+https://github.com/example/time.git#0123456789abcdef0123456789abcdef01234567";
    let lockfile = "subject/Cargo.lock";
    let expected = [
        ("narrow", "0.1.0", lockfile, None),
        ("subject", "0.1.0", lockfile, None),
        ("time", "0.1.40", lockfile, Some(git)),
        (
            "windowed",
            "0.6.0",
            lockfile,
            Some("registry+https://example.invalid/index"),
        ),
    ];
    if skipped != expected {
        return refuse(format!("skipped {skipped:?}, expected {expected:?}"));
    }
    Ok(())
}

/// The first lines an advisory body can open with, and the title each gives:
/// only an `# ` heading with text after it is a title (#955).
const BODIES: [(&str, Option<&str>); 9] = [
    ("# A flaw in pkg\n", Some("A flaw in pkg")),
    ("#   Spaced out   \n", Some("Spaced out")),
    (
        "# Ünïcödé — \"quoted\" `code`\n",
        Some("Ünïcödé — \"quoted\" `code`"),
    ),
    ("## A subheading first\n", None),
    ("#NoSpace\n", None),
    ("# \n", None),
    ("Prose before any heading\n\n# Late heading\n", None),
    ("", None),
    ("```toml\n[advisory]\n```\n", None),
];

/// Catches: a title read from a `title` key advisory-db never writes, so every
/// live match is untitled; a later heading or a subheading taken as the title;
/// and a CRLF or blank-line opening that hides the heading. A `title` key, which
/// a hand-written file may carry, wins over the heading.
#[test]
fn sim_advisories_title_is_the_first_heading() -> Result<(), Failure> {
    replay_sweep("advisory-titles", 64, |rng, trace| {
        let scratch = Scratch::new("advisories-titles")?;
        let db = scratch.path().join("advisory-db");
        let &(body, heading) = rng.pick_named("bodies", &BODIES)?;
        let keyed = rng.between(0, 1) == 1;
        let blanks: String = (0..rng.between(0, 3)).map(|_| '\n').collect();
        let key = if keyed { "title = \"Key title\"\n" } else { "" };
        let mut text = format!(
            "```toml\n[advisory]\nid = \"RUSTSEC-2099-0001\"\npackage = \"pkg\"\n{key}\n[versions]\npatched = [\">= 9.0.0\"]\n```\n{blanks}{body}"
        );
        let crlf = rng.between(0, 1) == 1;
        if crlf {
            text = text.replace('\n', "\r\n");
        }
        trace.record_number("keyed", u8::from(keyed));
        trace.record_number("crlf", u8::from(crlf));
        write_advisory(&db, "pkg", "RUSTSEC-2099-0001", &text)?;
        let lock = render(&[Package::registry("pkg", "1.0.0", &[])])?;
        let report = audit(&[Lockfile::new("Cargo.lock", &lock)], &db)?;
        let expected = keyed.then_some("Key title").or(heading);
        let [ref found] = *report.matches() else {
            return refuse(format!("{} matches, expected 1", report.matches().len()));
        };
        if found.title() != expected {
            return refuse(format!(
                "{body:?} keyed={keyed} crlf={crlf} titled {:?}, expected {expected:?}",
                found.title()
            ));
        }
        Ok(())
    })
}

/// Catches: a second lockfile's pins dropped or merged into the first's, and a
/// database identity that does not name the checkout or move with its content.
#[test]
fn sim_advisories_many_lockfiles_bind_to_one_named_checkout() -> Result<(), Failure> {
    let scratch = Scratch::new("advisories-identity")?;
    let db = scratch.path().join("advisory-db");
    write_fixture(&db)?;
    commit_fixture(&db)?;
    let lock = fixture_lock()?;
    let lockfiles = [
        Lockfile::new("a/Cargo.lock", &lock),
        Lockfile::new("b/Cargo.lock", &lock),
    ];
    let report = audit(&lockfiles, &db)?;
    let expected = CARGO_AUDIT_ROWS
        .len()
        .saturating_add(ESTATE_ONLY_ROWS.len());
    let per_lock = |label: &str| {
        report
            .matches()
            .iter()
            .filter(|found| found.lockfile() == label)
            .count()
    };
    if report.lockfiles() != 2
        || per_lock("a/Cargo.lock") != expected
        || per_lock("b/Cargo.lock") != expected
    {
        return refuse(format!(
            "{} lockfiles, {} matches",
            report.lockfiles(),
            report.matches().len()
        ));
    }
    let database = report.database();
    let commit = database.commit().ok_or("a checkout named no commit")?;
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return refuse(format!("HEAD read as {commit:?}"));
    }
    if database.date() != Some("2026-11-01") || !database.digest().starts_with("fnv1a128:") {
        return refuse(format!("identity {database:?}"));
    }
    let again = audit(&lockfiles, &db)?;
    if again.database() != database {
        return refuse("the same checkout named itself twice differently");
    }
    let Some(&(id, package, extra, affected, _)) = FIXTURE.first() else {
        return refuse("the fixture is empty");
    };
    write_advisory(
        &db,
        package,
        id,
        &advisory(id, package, extra, affected, "patched = [\">= 1.2.4\"]"),
    )?;
    if audit(&lockfiles, &db)?.database().digest() == database.digest() {
        return refuse("an edited advisory kept the digest");
    }
    Ok(())
}

/// The source spellings an advised pin can carry, and whether it is crates.io.
const SOURCES: [(Option<&str>, bool); 7] = [
    (Some(CRATES_IO), true),
    (Some(SPARSE_CRATES_IO), true),
    (Some("registry+https://example.invalid/index"), false),
    (Some("sparse+https://example.invalid/index/"), false),
    (
        Some("registry+https://github.com/rust-lang/crates.io-index.evil"),
        false,
    ),
    (
        Some("git+https://github.com/example/pkg.git#0123456789abcdef0123456789abcdef01234567"),
        false,
    ),
    (None, false),
];

/// Catches: a path, git or other-registry pin judged by a crates.io advisory
/// that shares its name, and a crates.io pin skipped for its sparse spelling.
#[test]
fn sim_advisories_only_registry_sources_match() -> Result<(), Failure> {
    replay_sweep("advisory-sources", 32, |rng, trace| {
        let scratch = Scratch::new("advisories-sources")?;
        let db = scratch.path().join("advisory-db");
        let count = rng.between(1, 12);
        let (mut packages, mut matched) = (Vec::new(), 0_usize);
        for index in 0..count {
            let name = format!("pkg{index}");
            let id = format!("RUSTSEC-2099-{index:04}");
            write_advisory(
                &db,
                &name,
                &id,
                &advisory(&id, &name, "", "", "patched = [\">= 9.0.0\"]"),
            )?;
            let &(source, is_crates_io) = rng.pick_named("sources", &SOURCES)?;
            let version = format!("1.0.{}", rng.between(0, 9));
            packages.push(match source {
                Some(source) => Package::from_source(&name, &version, source, &[]),
                None => Package::local(&name, &[]),
            });
            if is_crates_io {
                matched = matched.saturating_add(1);
            }
            trace.record_number("crates-io", u8::from(is_crates_io));
        }
        let lock = render(&packages)?;
        let report = audit(&[Lockfile::new("Cargo.lock", &lock)], &db)?;
        let skipped = packages.len().saturating_sub(matched);
        if report.matches().len() != matched
            || report.packages_skipped() != skipped
            || report.skipped().len() != skipped
        {
            return refuse(format!(
                "{} matches and {} skipped, expected {matched} and {skipped}",
                report.matches().len(),
                report.packages_skipped()
            ));
        }
        let names: Vec<&str> = report.skipped().iter().map(|pin| pin.name()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        if names != sorted
            || report
                .matches()
                .iter()
                .any(|found| names.contains(&found.package()))
        {
            return refuse(format!("skipped {names:?} is unsorted or holds a match"));
        }
        Ok(())
    })
}

/// The most pins one audit judges, skipped and audited together, over every
/// lockfile: `advisories::db::MAX_PACKAGES`.
const MAX_PINS: usize = 65_536;

/// A lock of `count` workspace pins named from `first` on.
fn local_lock(first: usize, count: usize) -> Result<String, Failure> {
    let packages: Vec<Package> = (first..first.saturating_add(count))
        .map(|index| Package::local(&format!("member{index}"), &[]))
        .collect();
    render(&packages)
}

/// Catches: the package bound off by one, or counted per lockfile, or counting
/// only the audited pins. Exactly the bound across two lockfiles of skipped
/// pins is judged and every pin named; one more is the typed bound refusal.
#[test]
fn sim_advisories_package_bound_holds_skipped_pins_across_lockfiles() -> Result<(), Failure> {
    let scratch = Scratch::new("advisories-package-bound")?;
    let db = scratch.path().join("advisory-db");
    write_fixture(&db)?;
    let half = MAX_PINS >> 1_u32;
    let (first, second) = (local_lock(0, half)?, local_lock(half, half)?);
    let at_bound = [
        Lockfile::new("a/Cargo.lock", &first),
        Lockfile::new("b/Cargo.lock", &second),
    ];
    let report = audit(&at_bound, &db)?;
    if report.skipped().len() != MAX_PINS || report.packages_audited() != 0 {
        return refuse(format!(
            "{} skipped and {} audited at the bound",
            report.skipped().len(),
            report.packages_audited()
        ));
    }
    let past = local_lock(half, half.saturating_add(1))?;
    let past_bound = [
        Lockfile::new("a/Cargo.lock", &first),
        Lockfile::new("b/Cargo.lock", &past),
    ];
    match audit(&past_bound, &db) {
        Err(AdvisoryError::Bound {
            limit: MAX_PINS, ..
        }) => Ok(()),
        other => refuse(format!("one pin past the bound gave {other:?}")),
    }
}

/// Catches: an empty or misnamed database read as a clean audit. A database is
/// unmeasured unless it holds an advisory under `crates/*/` or `rust/*/`;
/// dotfiles, the template, non-Markdown names and directories are skipped
/// without moving the digest.
#[test]
fn sim_advisories_empty_or_misnamed_database_is_unmeasured() -> Result<(), Failure> {
    let scratch = Scratch::new("advisories-layout")?;
    let db = scratch.path().join("advisory-db");
    let lock = render(&[Package::registry("std", "1.0.0", &[])])?;
    let lockfiles = [Lockfile::new("Cargo.lock", &lock)];
    let empty = |db: &Path| {
        matches!(
            audit(&lockfiles, db),
            Err(AdvisoryError::EmptyDatabase { .. })
        )
    };
    std::fs::create_dir_all(&db)?;
    if !empty(&db) {
        return refuse("an empty directory audited clean");
    }
    let holder = db.join("crates").join("std");
    std::fs::create_dir_all(holder.join("RUSTSEC-2099-0002.md"))?;
    std::fs::create_dir_all(db.join("crates").join(".git"))?;
    std::fs::write(
        db.join("crates").join(".git").join("RUSTSEC-2099-0003.md"),
        "x",
    )?;
    std::fs::write(holder.join(".RUSTSEC-2099-0004.md"), "x")?;
    std::fs::write(holder.join("RUSTSEC-0000-0000.md"), "x")?;
    std::fs::write(holder.join("README.txt"), "x")?;
    std::fs::write(db.join("crates").join("RUSTSEC-2099-0005.md"), "x")?;
    if !empty(&db) {
        return refuse("a database of skipped names audited clean");
    }
    if !matches!(
        audit(&lockfiles, &scratch.path().join("absent")),
        Err(AdvisoryError::Io { .. })
    ) {
        return refuse("an absent database was not an I/O refusal");
    }
    let rust = db.join("rust").join("std");
    std::fs::create_dir_all(&rust)?;
    let text = advisory(
        "RUSTSEC-2099-0001",
        "std",
        "",
        "",
        "patched = [\">= 2.0.0\"]",
    );
    std::fs::write(rust.join("RUSTSEC-2099-0001.md"), &text)?;
    let report = audit(&lockfiles, &db)?;
    if report.database().advisories() != 1 {
        return refuse(format!(
            "loaded {} advisories, expected 1",
            report.database().advisories()
        ));
    }
    let lone = Scratch::new("advisories-layout-lone")?;
    let lone_rust = lone.path().join("rust").join("std");
    std::fs::create_dir_all(&lone_rust)?;
    std::fs::write(lone_rust.join("RUSTSEC-2099-0001.md"), &text)?;
    if audit(&lockfiles, lone.path())?.database().digest() != report.database().digest() {
        return refuse("a skipped name moved the digest");
    }
    Ok(())
}

/// Catches: a requirement RustSec refuses to load, or an input past a bound,
/// read as anything but its own typed refusal.
#[test]
fn sim_advisories_unloadable_requirements_and_bounds_are_typed_refusals() -> Result<(), Failure> {
    let scratch = Scratch::new("advisories-refusals")?;
    let lock = render(&[Package::registry("vuln", "1.0.0", &[])])?;
    let lockfiles = [Lockfile::new("Cargo.lock", &lock)];
    let shapes = [
        "1.x",
        ">= 1.0.0, >= 2.0.0",
        ">= 1.0.0, < 2.0.0, < 3.0.0",
        "^1.0.0, < 2.0.0",
    ];
    for (index, shape) in shapes.iter().enumerate() {
        let db = scratch.path().join(format!("db{index}"));
        let versions = format!("patched = [\"{shape}\"]");
        write_advisory(
            &db,
            "vuln",
            "RUSTSEC-2099-0001",
            &advisory("RUSTSEC-2099-0001", "vuln", "", "", &versions),
        )?;
        match audit(&lockfiles, &db) {
            Err(AdvisoryError::UnsupportedRequirement { ref text, .. }) if text == shape => {}
            other => return refuse(format!("{shape:?} gave {other:?}")),
        }
    }
    let db = scratch.path().join("oversized");
    let padding = "x".repeat((1 << 20) + 1);
    write_advisory(&db, "vuln", "RUSTSEC-2099-0001", &padding)?;
    if !matches!(
        audit(&lockfiles, &db),
        Err(AdvisoryError::Bound {
            limit: 1_048_576,
            ..
        })
    ) {
        return refuse("an advisory past 1 MiB was read");
    }
    let many = vec![Lockfile::new("Cargo.lock", &lock); 65];
    if !matches!(
        audit(&many, &db),
        Err(AdvisoryError::Bound { limit: 64, .. })
    ) {
        return refuse("65 lockfiles were read");
    }
    Ok(())
}
