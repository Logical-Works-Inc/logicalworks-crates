//! Holds `CHANGELOG.md` to the manifests (`docs/releasing.md` §3).
//!
//! A release commit moves the versions, and its changelog row is typed by hand
//! beside them. The README table has had a test since the 2026-11-08 audit; the
//! changelog had none, so a cut that forgot its row, or named a version the
//! manifest does not carry, still passed the suite (release of 2026-11-13).

use std::error::Error;

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
