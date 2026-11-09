//! The requirement check's own tests.

use super::{
    MAX_PAGES, digest_of, execute, heading_id, is_log_heading, log_ids, parse_lock,
    parse_requirements, quote_body,
};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

type TestResult = Result<(), Box<dyn Error>>;

/// A fresh tree holding `spine` as `REQUIREMENTS.md`, named for one test.
fn spine_tree(test: &str, spine: &str) -> Result<PathBuf, Box<dyn Error>> {
    let nonce = u64::from_le_bytes(lgwks_std::random::bytes::<8>()?);
    let root = std::env::temp_dir().join(format!("lwc-requirements-{nonce:016x}-{test}"));
    fs::create_dir_all(root.join("docs/requirements"))?;
    fs::create_dir_all(root.join("scripts"))?;
    fs::write(root.join("REQUIREMENTS.md"), spine)?;
    Ok(root)
}

/// A check of `root`: the exit code and what it wrote to stderr.
fn run_check(root: &Path, update: bool) -> Result<(u8, String), Box<dyn Error>> {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = execute(root, update, &mut out, &mut err)?;
    Ok((code, String::from_utf8_lossy(&err).into_owned()))
}

/// ADR 0023: the 120-line law split the spine, so a sentence on a page is held
/// exactly as one in the file, in name order and whatever else the folder holds.
#[test]
fn split_pages_read_as_one_spine() -> TestResult {
    let root = spine_tree(
        "split",
        "### R1. One\n\n> first\n\n## Supersession log\n\n- R9 moved\n\n## Relationship\n",
    )?;
    fs::write(
        root.join("docs/requirements/b.md"),
        "### R3. Three\n\n> third\n",
    )?;
    fs::write(
        root.join("docs/requirements/a.md"),
        "### R2. Two\n\n> second\n",
    )?;
    fs::write(
        root.join("docs/requirements/note.txt"),
        "### R8. No\n\n> not a page\n",
    )?;
    assert_eq!(
        run_check(&root, true)?.0,
        0,
        "the first update records the spine"
    );
    let lock = fs::read_to_string(root.join("scripts/requirements.lock"))?;
    let ids: Vec<&str> = lock
        .lines()
        .filter(|line| line.starts_with('R'))
        .filter_map(|line| line.split('\t').next())
        .collect();
    assert_eq!(ids, ["R1", "R2", "R3"], "three pages, no text file");
    assert_eq!(run_check(&root, false)?.0, 0, "the lock verifies");
    fs::write(
        root.join("docs/requirements/a.md"),
        "### R2. Two\n\n> edited\n",
    )?;
    let (code, err) = run_check(&root, false)?;
    assert_eq!(code, 1, "an edit on a page is an edit of the spine");
    assert!(err.contains("R2: normative sentence changed"), "{err}");
    assert!(err.contains("no Supersession log entry: R2"), "{err}");
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// The hole a split opens: a second copy of an id on a later page would win the
/// map, so the first copy could be edited with no change to what the lock reads.
#[test]
fn a_requirement_declared_twice_is_refused() -> TestResult {
    let same_page = parse_requirements("### R1. A\n\n> a\n\n### R1. B\n\n> b\n");
    assert!(same_page.is_err(), "twice on one page");
    let root = spine_tree("twice", "### R1. One\n\n> first\n")?;
    fs::write(
        root.join("docs/requirements/a.md"),
        "### R1. Copy\n\n> first\n",
    )?;
    let message = run_check(&root, true).err().map(|error| error.to_string());
    assert!(
        message
            .as_deref()
            .is_some_and(|text| text.contains("R1 is declared twice")),
        "{message:?}"
    );
    assert!(
        !root.join("scripts/requirements.lock").exists(),
        "a refused spine records nothing"
    );
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// The page ceiling holds at its bound and refuses one past it.
#[test]
fn the_page_ceiling_is_exact() -> TestResult {
    let root = spine_tree("ceiling", "### R1. One\n\n> first\n")?;
    for page in 0..MAX_PAGES {
        fs::write(root.join(format!("docs/requirements/{page}.md")), "\n")?;
    }
    assert_eq!(
        run_check(&root, true)?.0,
        0,
        "the ceiling itself is admitted"
    );
    fs::write(root.join("docs/requirements/past.md"), "\n")?;
    let message = run_check(&root, false).err().map(|error| error.to_string());
    assert!(
        message
            .as_deref()
            .is_some_and(|text| text.contains("past the ceiling")),
        "{message:?}"
    );
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// A repository keeping every requirement in one file reads as it always did.
#[test]
fn a_tree_with_no_pages_folder_reads_the_one_file() -> TestResult {
    let root = spine_tree("flat", "### R1. One\n\n> first\n")?;
    fs::remove_dir_all(root.join("docs/requirements"))?;
    assert_eq!(run_check(&root, true)?.0, 0, "the update reads one file");
    assert_eq!(run_check(&root, false)?.0, 0, "the check reads one file");
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// FIPS 180-4 / RFC 6234 test vectors: the empty message, "abc", and the
/// 56-byte message that forces a second block.
#[test]
fn the_digest_matches_the_published_sha256_vectors() -> TestResult {
    assert_eq!(
        digest_of("")?,
        "e3b0c44298fc",
        "SHA-256 of the empty message"
    );
    assert_eq!(digest_of("abc")?, "ba7816bf8f01", "SHA-256 of abc");
    assert_eq!(
        digest_of("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")?,
        "248d6a61d206",
        "SHA-256 of the two-block message"
    );
    Ok(())
}

#[test]
fn a_heading_names_its_id_and_a_plain_line_names_none() {
    assert_eq!(
        heading_id("### R12. Human mess is in scope"),
        Some(12),
        "a heading"
    );
    assert_eq!(heading_id("### R12 no dot"), None, "no dot");
    assert_eq!(heading_id("## R12. wrong level"), None, "wrong level");
    assert_eq!(heading_id("### R. no id"), None, "no digits");
}

#[test]
fn a_quote_loses_its_marker_and_one_space() {
    assert_eq!(quote_body("> text"), Some("text"), "marker and space");
    assert_eq!(quote_body(">text"), Some("text"), "marker only");
    assert_eq!(quote_body("  >  two"), Some(" two"), "one space only");
    assert_eq!(quote_body("text"), None, "no marker");
}

#[test]
fn the_log_heading_and_its_ids_are_found() {
    assert!(is_log_heading("## Supersession log"), "the heading");
    assert!(!is_log_heading("### Supersession log"), "a deeper heading");
    assert_eq!(
        log_ids("R3 and R41, not XR5 or R6x"),
        vec![3, 41],
        "standalone ids"
    );
}

#[test]
fn requirements_collapse_a_wrapped_quote_and_record_the_log() -> TestResult {
    let text = "### R1. One\n\n> first\n> second\n\n### R2. Two\n> only\n\n## Supersession log\nR2 moved\n";
    let parsed = parse_requirements(text)?;
    assert_eq!(
        parsed.sentences.get(&1).map(String::as_str),
        Some("first second"),
        "a wrapped quote collapses"
    );
    assert_eq!(
        parsed.sentences.get(&2).map(String::as_str),
        Some("only"),
        "a one-line quote"
    );
    assert!(parsed.logged.contains(&2), "the log names R2");
    Ok(())
}

#[test]
fn a_heading_without_a_quote_is_refused() {
    assert!(
        parse_requirements("### R1. One\n\ntext\n").is_err(),
        "a requirement with no blockquote is not written yet"
    );
}

#[test]
fn a_lock_row_needs_three_tab_separated_fields() -> TestResult {
    let rows = parse_lock("# header\nR7\tabc\tsentence\n")?;
    assert_eq!(
        rows.get(&7).map(|row| row.digest.as_str()),
        Some("abc"),
        "the digest is read"
    );
    assert!(
        parse_lock("R7 abc sentence\n").is_err(),
        "spaces are not tabs"
    );
    assert!(
        parse_lock("X7\tabc\tsentence\n").is_err(),
        "the id must start with R"
    );
    Ok(())
}

/// The committed lock verifies against the committed `REQUIREMENTS.md`: the
/// digests the Python script recorded are the digests this binary computes.
#[test]
fn the_committed_lock_verifies() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = execute(&root, false, &mut out, &mut err)?;
    assert_eq!(
        code,
        0,
        "the committed lock must verify: {}",
        String::from_utf8_lossy(&err)
    );
    Ok(())
}
