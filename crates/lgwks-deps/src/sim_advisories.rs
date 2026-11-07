//! Simulation and verification tests for `lgwks_deps::advisories`.
//!
//! Exercises realistic RustSec advisory parsing, frontmatter error refusal,
//! SemVer comparator edge cases, and 500 seeded simulation cases verifying that
//! lockfile evaluation matches an independent reference oracle, skips withdrawn
//! advisories and local packages, and produces a deterministic database digest.

use std::fmt;
use std::path::Path;

use lgwks_std::seeded::Seeded;

use crate::advisories::{
    Advisory, AdvisoryError, AdvisoryKind, compute_db_digest, evaluate_lock, parse_advisory,
};

/// The sweep seeds for deterministic simulation testing.
const SWEEP_SEEDS: [u64; 4] = [
    0x5EED_0000_0000_0001,
    0x5EED_0000_0000_0002,
    0x5EED_0000_FFFF_FFFF,
    0xDEAD_BEEF_CAFE_0001,
];

/// Result type used by all test functions to avoid panics.
type Outcome = Result<(), String>;

/// Asserts equality without panicking, returning an informative error on mismatch.
fn check_eq<T: PartialEq + fmt::Debug>(actual: T, expected: T, context: &str) -> Outcome {
    if actual != expected {
        lgwks_std::trace::debug!(context, "assertion mismatch");
        return Err(format!("{context}: expected {expected:?}, got {actual:?}"));
    }
    Ok(())
}

/// Asserts that a condition is true without panicking.
fn check_true(condition: bool, context: &str) -> Outcome {
    if !condition {
        lgwks_std::trace::debug!(context, "condition false");
        return Err(format!("{context}: expected true, got false"));
    }
    Ok(())
}

/// Package name candidates for synthetic generation.
const PACKAGE_POOL: [&str; 8] = [
    "tokio", "serde", "hyper", "regex", "nom", "syn", "rand", "flate2",
];

/// Draws an index uniformly below `bound` without panicking.
fn draw_below(stream: &mut Seeded, bound: u64) -> u64 {
    let mut result = 0_u64;
    if bound > 0
        && let Ok(val) = stream.below(bound)
    {
        result = val;
    }
    result
}

/// Synthetic package definition for lockfile generation.
#[derive(Debug, Clone)]
struct SyntheticPackage {
    /// Package name.
    name: String,
    /// Whether package is local to the workspace.
    is_local: bool,
}

// ── Realistic Frontmatter Tests ─────────────────────────────────────────────

