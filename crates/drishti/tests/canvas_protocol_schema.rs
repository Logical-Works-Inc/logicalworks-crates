//! Cross-runtime schema lock: Rust enum order matches JSON schema order.
//!
//! Counterpart of `kernel/crates/canvas-protocol/tests/schema_lock.rs`
//! (kernel#850 row 15). A set-equal-but-reordered enum is a cross-runtime
//! break, so this test prints both sequences with the first divergence index.
//!
//! Scope: `Capability` has a 1:1 schema (`Capability.json`, locked below).
//! `ActorType`, `TrustBin`, `LocalActionStatus`, `SyncStatus`,
//! `CompensationStrategy`, and `OfflineBehavior` are Rust-only with no schema
//! files; that absence is intentional, not an oversight.

use std::path::{Path, PathBuf};

/// Locate the absorbed schema tree regardless of the caller's cwd.
fn schema_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets/canvas-protocol/schema")
        .join(relative)
}

/// Parse a JSON-Schema enum file and return its `enum` array as strings.
fn schema_enum_values(path: &Path) -> Result<Vec<String>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read schema {}: {error}", path.display()))?;
    let doc: lgwks_std::json::Value = lgwks_std::json::from_str(&raw)
        .map_err(|error| format!("cannot parse schema {}: {error}", path.display()))?;
    let array = doc
        .get("enum")
        .ok_or_else(|| format!("schema {} has no 'enum' key", path.display()))?
        .as_array()
        .ok_or_else(|| format!("schema {} 'enum' is not an array", path.display()))?;
    let mut values = Vec::with_capacity(array.len());
    for value in array {
        let entry = value
            .as_str()
            .ok_or_else(|| format!("schema {} 'enum' contains non-string", path.display()))?;
        values.push(entry.to_owned());
    }
    Ok(values)
}

/// Format two sequences with a marker at the first divergence.
fn divergence_report(schema: &[String], rust: &[String]) -> Result<String, String> {
    use std::fmt::Write as _;
    let longer = schema.len().max(rust.len());
    let mut out = format!(
        "Schema ({} entries) vs Rust ({} entries):\n",
        schema.len(),
        rust.len()
    );
    for index in 0..longer {
        // The `Some` arms compute (method call), so this is not an
        // `unwrap_or` shape; the `None` arms mark ragged tails.
        let schema_value = match schema.get(index) {
            Some(entry) => entry.as_str(),
            None => "(missing)",
        };
        let rust_value = match rust.get(index) {
            Some(entry) => entry.as_str(),
            None => "(missing)",
        };
        let marker = if schema_value == rust_value {
            ""
        } else {
            "<-- FIRST DIVERGENCE"
        };
        writeln!(
            out,
            "  [{index:2}] schema={schema_value:?} rust={rust_value:?} {marker}"
        )
        .map_err(|error| format!("report failed: {error}"))?;
    }
    Ok(out)
}

/// The capability lock: schema order and Rust declaration order agree.
#[test]
fn capability_enum_matches_schema_in_order() -> Result<(), String> {
    let path = schema_path("Capability.json");
    let schema_vals = schema_enum_values(&path)?;
    let rust_vals: Vec<String> = drishti::canvas_protocol::Capability::all()
        .into_iter()
        .map(|capability| capability.to_string())
        .collect();
    if schema_vals != rust_vals {
        return Err(format!(
            "Capability schema/Rust drift detected!\n{}",
            divergence_report(&schema_vals, &rust_vals)?
        ));
    }
    Ok(())
}
