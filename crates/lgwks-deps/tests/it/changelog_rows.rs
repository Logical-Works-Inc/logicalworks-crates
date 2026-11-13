//! Holds `CHANGELOG.md` to the manifests (`docs/releasing.md` §3).
//!
//! A release commit moves the versions, and its changelog row is typed by hand
//! beside them. The README table has had a test since the 2026-11-08 audit; the
//! changelog had none, so a cut that forgot its row, or named a version the
//! manifest does not carry, still passed the suite (release of 2026-11-13).
//! The governance ledger's newest part is named the same way, in two indexes.

use std::error::Error;
use std::fs;

use super::agent_start_page::{CRATES, identity, page};

type TestResult = Result<(), Box<dyn Error>>;

/// A crate name and the version a row or a manifest gives it.
type Release = (String, String);

/// One release row of `CHANGELOG.md`.
struct Row {
    /// The crates the row names, each with its version.
    released: Vec<Release>,
    /// The row's notes page, as written.
    link: Option<String>,
}

/// One release row, such as
/// `- lgwks_std 4.1.2 / lgwks_deps 7.1.0 - 2026-11-13: [notes](docs/changelog/x.md)`.
fn release_row(row: &str) -> Option<Row> {
    let body = row.strip_prefix("- ")?;
    let (released, rest) = body.split_once(" - ")?;
    let released = released
        .split(" / ")
        .filter_map(|pair| pair.split_once(' '))
        .map(|(name, version)| (name.to_owned(), version.to_owned()))
        .collect();
    let link = rest
        .split_once("](")
        .and_then(|(_, target)| target.split_once(')'))
        .map(|(target, _)| target.to_owned());
    Some(Row { released, link })
}

/// The rows under `## Releases, newest first`, newest first.
fn release_rows(changelog: &str) -> Vec<&str> {
    changelog
        .lines()
        .skip_while(|line| *line != "## Releases, newest first")
        .skip(1)
        .filter(|line| line.starts_with("- "))
        .collect()
}

/// Every version a manifest carries was released under a changelog row.
#[test]
fn every_manifest_version_has_a_changelog_row() -> TestResult {
    let changelog = page("CHANGELOG.md")?;
    let released: Vec<Release> = release_rows(&changelog)
        .into_iter()
        .filter_map(release_row)
        .flat_map(|row| row.released)
        .collect();
    for directory in CRATES {
        let manifest = identity(directory)?;
        assert!(
            released.contains(&manifest),
            "CHANGELOG.md has no release row naming {} {}",
            manifest.0,
            manifest.1
        );
    }
    Ok(())
}

/// The newest row names only manifest versions and links a page that exists.
#[test]
fn the_newest_changelog_row_is_the_manifests() -> TestResult {
    let changelog = page("CHANGELOG.md")?;
    let newest = release_rows(&changelog)
        .first()
        .copied()
        .ok_or("CHANGELOG.md has no release row")?;
    let row = release_row(newest).ok_or("the newest row is not a release row")?;
    assert!(
        !row.released.is_empty(),
        "the newest row names no crate: {newest}"
    );
    let manifests = CRATES
        .iter()
        .map(|directory| identity(directory))
        .collect::<Result<Vec<Release>, _>>()?;
    for release in &row.released {
        assert!(
            manifests.contains(release),
            "the newest row names {} {}, which no manifest carries",
            release.0,
            release.1
        );
    }
    let link = row.link.ok_or("the newest row links no notes page")?;
    assert!(
        page(&link).is_ok(),
        "the newest row links {link}, which is missing"
    );
    Ok(())
}

/// The row reader takes a row apart as a reader does, and refuses prose.
#[test]
fn a_release_row_reads_as_its_pairs_and_its_link() -> TestResult {
    let line = "- lgwks_std 4.1.2 / lgwks_deps 7.1.0 - 2026-11-13: [notes](docs/changelog/19.md)";
    let row = release_row(line).ok_or("a release row was not read")?;
    assert_eq!(
        row.released,
        [
            ("lgwks_std".to_owned(), "4.1.2".to_owned()),
            ("lgwks_deps".to_owned(), "7.1.0".to_owned())
        ]
    );
    assert_eq!(row.link.as_deref(), Some("docs/changelog/19.md"));
    assert!(release_row("All notable changes are recorded here.").is_none());
    let rows =
        release_rows("# Changelog\n\n- not a release\n## Releases, newest first\n\n- a 1 - d\n");
    assert_eq!(rows, ["- a 1 - d"]);
    Ok(())
}

/// The newest `docs/governance/ledger-<date>.md` part, by its dated name.
fn newest_ledger_part() -> Result<String, Box<dyn Error>> {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/governance");
    let mut newest: Option<String> = None;
    for entry in fs::read_dir(directory)? {
        let raw = entry?.file_name();
        let name = raw
            .to_str()
            .ok_or_else(|| format!("ledger name {raw:?} is not UTF-8"))?
            .to_owned();
        let is_part = name.starts_with("ledger-") && name.ends_with(".md");
        if is_part && newest.as_ref().is_none_or(|held| name > *held) {
            newest = Some(name);
        }
    }
    newest.ok_or_else(|| "docs/governance holds no ledger part".into())
}

/// A ledger entry goes at the top of the newest part, so both indexes must name
/// it first: `GOVERNANCE.md` §5 and the one "newest part" row of `docs/README.md`.
#[test]
fn both_indexes_name_the_newest_ledger_part_first() -> TestResult {
    let newest = newest_ledger_part()?;
    let link = format!("](docs/governance/{newest})");
    let governance = page("GOVERNANCE.md")?;
    let section = governance
        .lines()
        .find(|line| line.starts_with("| 5 |"))
        .ok_or("GOVERNANCE.md has no §5 row")?;
    let first = section
        .find("](docs/governance/ledger-")
        .ok_or("the §5 row links no ledger part")?;
    assert_eq!(
        section.get(first..first + link.len()),
        Some(link.as_str()),
        "GOVERNANCE.md §5 does not list {newest} first"
    );
    let readme = page("docs/README.md")?;
    let marked: Vec<&str> = readme
        .lines()
        .filter(|line| line.contains("the newest part"))
        .collect();
    assert_eq!(
        marked.len(),
        1,
        "docs/README.md marks {} newest parts",
        marked.len()
    );
    assert!(
        marked
            .iter()
            .all(|line| line.contains(&format!("(governance/{newest})"))),
        "docs/README.md marks another part than {newest} as the newest"
    );
    Ok(())
}