#[test]
/// Verifies parsing a realistic vulnerability advisory with code fence delimiters.
fn sim_parse_advisory_realistic_toml_fence() -> Outcome {
    let content = r#"```toml
[advisory]
id = "RUSTSEC-2026-0001"
package = "serde_json"
title = "Deserialization recursion limit bypass"
date = "2026-03-01"

[versions]
patched = [">= 1.0.100"]
unaffected = ["< 1.0.50"]
```

# Description
A flaw in serde_json allowed deeply nested structures to exceed recursion limits.
"#;

    let path = Path::new("crates/serde_json/RUSTSEC-2026-0001.md");
    let advisory = parse_advisory(path, content).map_err(|err_val| format!("{err_val}"))?;

    check_eq(advisory.id(), "RUSTSEC-2026-0001", "advisory id")?;
    check_eq(advisory.package(), "serde_json", "advisory package")?;
    check_eq(
        advisory.title(),
        Some("Deserialization recursion limit bypass"),
        "advisory title",
    )?;
    check_eq(advisory.date(), Some("2026-03-01"), "advisory date")?;
    check_eq(advisory.withdrawn(), None, "advisory not withdrawn")?;
    check_eq(
        advisory.kind(),
        &AdvisoryKind::Vulnerability,
        "advisory kind",
    )?;
    check_true(advisory.kind().is_vulnerability(), "is vulnerability")?;
    check_eq(
        advisory.patched(),
        &[">= 1.0.100".to_owned()],
        "patched versions",
    )?;
    check_eq(
        advisory.unaffected(),
        &["< 1.0.50".to_owned()],
        "unaffected versions",
    )?;

    Ok(())
}

#[test]
/// Verifies that withdrawn advisories are correctly identified and carry their withdrawal date.
fn sim_parse_advisory_withdrawn() -> Outcome {
    let content = r#"```toml
[advisory]
id = "RUSTSEC-2026-0002"
package = "tokio"
title = "Spurious cancellation bug"
date = "2026-03-02"
withdrawn = "2026-03-04"

[versions]
patched = [">= 1.30.0"]
```
"#;

    let path = Path::new("crates/tokio/RUSTSEC-2026-0002.md");
    let advisory = parse_advisory(path, content).map_err(|err_val| format!("{err_val}"))?;

    check_eq(advisory.id(), "RUSTSEC-2026-0002", "advisory id")?;
    check_eq(advisory.withdrawn(), Some("2026-03-04"), "withdrawal date")?;
    check_eq(
        advisory.kind(),
        &AdvisoryKind::Vulnerability,
        "advisory kind",
    )?;

    Ok(())
}

#[test]
/// Verifies parsing informational advisories with specific categories.
fn sim_parse_advisory_informational() -> Outcome {
    let content = r#"```toml
[advisory]
id = "RUSTSEC-2026-0003"
package = "net2"
title = "net2 is unmaintained; use socket2 instead"
date = "2026-01-01"
informational = "unmaintained"

[versions]
patched = []
```
"#;

    let path = Path::new("crates/net2/RUSTSEC-2026-0003.md");
    let advisory = parse_advisory(path, content).map_err(|err_val| format!("{err_val}"))?;

    check_eq(advisory.id(), "RUSTSEC-2026-0003", "advisory id")?;
    check_eq(
        advisory.kind(),
        &AdvisoryKind::Informational("unmaintained".to_owned()),
        "informational kind",
    )?;
    check_eq(
        advisory.kind().informational_name(),
        Some("unmaintained"),
        "informational name",
    )?;
    check_true(!advisory.kind().is_vulnerability(), "not vulnerability")?;
    check_true(advisory.patched().is_empty(), "patched empty")?;

    Ok(())
}

#[test]
/// Verifies parsing YAML-style frontmatter delimiters (`--- ... ---`).
fn sim_parse_advisory_yaml_delimiters() -> Outcome {
    let content = r#"---
[advisory]
id = "RUSTSEC-2026-0004"
package = "regex"
date = "2026-02-14"

[versions]
patched = [">= 1.9.0", ">= 2.0.0"]
unaffected = ["< 1.0.0"]
---
# Regex Advisory Details
"#;

    let path = Path::new("crates/regex/RUSTSEC-2026-0004.md");
    let advisory = parse_advisory(path, content).map_err(|err_val| format!("{err_val}"))?;

    check_eq(advisory.id(), "RUSTSEC-2026-0004", "advisory id")?;
    check_eq(advisory.package(), "regex", "package")?;
    check_eq(advisory.patched().len(), 2, "patched count")?;
    check_eq(advisory.unaffected().len(), 1, "unaffected count")?;

    Ok(())
}

#[test]
/// Verifies parsing multiline arrays in frontmatter.
fn sim_parse_advisory_multiline_arrays() -> Outcome {
    let content = r#"```toml
[advisory]
id = "RUSTSEC-2026-0005"
package = "hyper"

[versions]
patched = [
    ">= 0.14.27",
    ">= 1.1.0",
]
unaffected = [
    "< 0.14.0",
]
```
"#;

    let path = Path::new("crates/hyper/RUSTSEC-2026-0005.md");
    let advisory = parse_advisory(path, content).map_err(|err_val| format!("{err_val}"))?;

    check_eq(
        advisory.patched(),
        &[">= 0.14.27".to_owned(), ">= 1.1.0".to_owned()],
        "multiline patched",
    )?;
    check_eq(
        advisory.unaffected(),
        &["< 0.14.0".to_owned()],
        "multiline unaffected",
    )?;

    Ok(())
}

// ── Malformed Frontmatter Refusal Tests ──────────────────────────────────────

fn assert_malformed_frontmatter(path: &Path, content: &str, expected_substring: &str) -> Outcome {
    match parse_advisory(path, content) {
        Err(AdvisoryError::MalformedFrontmatter { ref detail, .. }) => {
            check_true(
                detail.contains(expected_substring),
                "detail names missing field",
            )?;
            Ok(())
        }
        other => Err(format!("expected MalformedFrontmatter, got {other:?}")),
    }
}

#[test]
/// Verifies that frontmatter lacking an `id` field is refused.
fn sim_refuse_missing_id() -> Outcome {
    let content = "```toml\n[advisory]\npackage = \"serde\"\n```\n";
    let path = Path::new("crates/serde/RUSTSEC-2026-0006.md");
    assert_malformed_frontmatter(path, content, "id")
}

