//! Shared fixtures for the seeded dependency-policy simulations.
//!
//! Included (via `#[path]`) by `sim_dependency_policy.rs` and
//! `sim_policy_properties.rs`, so the edge/register builders, the verdict model
//! and the generator are written once rather than copied between suites.

use crate::sim;

use std::error::Error;

use lgwks_deps::contract::Contract;
use lgwks_deps::metadata::{self, DirectEdge};
use lgwks_deps::{Refusal, audit_direct};

pub(crate) use sim::Rng;

/// The registry source the fixture approvals and edges name.
pub(crate) const REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// What every test in the including suites returns.
pub(crate) type TestResult = Result<(), Box<dyn Error>>;

/// An `aliases = "…"` line for an approval, or empty when there is no alias.
pub(crate) fn alias_line(alias: Option<&str>) -> String {
    match alias {
        Some(value) => format!("aliases = \"{value}\"\n"),
        None => String::new(),
    }
}

/// A JSON string literal for `value`, or JSON `null` when the dimension is absent.
fn json_text_or_null(value: Option<&str>) -> String {
    match value {
        Some(text) => format!("\"{text}\""),
        None => "null".to_owned(),
    }
}

/// One edge with every authored dimension spelled out, through the parser.
pub(crate) fn edge(
    package: &str,
    source: Option<&str>,
    features: &[&str],
    uses_default_features: bool,
    optional: bool,
    target: Option<&str>,
    rename: Option<&str>,
) -> Result<DirectEdge, Box<dyn Error>> {
    let source = json_text_or_null(source);
    let features = features
        .iter()
        .map(|feature| format!("\"{feature}\""))
        .collect::<Vec<_>>()
        .join(",");
    let target = json_text_or_null(target);
    let rename = json_text_or_null(rename);
    let document = format!(
        r#"{{"packages":[{{"id":"app","name":"app","repository":null,"manifest_path":"/repo/Cargo.toml","dependencies":[{{"name":"{package}","source":{source},"req":"1.0","kind":null,"rename":{rename},"optional":{optional},"uses_default_features":{uses_default_features},"features":[{features}],"target":{target},"path":null}}]}},{{"id":"lic-{package}","name":"{package}","license":"MIT OR Apache-2.0","manifest_path":"/dep/Cargo.toml","dependencies":[]}}],"workspace_members":["app"]}}"#
    );
    let mut edges = metadata::parse(&document)?;
    edges
        .pop()
        .ok_or_else(|| "the fixture edge must parse".into())
}

/// A register with one approval for `package` owned by `app`, carrying the given
/// extra lines (an alias and any policy keys).
pub(crate) fn register(
    package: &str,
    source: &str,
    extra: &str,
) -> Result<Contract, Box<dyn Error>> {
    let text = format!(
        concat!(
            "[policy]\nschema = 2\nenforce = true\naccepted_licenses = \"MIT, Apache-2.0\"\n\n",
            "[[approved]]\n",
            "crate = \"{package}\"\ntier = \"boundary\"\nversion = \"1.0\"\nowner = \"app\"\n",
            "capability = \"engine.core\"\nlicense = \"MIT OR Apache-2.0\"\nsource = \"{source}\"\n{extra}",
            "allowed_consumers = \"app\"\nallowed_kinds = \"normal\"\n",
            "reason = \"The engine supplies a capability the standard library cannot express.\"\n",
            "approved_by = \"reviewer\"\napproved_on = \"2026-09-30\"\nreview = \"tests/support/deps_sim.rs\"\n",
        ),
        package = package,
        source = source,
        extra = extra,
    );
    Ok(Contract::parse(&text)?)
}

/// 0 admitted, 1 any refusal.
fn verdict(refusals: &[Refusal]) -> u8 {
    u8::from(!refusals.is_empty())
}

/// Audit one edge and return its verdict code.
pub(crate) fn code_for(register: &Contract, edge: DirectEdge) -> u8 {
    verdict(&audit_direct(&[edge], register))
}
