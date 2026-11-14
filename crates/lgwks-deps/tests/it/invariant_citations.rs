//! Every invariant id the tree cites is one a register defines.
//!
//! #911 item 11 found `clippy.toml` and the std-first scan citing ids no
//! register held: a reader sent to the rule found nothing, so the citation
//! claimed an enforcement that did not exist (SD-7). An id is defined as
//! `**INV-…**` in Markdown, as `id = "INV-…"` in `contract/INVARIANTS.toml`, or
//! as `INV-…:` in a comment that states the rule beside its code. A quoted id
//! is a test's fixture value and a `INV-…-*` one a family, so neither is a
//! citation. History (the changelog, the ledger and the ADRs) records ids as
//! they were, so it is not read.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

type TestResult = Result<(), Box<dyn Error>>;

/// Directories that are build output, other people's code, or dated history.
const NOT_READ: [&str; 8] = [
    "target",
    ".git",
    "vendor",
    "node_modules",
    "changelog",
    "governance",
    "adr",
    "fixtures",
];

/// The file kinds a citation is written in.
const READ_KINDS: [&str; 4] = ["rs", "toml", "md", "sh"];

/// Every file of a read kind under `dir`.
fn files_under(dir: &Path, out: &mut Vec<PathBuf>) -> TestResult {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let text = name.to_str().ok_or("a path that is not UTF-8")?;
        if entry.file_type()?.is_dir() {
            if !NOT_READ.contains(&text) {
                files_under(&path, out)?;
            }
        } else if path
            .extension()
            .and_then(|kind| kind.to_str())
            .is_some_and(|kind| READ_KINDS.contains(&kind))
        {
            out.push(path);
        }
    }
    Ok(())
}

/// Each `INV-…` id in `text`, with whether it is written as a definition.
fn ids(text: &str) -> Vec<(String, bool)> {
    let mut found = Vec::new();
    for (at, _) in text.match_indices("INV-") {
        let (Some(before), Some(after)) = (text.get(..at), text.get(at..)) else {
            continue;
        };
        let spelled: String = after
            .chars()
            .take_while(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || *ch == '-')
            .collect();
        let id = spelled.trim_end_matches('-');
        let Some(tail) = after.get(id.len()..) else {
            continue;
        };
        let Some(line) = before.rsplit('\n').next() else {
            continue;
        };
        let line = line.trim_start();
        let in_comment = line.starts_with("//") || line.starts_with('#');
        let defined = (before.ends_with("**") && tail.starts_with("**"))
            || (before.ends_with("id = \"") && tail.starts_with('"'))
            || (in_comment && tail.starts_with(':'));
        let fixture = !defined && before.ends_with('"');
        let family = tail.starts_with("-*");
        if id.len() > "INV-".len() && !id.ends_with("-N") && !fixture && !family {
            found.push((id.to_owned(), defined));
        }
    }
    found
}

/// The repository root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn every_cited_invariant_id_is_defined_by_a_register() -> TestResult {
    let mut files = Vec::new();
    files_under(&root(), &mut files)?;
    let mut defined = BTreeSet::new();
    let mut cited = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file)?;
        for (id, is_definition) in ids(&text) {
            if is_definition {
                defined.insert(id);
            } else {
                cited.push((id, file.clone()));
            }
        }
    }
    assert!(
        defined.len() > 100,
        "read {} definitions; the reader is broken",
        defined.len()
    );
    let undefined: BTreeSet<String> = cited
        .iter()
        .filter(|cite| !defined.contains(&cite.0))
        .map(|cite| format!("{} in {}", cite.0, cite.1.display()))
        .collect();
    assert!(
        undefined.is_empty(),
        "{} citations name an id no register defines:\n{}",
        undefined.len(),
        undefined.into_iter().collect::<Vec<_>>().join("\n")
    );
    Ok(())
}

#[test]
fn the_id_reader_tells_a_definition_from_a_citation() {
    assert_eq!(
        ids("- **INV-STD-HTTP-5** A rule. See INV-RANDOM-1, and `id = \"INV-DEP-5\"`."),
        [
            ("INV-STD-HTTP-5".to_owned(), true),
            ("INV-RANDOM-1".to_owned(), false),
            ("INV-DEP-5".to_owned(), true),
        ]
    );
    assert_eq!(ids("INV- and INV-BOT-N are no ids"), []);
    assert_eq!(
        ids("//! `hex` enforces INV-HEX-1: a rule.\n/// see INV-HEX-1 here"),
        [
            ("INV-HEX-1".to_owned(), true),
            ("INV-HEX-1".to_owned(), false),
        ]
    );
    assert_eq!(
        ids("let name = \"INV-FIXTURE-1\"; // the INV-RANDOM-* rows"),
        [],
        "a fixture value and a family are not citations"
    );
}