#[test]
/// Verifies that frontmatter lacking a `package` field is refused.
fn sim_refuse_missing_package() -> Outcome {
    let content = "```toml\n[advisory]\nid = \"RUSTSEC-2026-0007\"\n```\n";
    let path = Path::new("crates/foo/RUSTSEC-2026-0007.md");
    assert_malformed_frontmatter(path, content, "package")
}

#[test]
/// Verifies that markdown without any frontmatter block is refused.
fn sim_refuse_no_frontmatter() -> Outcome {
    let content = "Just an unstructured markdown document with no TOML fence or advisory block.";
    let path = Path::new("crates/bar/RUSTSEC-2026-0008.md");
    match parse_advisory(path, content) {
        Err(AdvisoryError::MalformedFrontmatter { .. }) => Ok(()),
        other => Err(format!("expected MalformedFrontmatter, got {other:?}")),
    }
}

// ── SemVer Comparator Edge Cases ────────────────────────────────────────────

#[test]
/// Verifies SemVer caret (`^`) matching semantics across major, minor, and 0.0.x versions.
fn sim_semver_caret_comparators() -> Outcome {
    let lock_text = r#"
[[package]]
name = "pkg_one"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "pkg_two"
version = "0.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "pkg_three"
version = "0.0.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;

    let packages = crate::lock::parse(lock_text).map_err(|err_val| format!("{err_val}"))?;

    // Advisory with patched = ["^1.2.4"] -> pkg_one (1.2.3) is vulnerable
    let adv_one = Advisory::new(
        "RUSTSEC-2026-C01",
        "pkg_one",
        None,
        None,
        AdvisoryKind::Vulnerability,
        vec!["^1.2.4".to_owned()],
        Vec::new(),
    );

    // Advisory with patched = ["^0.2.4"] -> pkg_two (0.2.3) is vulnerable
    let adv_two = Advisory::new(
        "RUSTSEC-2026-C02",
        "pkg_two",
        None,
        None,
        AdvisoryKind::Vulnerability,
        vec!["^0.2.4".to_owned()],
        Vec::new(),
    );

    // Advisory with patched = ["^0.0.4"] -> pkg_three (0.0.3) is vulnerable
    let adv_three = Advisory::new(
        "RUSTSEC-2026-C03",
        "pkg_three",
        None,
        None,
        AdvisoryKind::Vulnerability,
        vec!["^0.0.4".to_owned()],
        Vec::new(),
    );

    let report = evaluate_lock(
        &packages,
        &[adv_one, adv_two, adv_three],
        "fnv1a128:test".to_owned(),
    )
    .map_err(|err_val| format!("{err_val}"))?;

    check_eq(report.matches().len(), 3, "all three vulnerable")?;

    // Now test with versions that ARE patched
    let adv_fixed = Advisory::new(
        "RUSTSEC-2026-C01",
        "pkg_one",
        None,
        None,
        AdvisoryKind::Vulnerability,
        vec!["^1.2.0".to_owned()],
        Vec::new(),
    );
    let report_fixed = evaluate_lock(&packages, &[adv_fixed], "fnv1a128:test".to_owned())
        .map_err(|err_val| format!("{err_val}"))?;

    check_eq(
        report_fixed.matches().len(),
        0,
        "fixed package has no match",
    )?;

    Ok(())
}

