//! Flow validation: the structural and declaration checks a document passes at load. Split from `session.rs` (#526).
//!
//! The whole-document checks live here; the per-node checks (`nodes`) and the
//! graph analyses (`analysis`) are the two children.

use super::{
    BTreeMap, BTreeSet, CompiledTemplate, FlowSpec, NodeKind, ResourceLimits, SessionError, Value,
    VarScope, VarType,
};

mod analysis;
mod nodes;

use analysis::{reject_uninitialized_reads, reject_unreachable};
use nodes::{check_target, validate_node};

/// Validate a flow's graph, variables, templates, and budget against the
/// operator's shipped byte ceilings.
pub(super) fn validate_flow(spec: &FlowSpec) -> Result<(), SessionError> {
    validate_flow_within(spec, ResourceLimits::shipped())
}

/// Validate a flow's graph, variables, templates, budget, and byte ceilings.
///
/// The ceilings are a parameter because two callers enforce different ones:
/// [`FlowSpec::validate`] applies the shipped ceilings a document must satisfy
/// on its own, and a session applies the effective ceilings it will run under —
/// its own, narrowed by whatever the document asked for. Both runs are the same
/// validation, so "the document is valid" and "the document can be run here"
/// cannot disagree about a candidate list or a template.
pub(super) fn validate_flow_within(
    spec: &FlowSpec,
    limits: ResourceLimits,
) -> Result<(), SessionError> {
    // First, and before any structural work: a document that asks for a
    // ceiling above the operator's gets a diagnostic naming the axis it asked
    // about, not a run under a bound it did not choose.
    let requested = spec.bounds.effective_resources().within_ceiling()?;
    let effective = requested.narrowed(limits);
    if spec.nodes.is_empty() {
        let refusal = Err(SessionError::MalformedFlow {
            cause: "flow declares no nodes".into(),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_flow_within: returning an error to the caller");
        return refusal;
    }
    if !spec.nodes.contains_key(&spec.entry) {
        let refusal = Err(SessionError::InvalidTransitionTarget {
            from: "<entry>".into(),
            target: spec.entry.clone(),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_flow_within: returning an error to the caller");
        return refusal;
    }
    if spec.bounds.budget == 0 || spec.nodes.len() > spec.bounds.budget {
        let refusal = Err(SessionError::FlowBudgetExceeded {
            steps: spec.nodes.len(),
            budget: spec.bounds.budget,
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_flow_within: returning an error to the caller");
        return refusal;
    }

    validate_declarations(&spec.vars)?;
    validate_terminals(spec)?;

    let mut writers = BTreeSet::new();
    for (node_id, kind) in &spec.nodes {
        validate_node(spec, node_id, kind, effective, &mut writers)?;
    }
    for name in spec.vars.keys() {
        if !writers.contains(name) {
            let refusal = Err(SessionError::VariableNeverWritten { name: name.clone() });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_flow_within: returning an error to the caller");
            return refusal;
        }
    }
    for edge in &spec.edges {
        let (from, to) = edge.endpoints();
        if !spec.nodes.contains_key(from) {
            let refusal = Err(SessionError::InvalidTransitionTarget {
                from: from.to_owned(),
                target: from.to_owned(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_flow_within: returning an error to the caller");
            return refusal;
        }
        check_target(&spec.nodes, from, to)?;
    }
    reject_uninitialized_reads(spec)?;
    reject_unreachable(spec)
}

/// Compiles `text`, attributing a malformed template to the node and field it
/// was reached through.
///
/// `compile` refuses with `MalformedTemplate` naming the placeholder
/// `"<runtime>"`, because it does not know its caller. The only thing this adds
/// is which node and field it was reached through, so the match is on the
/// variant rather than a map that would discard it: there is no detail in the
/// cause to keep, and a map would read as though there were.
fn compile_for_node<'a>(
    node_id: &str,
    field: &'static str,
    text: &'a str,
) -> Result<CompiledTemplate<'a>, SessionError> {
    match CompiledTemplate::compile(text) {
        Ok(compiled) => Ok(compiled),
        Err(SessionError::MalformedTemplate { .. }) => {
            let refusal = Err(SessionError::MalformedTemplate {
                node: node_id.to_owned(),
                field,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), node = node_id, field, "compile_for_node: the template is malformed");
            refusal
        }
        Err(other) => {
            lgwks_std::trace::debug!(error = ?other, node = node_id, field, "compile_for_node: the template did not compile");
            Err(other)
        }
    }
}

/// Looks `name` up in `scope`, refusing with [`SessionError::VariableUnset`] when it
/// holds no value.
///
/// One place for the refusal both the sizing pass and the render pass make, so a
/// marker with no value reads the same whichever of the two reached it first.
pub(super) fn bound<'s>(scope: &'s VarScope, name: &str) -> Result<&'s Value, SessionError> {
    match scope.get(name) {
        Some(value) => Ok(value),
        None => {
            let refusal = Err(SessionError::VariableUnset {
                name: name.to_owned(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), name, "bound: the marker has no value");
            refusal
        }
    }
}

/// Validate variable declaration names and choice contents.
pub(super) fn validate_declarations(vars: &BTreeMap<String, VarType>) -> Result<(), SessionError> {
    for (name, kind) in vars {
        if !valid_identifier(name) {
            let refusal = Err(SessionError::MalformedFlow {
                cause: format!("invalid variable name {name:?}"),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_declarations: returning an error to the caller");
            return refusal;
        }
        match *kind {
            VarType::Choice(ref options)
                if options.is_empty() || has_duplicate_strings(options) =>
            {
                let refusal = Err(SessionError::MalformedFlow {
                    cause: format!("choice variable {name:?} has invalid options"),
                });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_declarations: returning an error to the caller");
                return refusal;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Validate one terminal declaration and its node kind.
///
/// Two invariants, and the second is the one that was missing: a declaration
/// must name a terminal node, and on a node whose kind already fixes the
/// outcome it must not contradict it. `End` completes with nothing said about
/// where the person goes, so every declared outcome is a genuine override and
/// is accepted. A `Handoff` or `Refer` node already names its target, so a
/// declaration of a different outcome — or of the same kind aimed somewhere
/// else — is an authoring contradiction between two statements in one
/// document, and refusing it is the only reading that cannot be wrong. A
/// declaration that repeats the node's own outcome is accepted; it is then
/// read by [`FlowSpec::effective_terminal`] rather than ignored.
fn validate_terminals(spec: &FlowSpec) -> Result<(), SessionError> {
    for (node_id, declared) in &spec.terminals {
        let Some(kind) = spec.nodes.get(node_id) else {
            let refusal = Err(SessionError::InvalidTransitionTarget {
                from: "<terminal>".into(),
                target: node_id.clone(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_terminals: returning an error to the caller");
            return refusal;
        };
        if !matches!(
            *kind,
            NodeKind::End | NodeKind::Handoff { .. } | NodeKind::Refer { .. }
        ) {
            let refusal = Err(SessionError::MalformedFlow {
                cause: format!("terminal declaration {node_id:?} names a non-terminal node"),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_terminals: returning an error to the caller");
            return refusal;
        }
        // `End` has an intrinsic outcome too, but it is `Completed` — the
        // absence of a destination rather than a claim about one — so a
        // declaration replaces it instead of conflicting with it. Only the two
        // node kinds that name a target can be contradicted.
        if matches!(*kind, NodeKind::End) {
            continue;
        }
        let Some(intrinsic) = spec.intrinsic_terminal(node_id) else {
            // Unreachable on a validated document: the kind was just checked to
            // be one of the three terminal kinds, and every one of them has an
            // intrinsic outcome. Reported rather than skipped so a future node
            // kind added to the check above cannot silently lose the rule.
            let refusal = Err(SessionError::MalformedFlow {
                cause: format!("terminal declaration {node_id:?} has no intrinsic outcome"),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_terminals: returning an error to the caller");
            return refusal;
        };
        if intrinsic != *declared {
            let refusal = Err(SessionError::ConflictingTerminalDeclaration {
                node: node_id.clone(),
                intrinsic,
                declared: declared.clone(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_terminals: returning an error to the caller");
            return refusal;
        }
    }
    Ok(())
}

/// Reject repeated strings in a finite declaration.
fn has_duplicate_strings(values: &[String]) -> bool {
    let mut seen = BTreeSet::new();
    values.iter().any(|value| !seen.insert(value))
}

/// Return whether a variable or node id uses the supported identifier shape.
pub(super) fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}
