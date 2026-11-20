//! Structural checks for the documentation bundle, not frontend runtime tests.

use std::collections::HashSet;
use std::path::{Component, Path};

const REQUIREMENTS: &str = include_str!("../contract/requirements.tsv");
const JOURNEYS: &str = include_str!("../experience/journeys.tsv");
const DOCUMENTS: &[&str] = &[
    "README.md",
    "VISION.md",
    "docs/architecture.md",
    "docs/architecture-operations.md",
    "docs/codebase.md",
    "docs/planned-tree.md",
    "docs/decisions.md",
    "docs/contract.md",
    "docs/research.md",
    "docs/assurance.md",
    "docs/status.md",
    "docs/status-history.md",
    "docs/inklab.md",
    "docs/ink-engine.md",
    "docs/gpu-runtime.md",
    "docs/native-host.md",
];

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Why a registry was refused.
#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    /// The first line is not the registry's header.
    Header,
    /// A row with a missing, empty or extra field.
    Field,
    /// A malformed or repeated identity.
    Identity,
    /// A row whose status claims execution the documents cannot prove.
    Execution,
    /// A registry with no rows.
    Empty,
    /// A requirement whose journey is not in the journey registry.
    Unlinked,
}

impl std::fmt::Display for Refusal {
    /// The reason, in words.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match *self {
            Self::Header => "unexpected header",
            Self::Field => "missing or extra field",
            Self::Identity => "invalid or duplicate identity",
            Self::Execution => "documentation cannot assert product execution",
            Self::Empty => "empty registry",
            Self::Unlinked => "a requirement names a journey the registry lacks",
        })
    }
}

impl std::error::Error for Refusal {}

/// The cell at `index`, or the field refusal for a row too short to hold it.
fn cell<'a>(row: &[&'a str], index: usize) -> Result<&'a str, Refusal> {
    row.get(index).copied().ok_or(Refusal::Field)
}

fn rows<'a>(text: &'a str, header: &str, prefix: char) -> Result<Vec<Vec<&'a str>>, Refusal> {
    let mut lines = text.lines();
    let first = lines.next();
    if first != Some(header) {
        lgwks_std::trace::debug!(?first, header, "registry refused: unexpected header");
        return Err(Refusal::Header);
    }
    let width = header.split('\t').count();
    let mut ids = HashSet::new();
    let mut result = Vec::new();
    for line in lines {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != width || fields.iter().any(|field| field.trim().is_empty()) {
            lgwks_std::trace::debug!(line, width, "registry refused: missing or extra field");
            return Err(Refusal::Field);
        }
        let id = cell(&fields, 0)?;
        if id.len() != 3
            || !id
                .strip_prefix(prefix)
                .is_some_and(|digits| digits.bytes().all(|byte| byte.is_ascii_digit()))
            || !ids.insert(id)
        {
            lgwks_std::trace::debug!(id, %prefix, "registry refused: invalid or duplicate identity");
            return Err(Refusal::Identity);
        }
        if fields.last() != Some(&"unmeasured") {
            lgwks_std::trace::debug!(id, "registry refused: a row claims execution");
            return Err(Refusal::Execution);
        }
        result.push(fields);
    }
    if result.is_empty() {
        lgwks_std::trace::debug!(header, "registry refused: no rows");
        return Err(Refusal::Empty);
    }
    Ok(result)
}

const REQUIREMENT_HEADER: &str =
    "id\tobligation\tsource\tdesign\towner\tplanned_test\tjourney\tstatus";
const JOURNEY_HEADER: &str =
    "id\trequirements\tactor\tjob\tsetup\tstimulus\texpected\tforbidden\toracle\tstatus";

/// The validated requirement registry, or the reason it is invalid.
fn requirements() -> Result<Vec<Vec<&'static str>>, Refusal> {
    rows(REQUIREMENTS, REQUIREMENT_HEADER, 'R')
}

/// The validated journey registry, or the reason it is invalid.
fn journeys() -> Result<Vec<Vec<&'static str>>, Refusal> {
    rows(JOURNEYS, JOURNEY_HEADER, 'J')
}

#[test]
fn registry_inventory_is_explicit() -> TestResult {
    assert_eq!(requirements()?.len(), 37);
    assert_eq!(journeys()?.len(), 33);
    Ok(())
}

#[test]
fn finalized_contract_retains_answer_provenance_and_proof_boundary() {
    // The decision record and the finalized contract it continues into, read
    // as one text so every id and phrase is still counted exactly once.
    let decisions = concat!(
        include_str!("../docs/decisions.md"),
        "\n",
        include_str!("../docs/contract.md")
    );
    for (prefix, count, padded) in [("Q", 40, true), ("B1.", 8, false), ("B1B.", 6, false)] {
        let ids: Vec<_> = decisions
            .lines()
            .filter_map(|line| line.strip_prefix("| "))
            .filter_map(|line| line.split(" | ").next())
            .filter(|id| id.starts_with(prefix))
            .collect();
        assert_eq!(ids.len(), count, "answer inventory for {prefix}");
        for number in 1..=count {
            let id = if padded {
                format!("{prefix}{number:02}")
            } else {
                format!("{prefix}{number}")
            };
            assert_eq!(ids.iter().filter(|entry| **entry == id).count(), 1);
        }
    }
    assert!(decisions.contains("finalized requirements baseline, runtime admission blocked"));
    assert!(
        decisions.contains("Derived engineering constraints — not additional Director answers")
    );
    assert!(decisions.contains("line 33,221"));
    assert!(decisions.contains("A question or challenge is not consent"));
}