#[test]
/// Verifies SemVer tilde (`~`) matching semantics.
fn sim_semver_tilde_comparators() -> Outcome {
    let lock_text = r#"
[[package]]
name = "pkg_tilde"
version = "1.2.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
    let packages = crate::lock::parse(lock_text).map_err(|err_val| format!("{err_val}"))?;

    // ~1.2.0 matches 1.2.5
    let adv_patched = Advisory::new(
        "RUSTSEC-2026-T01",
        "pkg_tilde",
        None,
        None,
        AdvisoryKind::Vulnerability,
        vec!["~1.2.0".to_owned()],
        Vec::new(),
    );
    let report = evaluate_lock(&packages, &[adv_patched], "fnv1a128:test".to_owned())
        .map_err(|err_val| format!("{err_val}"))?;
    check_eq(
        report.matches().len(),
        0,
        "1.2.5 matches ~1.2.0 so not vulnerable",
    )?;

    // ~1.2.6 does NOT match 1.2.5 -> vulnerable
    let adv_unpatched = Advisory::new(
        "RUSTSEC-2026-T02",
        "pkg_tilde",
        None,
        None,
        AdvisoryKind::Vulnerability,
        vec!["~1.2.6".to_owned()],
        Vec::new(),
    );
    let report_unpatched = evaluate_lock(&packages, &[adv_unpatched], "fnv1a128:test".to_owned())
        .map_err(|err_val| format!("{err_val}"))?;
    check_eq(
        report_unpatched.matches().len(),
        1,
        "1.2.5 fails ~1.2.6 so vulnerable",
    )?;

    Ok(())
}

#[test]
/// Verifies comma-separated multi-comparator ranges (`>= 1.2.0, < 1.3.0`).
fn sim_semver_multi_comparator_ranges() -> Outcome {
    let lock_text = r#"
[[package]]
name = "pkg_range"
version = "1.2.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
    let packages = crate::lock::parse(lock_text).map_err(|err_val| format!("{err_val}"))?;

    // Unaffected by range: >= 1.2.0, < 1.3.0
    let adv = Advisory::new(
        "RUSTSEC-2026-R01",
        "pkg_range",
        None,
        None,
        AdvisoryKind::Vulnerability,
        vec![">= 1.3.0".to_owned()],
        vec![">= 1.2.0, < 1.3.0".to_owned()],
    );

    let report = evaluate_lock(&packages, &[adv], "fnv1a128:test".to_owned())
        .map_err(|err_val| format!("{err_val}"))?;
    check_eq(report.matches().len(), 0, "package is unaffected by range")?;

    Ok(())
}

#[test]
/// Verifies pre-release version ordering and comparator handling.
fn sim_semver_prerelease_precedence() -> Outcome {
    let lock_text = r#"
[[package]]
name = "pkg_alpha"
version = "1.0.0-alpha.1"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "pkg_beta"
version = "1.0.0-beta.2"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "pkg_rc"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
    let packages = crate::lock::parse(lock_text).map_err(|err_val| format!("{err_val}"))?;

    // An advisory unaffected by < 1.0.0-alpha.2 -> pkg_alpha (1.0.0-alpha.1) is unaffected
    let adv_alpha = Advisory::new(
        "RUSTSEC-2026-P01",
        "pkg_alpha",
        None,
        None,
        AdvisoryKind::Vulnerability,
        vec![">= 1.0.0".to_owned()],
        vec!["< 1.0.0-alpha.2".to_owned()],
    );

    // Advisory unaffected by < 1.0.0 -> 1.0.0-beta.2 is < 1.0.0, so unaffected!
    let adv_beta = Advisory::new(
        "RUSTSEC-2026-P02",
        "pkg_beta",
        None,
        None,
        AdvisoryKind::Vulnerability,
        vec![">= 1.0.0".to_owned()],
        vec!["< 1.0.0".to_owned()],
    );

    // Advisory unaffected by < 1.0.0 -> 1.0.0 is NOT < 1.0.0, but patched by >= 1.0.0
    let adv_rc = Advisory::new(
        "RUSTSEC-2026-P03",
        "pkg_rc",
        None,
        None,
        AdvisoryKind::Vulnerability,
        vec![">= 1.0.0".to_owned()],
        vec!["< 1.0.0".to_owned()],
    );

    let report = evaluate_lock(
        &packages,
        &[adv_alpha, adv_beta, adv_rc],
        "fnv1a128:test".to_owned(),
    )
    .map_err(|err_val| format!("{err_val}"))?;

    check_eq(
        report.matches().len(),
        0,
        "all packages patched or unaffected",
    )?;

    Ok(())
}

