//! The manifest-edge checks: the gate's own dependencies stay optional storefront
//! edges, and lgwks_std declares only its reviewed leaves.

use super::*;

/// Every line of a manifest's `[dependencies]` table, up to the next table.
///
/// One reader for both manifest checks: they differ in which manifest they
/// read and what they assert, and two copies of the same six-line walk are
/// two places for a table header to be read wrongly.
fn dependency_declarations<'a>(
    manifest: &'a str,
    crate_label: &str,
) -> Result<Vec<&'a str>, Box<dyn std::error::Error>> {
    let after = manifest
        .split("[dependencies]")
        .nth(1)
        .ok_or_else(|| format!("{crate_label} declares no [dependencies]"))?;
    Ok(after
        .lines()
        .map(str::trim)
        .take_while(|line| !line.starts_with('['))
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect())
}

/// The package name a `key = "value"` dependency line declares.
fn declared_name(line: &str) -> &str {
    match line.split_once('=') {
        Some((name, _)) => name.trim(),
        None => line.trim(),
    }
}
/// INV-STDPLUS-APPROVED-ONLY: lgwks-std may depend on vetted leaf crates
/// whose transitive trees bottom out at zero external deps. The gate itself
/// stays zero-dep (self-refuting otherwise). This test enforces both.
#[test]
fn deps_are_approved_leaves() -> TestResult {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("crates/lgwks-deps has no parent")?
        .parent()
        .ok_or("crates/ has no parent")?;

    // The deps storefront may depend only on the workspace facade it enforces,
    // plus optional storefront features that the end user selects; the Rust
    // source detectors live in `lgwks_ast` (ADR 0002), so no parser is here.
    // The default build stays zero-dependency:
    // every third-party edge other than lgwks_std must be `optional = true`,
    // so `cargo build`/`cargo install` with no features pulls nothing.
    {
        let manifest = std::fs::read_to_string(workspace.join("crates/lgwks-deps/Cargo.toml"))?;
        let declared = dependency_declarations(&manifest, "gate")?;
        let names: Vec<&str> = declared.iter().copied().map(declared_name).collect();
        assert_eq!(
            names,
            [
                "lgwks_std",
                "proc-macro2",
                "quote",
                "axum",
                "tower-http",
                "llm-guard",
                "gpui",
                "appcui",
                "bevy_ecs",
                "bevy_app",
                "bevy_time",
                "bevy_state",
                "tokio",
                "candle-core",
                "candle-nn",
                "candle-transformers",
                "tokenizers",
                "jiff",
                "rusqlite",
                "chacha20poly1305",
                "prettyplease",
                "syn",
                "lgwks_ast",
                "rustls",
                "reqwest",
                "rmcp",
                "genai",
                "jsonschema",
                "oauth2",
                "coset",
                "ct-merkle",
                "opentelemetry",
                "opentelemetry_sdk",
                "opentelemetry-otlp",
                "tracing-opentelemetry",
                "tracing-subscriber",
            ],
            "unexpected gate dependencies: {declared:?}"
        );
        for line in &declared {
            if !line.starts_with("lgwks_std") {
                assert!(
                    line.contains("optional = true"),
                    "storefront dependency must stay optional so the default build is \
                     zero-dependency: {line}"
                );
            }
        }
        assert!(declared[0].starts_with("lgwks_std ="));
    }

    // lgwks-std may only declare deps on this approved list.
    {
        const APPROVED: &[&str] = &[
            "blake3",
            "ed25519-dalek",
            "getrandom",
            "iri-string",
            "regex",
            "rkyv",
            "ron",
            "rustix",
            "serde",
            "serde_json",
            "tracing",
            "tracing-subscriber",
            "ureq",
        ];

        let manifest = std::fs::read_to_string(workspace.join("crates/lgwks-std/Cargo.toml"))?;
        let declared = dependency_declarations(&manifest, "std")?;
        for line in &declared {
            let name = declared_name(line);
            assert!(
                APPROVED.contains(&name),
                "lgwks-std declares unapproved dependency `{name}` — \
                 add it to APPROVED in this test after review"
            );
        }
    }
    Ok(())
}