#[test]
fn requirement_and_journey_links_are_bidirectional() -> TestResult {
    let requirements = requirements()?;
    let journeys = journeys()?;
    for requirement in &requirements {
        let wanted = cell(requirement, 6)?;
        let journey = journeys
            .iter()
            .find(|row| row.first() == Some(&wanted))
            .ok_or(Refusal::Unlinked)?;
        let requirement_id = cell(requirement, 0)?;
        let journey_id = cell(journey, 0)?;
        assert!(
            cell(journey, 1)?.split(',').any(|id| id == requirement_id),
            "journey {journey_id} does not list requirement {requirement_id}"
        );
    }
    for journey in &journeys {
        let journey_id = cell(journey, 0)?;
        for id in cell(journey, 1)?.split(',') {
            assert!(
                requirements.iter().any(|row| row.first() == Some(&id)),
                "journey {journey_id} names unknown requirement {id}"
            );
        }
    }
    Ok(())
}

#[test]
fn design_references_exist_and_tests_are_explicitly_planned() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let research = include_str!("../docs/research.md");
    for row in requirements()? {
        let design = cell(&row, 3)?;
        let planned = cell(&row, 5)?;
        assert!(DOCUMENTS.contains(&design), "unknown design {design}");
        assert!(root.join(design).is_file());
        assert!(research.contains(&format!("| {} |", cell(&row, 2)?)));
        let path = Path::new(planned);
        assert!(
            path.components()
                .all(|part| matches!(part, Component::Normal(_)))
        );
        assert!(planned.starts_with("tests/"));
        assert!(planned.ends_with(".rs"));
        assert!(
            !root.join(path).exists(),
            "planned test is now present: update status"
        );
    }
    Ok(())
}

#[test]
fn all_narrative_documents_are_in_the_publication() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for path in DOCUMENTS {
        let text = std::fs::read_to_string(root.join(path))?;
        assert!(text.starts_with("# "), "missing title in {path}");
        assert!(text.len() > 500, "empty or placeholder document: {path}");
    }
    let library = include_str!("../src/lib.rs");
    for path in DOCUMENTS.iter().skip(1) {
        assert!(library.contains(path), "not embedded in Rustdoc: {path}");
    }
    // The package must declare what it actually is: the design publication
    // plus the reference core, with the realized modules present. This
    // replaces the older "documentation-only" assertion, which the Director's
    // instruction to implement the ink lab and GPU runtime superseded.
    let package = include_str!("../Cargo.toml").to_ascii_lowercase();
    assert!(
        package.contains("design publication"),
        "undeclared design publication"
    );
    assert!(
        package.contains("reference core"),
        "undeclared reference core"
    );
    for module in [
        "pub mod design;",
        "pub mod ink_lab;",
        "pub mod runtime;",
        "pub mod canvas_protocol;",
        "pub mod canvas_scene;",
        "pub mod cli;",
        "pub mod motion;",
        "pub mod native_host;",
        "pub mod preview;",
    ] {
        assert!(library.contains(module), "missing realized module {module}");
    }
    Ok(())
}

#[test]
fn decision_ids_are_unique_and_the_adr_cites_live_ones() -> TestResult {
    let decisions = include_str!("../docs/decisions.md");
    let ids: Vec<&str> = decisions
        .lines()
        .filter_map(|line| line.strip_prefix("| D"))
        .filter_map(|line| line.split(" | ").next())
        .collect();
    let unique: HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(ids.len(), unique.len(), "a decision id is repeated");
    for number in 1..=19 {
        let id = format!("{number:02}");
        assert!(ids.contains(&id.as_str()), "decision D{id} is missing");
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let adr =
        std::fs::read_to_string(root.join("docs/adr/0001-scope-audit-and-bound-the-broker.md"))?;
    assert!(adr.starts_with("# "), "the ADR has no title");
    assert!(
        adr.contains("D18 and D19"),
        "the ADR must cite the renumbered ledger entries"
    );
    assert!(
        !adr.contains("D16 and D17"),
        "the ADR cites the private repository's ids"
    );
    Ok(())
}

#[test]
fn validator_rejects_malformed_and_false_success_registries() -> TestResult {
    let header = "id\tmeaning\tstatus";
    for (invalid, why) in [
        ("wrong\nR01\tmeaning\tunmeasured\n", Refusal::Header),
        ("id\tmeaning\tstatus\n", Refusal::Empty),
        ("id\tmeaning\tstatus\nR01\t\tunmeasured\n", Refusal::Field),
        (
            "id\tmeaning\tstatus\nR01\tmeaning\tunmeasured\textra\n",
            Refusal::Field,
        ),
        (
            "id\tmeaning\tstatus\nR01\tmeaning\tpassed\n",
            Refusal::Execution,
        ),
        (
            "id\tmeaning\tstatus\nJ01\tmeaning\tunmeasured\n",
            Refusal::Identity,
        ),
        (
            "id\tmeaning\tstatus\nRxx\tmeaning\tunmeasured\n",
            Refusal::Identity,
        ),
        (
            "id\tmeaning\tstatus\nR01\tmeaning\tunmeasured\nR01\tagain\tunmeasured\n",
            Refusal::Identity,
        ),
    ] {
        assert_eq!(rows(invalid, header, 'R'), Err(why), "{invalid:?}");
    }
    let parsed = rows(
        "id\tmeaning\tstatus\nR01\tmeaning\tunmeasured\n",
        header,
        'R',
    )?;
    assert_eq!(parsed, vec![vec!["R01", "meaning", "unmeasured"]]);
    Ok(())
}