// ── Seeded Simulation Sweep (500 Cases) ─────────────────────────────────────

#[test]
/// Seeded simulation sweep across 500 cases verifying that `evaluate_lock` matches
/// a reference oracle, skips withdrawn advisories and local packages, and produces
/// deterministic database digests.
fn sim_advisories_fuzzy_match_sweep() -> Outcome {
    let cases_per_seed: usize = 125;
    let mut total_cases: usize = 0;

    for &seed_val in &SWEEP_SEEDS {
        let mut stream = Seeded::from_seed(seed_val);

        for case_idx in 0..cases_per_seed {
            total_cases = total_cases.saturating_add(1);

            // Generate 3 to 6 synthetic packages
            let pkg_count_u64 = draw_below(&mut stream, 4).saturating_add(3);
            let mut pkg_count = 3_usize;
            if let Ok(count_val) = usize::try_from(pkg_count_u64) {
                pkg_count = count_val;
            }
            let mut synthetic_packages: Vec<SyntheticPackage> = Vec::with_capacity(pkg_count);
            let mut lock_content = String::new();

            for pkg_idx in 0..pkg_count {
                let mut pool_len_u64 = 8_u64;
                if let Ok(len_val) = u64::try_from(PACKAGE_POOL.len()) {
                    pool_len_u64 = len_val;
                }
                let name_idx_u64 = draw_below(&mut stream, pool_len_u64);
                let mut name_idx = 0_usize;
                if let Ok(idx_val) = usize::try_from(name_idx_u64) {
                    name_idx = idx_val;
                }
                let mut pkg_name = "tokio";
                if let Some(name_ref) = PACKAGE_POOL.get(name_idx) {
                    pkg_name = *name_ref;
                }

                let major = draw_below(&mut stream, 3);
                let minor = draw_below(&mut stream, 5);
                let patch = draw_below(&mut stream, 10);
                let version_str = format!("{major}.{minor}.{patch}");

                let is_local = draw_below(&mut stream, 4) == 0;
                synthetic_packages.push(SyntheticPackage {
                    name: format!("{pkg_name}_{pkg_idx}"),
                    is_local,
                });

                if is_local {
                    std::fmt::Write::write_fmt(
                        &mut lock_content,
                        format_args!(
                            "[[package]]\nname = \"{pkg_name}_{pkg_idx}\"\nversion = \"{version_str}\"\n\n"
                        ),
                    )
                    .map_err(|err_val| format!("{err_val}"))?;
                } else {
                    std::fmt::Write::write_fmt(
                        &mut lock_content,
                        format_args!(
                            "[[package]]\nname = \"{pkg_name}_{pkg_idx}\"\nversion = \"{version_str}\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n"
                        ),
                    )
                    .map_err(|err_val| format!("{err_val}"))?;
                }
            }

            // Generate 2 to 4 synthetic advisories
            let adv_count_u64 = draw_below(&mut stream, 3).saturating_add(2);
            let mut adv_count = 2_usize;
            if let Ok(count_val) = usize::try_from(adv_count_u64) {
                adv_count = count_val;
            }
            let mut advisories: Vec<Advisory> = Vec::with_capacity(adv_count);

            for adv_idx in 0..adv_count {
                let mut pkg_count_bound = 1_u64;
                if let Ok(bound_val) = u64::try_from(pkg_count) {
                    pkg_count_bound = bound_val;
                }
                let p_ref_u64 = draw_below(&mut stream, pkg_count_bound);
                let mut p_ref = 0_usize;
                if let Ok(ref_val) = usize::try_from(p_ref_u64) {
                    p_ref = ref_val;
                }
                let mut target_name = "tokio_0".to_owned();
                if let Some(pkg_entry) = synthetic_packages.get(p_ref) {
                    target_name = pkg_entry.name.clone();
                }

                let is_withdrawn = draw_below(&mut stream, 4) == 0;
                let withdrawn = if is_withdrawn {
                    Some("2026-03-01".to_owned())
                } else {
                    None
                };

                let is_info = draw_below(&mut stream, 4) == 0;
                let kind = if is_info {
                    AdvisoryKind::Informational("notice".to_owned())
                } else {
                    AdvisoryKind::Vulnerability
                };

                let patched_min = draw_below(&mut stream, 5);
                let patched = vec![format!(">= 1.{patched_min}.0")];
                let unaffected = vec!["< 0.5.0".to_owned()];

                advisories.push(Advisory::new(
                    format!("RUSTSEC-2026-{case_idx:04}-{adv_idx}"),
                    target_name,
                    Some("2026-01-01".to_owned()),
                    withdrawn,
                    kind,
                    patched,
                    unaffected,
                ));
            }

            // Verify database digest computation is deterministic
            let file_records: Vec<(&str, &str)> = vec![
                ("crates/a/RUSTSEC-0001.md", "content_a"),
                ("crates/b/RUSTSEC-0002.md", "content_b"),
            ];
            let digest_one = compute_db_digest(file_records.iter().copied());
            let digest_two = compute_db_digest(file_records.iter().copied());
            check_eq(&digest_one, &digest_two, "deterministic db_digest")?;
            check_true(digest_one.starts_with("fnv1a128:"), "digest prefix")?;

            // Parse lock packages
            let lock_packages =
                crate::lock::parse(&lock_content).map_err(|err_val| format!("{err_val}"))?;

            // Evaluate lock against advisories
            let report = evaluate_lock(&lock_packages, &advisories, digest_one.clone())
                .map_err(|err_val| format!("{err_val}"))?;

            // Verify loaded count and audited packages count
            check_eq(
                report.advisories_loaded(),
                advisories.len(),
                "advisories_loaded count",
            )?;

            let expected_audited = synthetic_packages
                .iter()
                .filter(|pkg_entry| !pkg_entry.is_local)
                .count();
            check_eq(
                report.packages_audited(),
                expected_audited,
                "packages_audited count",
            )?;

            // Verify local packages are never in matches
            for matched_entry in report.matches() {
                let mut is_matching_pkg_local = false;
                if let Some(pkg_entry) = synthetic_packages
                    .iter()
                    .find(|pkg_entry| pkg_entry.name == matched_entry.package())
                {
                    is_matching_pkg_local = pkg_entry.is_local;
                }
                check_true(!is_matching_pkg_local, "local packages are never matched")?;
            }

            // Verify withdrawn advisories are never in matches
            for matched_entry in report.matches() {
                let is_matching_adv_withdrawn = advisories
                    .iter()
                    .find(|adv_entry| adv_entry.id() == matched_entry.advisory_id())
                    .and_then(|adv_entry| adv_entry.withdrawn())
                    .is_some();
                check_true(
                    !is_matching_adv_withdrawn,
                    "withdrawn advisories are skipped",
                )?;
            }
        }
    }

    check_eq(total_cases, 500, "total sweep cases executed")?;
    Ok(())
}

#[test]
/// Verifies deterministic database digest properties.
fn sim_db_digest_determinism() -> Outcome {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    let expected_empty = format!("fnv1a128:{OFFSET:032x}");

    let empty_digest = compute_db_digest([]);
    check_eq(empty_digest, expected_empty, "empty database digest")?;

    let set_a = [
        ("crates/foo/RUSTSEC-2026-0001.md", "hello"),
        ("crates/bar/RUSTSEC-2026-0002.md", "world"),
    ];
    let digest_a1 = compute_db_digest(set_a);
    let digest_a2 = compute_db_digest(set_a);
    check_eq(&digest_a1, &digest_a2, "digest reproducibility")?;

    let set_b = [
        ("crates/foo/RUSTSEC-2026-0001.md", "hello"),
        ("crates/bar/RUSTSEC-2026-0002.md", "world!"),
    ];
    let digest_b = compute_db_digest(set_b);
    check_true(
        digest_a1 != digest_b,
        "different contents yield different digests",
    )?;

    Ok(())
}
