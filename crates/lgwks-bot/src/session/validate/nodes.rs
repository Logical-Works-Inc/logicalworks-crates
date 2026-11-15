//! Per-node validation: what each node kind must satisfy at load. Split from `validate.rs` (#990).

use super::analysis::comparison_operands;
use super::compile_for_node;
use crate::session::{
    AnswerDomain, BTreeMap, BTreeSet, FlowSpec, NodeId, NodeKind, Predicate, ResourceAxis,
    ResourceLimits, SessionError, TemplatePart, ValueExpr, VarType, decode_integer,
};

/// Validates one `branch` node: its subject, its predicate and both edges.
fn validate_branch_node(
    spec: &FlowSpec,
    node_id: &str,
    var: &str,
    when: &Predicate,
    then: &str,
    otherwise: &str,
) -> Result<(), SessionError> {
    validate_variable_reference(spec, var)?;
    validate_predicate(spec, node_id, when)?;
    check_edge_pair(&spec.nodes, node_id, then, otherwise)
}

/// Validates one `refer` node: its template and every edge it continues along.
fn validate_refer_node(
    spec: &FlowSpec,
    node_id: &str,
    text: &str,
    limits: ResourceLimits,
) -> Result<(), SessionError> {
    validate_template(spec, node_id, text, "refer.text", limits)?;
    for target in spec.edge_targets(node_id) {
        check_target(&spec.nodes, node_id, &target)?;
    }
    Ok(())
}

/// Validates one `say` node: its template, and that it has exactly one way out.
///
/// A helper so every kind's arm in [`validate_node`] is one call. A `say` node
/// that continues along two edges is ambiguous at run time -- which one the
/// driver follows is not written down anywhere -- and a `say` node with no
/// continuation and no terminal leaves the conversation with nowhere to go, so
/// both are refused at load rather than at the moment a person is waiting.
fn validate_say_node(
    spec: &FlowSpec,
    node_id: &str,
    text: &str,
    limits: ResourceLimits,
) -> Result<(), SessionError> {
    validate_template(spec, node_id, text, "say.text", limits)?;
    if spec.edge_targets(node_id).len() > 1 {
        let refusal = Err(SessionError::MalformedFlow {
            cause: format!("say node {node_id:?} has multiple continuations"),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_say_node: returning an error to the caller");
        return refusal;
    }
    if spec.edge_targets(node_id).is_empty() && !spec.terminals.contains_key(node_id) {
        let refusal = Err(SessionError::MissingTransition {
            node: node_id.to_owned(),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_say_node: returning an error to the caller");
        return refusal;
    }
    Ok(())
}

/// Validates one `ask` node: its variable, its options and its routes.
///
/// A helper rather than the arm inline: as one block the arm carried the
/// variable lookup, the option decode, the byte check and the two route passes,
/// so `validate_node` read as a list of kinds when what it needed to say was
/// which parts of an `ask` node are checked and in what order -- decode with
/// the runtime's own decoder first, then the session's own ceiling, then the
/// routes, so a refusal names the earliest thing wrong rather than an arbitrary
/// one.
fn validate_ask_node(
    spec: &FlowSpec,
    node_id: &str,
    var: &str,
    options: &[String],
    routes: &BTreeMap<String, NodeId>,
    limits: ResourceLimits,
    writers: &mut BTreeSet<String>,
) -> Result<(), SessionError> {
    let declared = declared_variable(spec, var)?;
    writers.insert(var.to_owned());
    if options.is_empty() || super::has_duplicate_strings(options) {
        let refusal = Err(SessionError::MalformedFlow {
            cause: format!("ask node {node_id:?} has invalid options"),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_ask_node: returning an error to the caller");
        return refusal;
    }
    // Distinct strings are not distinct answers. Two options the answer
    // policy cannot tell apart are one option the person cannot choose
    // between, and the resolver can only ever report the tie — so the
    // authoring mistake is refused here, where it names an option,
    // rather than at run time, where it names none.
    if let Some((first, second)) = colliding_options(options, answer_domain_of(spec, var)) {
        let refusal = Err(SessionError::MalformedFlow {
            cause: format!(
                "ask node {node_id:?} offers {first:?} and {second:?} as the same \
                         {} answer",
                answer_domain_of(spec, var)
            ),
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_ask_node: returning an error to the caller");
        return refusal;
    }
    for option in options {
        let Some(target) = routes.get(option) else {
            let refusal = Err(SessionError::MissingAskRoute {
                node: node_id.to_owned(),
                option: option.clone(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_ask_node: returning an error to the caller");
            return refusal;
        };
        check_target(&spec.nodes, node_id, target)?;
        // Every candidate is decoded with the same decoder the runtime
        // assigns with, so "the flow asks a question the variable
        // cannot hold an answer to" is a load-time refusal rather than
        // a conversation the person cannot complete.
        let value = match declared.decode_answer(option) {
            Ok(value) => value,
            Err(rejection) => {
                let refusal = Err(SessionError::AskOptionNotAssignable {
                    node: node_id.to_owned(),
                    variable: var.to_owned(),
                    option: option.clone(),
                    expected: declared.label(),
                    cause: rejection.to_string(),
                });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_ask_node: returning an error to the caller");
                return refusal;
            }
        };
        // Then that the value fits the session that will store it.
        // "The candidate cannot be held" and "the candidate cannot be
        // held *here*" are different statements, and both are knowable
        // while the document is loading: the second one is a session
        // whose value ceiling is below the option it asked for, and
        // deferring it to the person's answer would charge them a step
        // for an operator's configuration.
        let value_bytes = limits.get(ResourceAxis::Value);
        if value.rendered_bytes() > value_bytes {
            let refusal = Err(SessionError::AskOptionTooLarge {
                node: node_id.to_owned(),
                variable: var.to_owned(),
                option: option.clone(),
                bytes: value.rendered_bytes(),
                limit: value_bytes,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_ask_node: returning an error to the caller");
            return refusal;
        }
    }
    for (option, target) in routes {
        if !options.iter().any(|candidate| candidate == option) {
            let refusal = Err(SessionError::MalformedFlow {
                cause: format!("ask node {node_id:?} routes undeclared option {option:?}"),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_ask_node: returning an error to the caller");
            return refusal;
        }
        check_target(&spec.nodes, node_id, target)?;
    }
    Ok(())
}

/// Checks both edges of a branch, in the order the branch names them.
///
/// A helper for the pair rather than two calls: the branch's whole contract is
/// that both targets resolve, and a reader checking one edge had no reason to
/// look for the other three lines below it.
fn check_edge_pair(
    nodes: &BTreeMap<NodeId, NodeKind>,
    node_id: &str,
    first: &str,
    second: &str,
) -> Result<(), SessionError> {
    check_target(nodes, node_id, first)?;
    check_target(nodes, node_id, second)
}

/// Validates one node's references, variables, and text.
///
/// The two kinds this skips are `handoff` and `end`, which carry no text, no
/// variable and no edge to resolve: there is nothing about them that a load-time
/// check could refuse. That is a claim about the kinds, so it is stated here
/// rather than left as two empty arms inside the dispatch.
///
/// One call rather than a `match` of seven: every arm was a single validator
/// call, which is the right shape for the reader and the wrong shape for a gate
/// that reads the whole `match` as one statement.
pub(super) fn validate_node(
    spec: &FlowSpec,
    node_id: &str,
    kind: &NodeKind,
    limits: ResourceLimits,
    writers: &mut BTreeSet<String>,
) -> Result<(), SessionError> {
    match *kind {
        NodeKind::Say { ref text } => validate_say_node(spec, node_id, text, limits),
        NodeKind::Ask {
            ref var,
            ref options,
            ref routes,
        } => validate_ask_node(spec, node_id, var, options, routes, limits, writers),
        NodeKind::Branch {
            ref var,
            ref when,
            ref then,
            ref otherwise,
        } => validate_branch_node(spec, node_id, var, when, then, otherwise),
        NodeKind::Refer { ref text, .. } => validate_refer_node(spec, node_id, text, limits),
        NodeKind::Route {
            ref dispatch,
            ref fallback,
        } => {
            check_target(&spec.nodes, node_id, dispatch)?;
            check_target(&spec.nodes, node_id, fallback)
        }
        NodeKind::Handoff { .. } | NodeKind::End => Ok(()),
    }
}

/// Returns how answers writing `name` are read in `spec`.
///
/// The flow's own reading of a node's answers, so validation and resolution
/// cannot disagree about which policy a question is under — the same reason
/// there is one decoder rather than two.
fn answer_domain_of(spec: &FlowSpec, name: &str) -> AnswerDomain {
    match spec.vars.get(name) {
        Some(declared) => declared.answer_domain(),
        None => AnswerDomain::Label,
    }
}

/// Returns the first pair of options the answer policy cannot tell apart.
///
/// `None` under [`AnswerDomain::Integer`] for an option that is not a whole
/// number: an option that cannot produce the declared value is a different
/// defect, owned by the check that reports it, and folding it into a collision
/// report would name the wrong cause.
fn colliding_options(options: &[String], domain: AnswerDomain) -> Option<(String, String)> {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for option in options {
        let identity = match domain {
            AnswerDomain::Label => crate::language::normalize(option),
            AnswerDomain::Integer => match decode_integer(option) {
                Some(value) => value.to_string(),
                None => continue,
            },
        };
        if let Some(first) = seen.get(&identity) {
            return Some((first.clone(), option.clone()));
        }
        seen.insert(identity, option.clone());
    }
    None
}

/// Validate a variable reference in a node.
fn validate_variable_reference(spec: &FlowSpec, name: &str) -> Result<(), SessionError> {
    declared_variable(spec, name).map(|_declared| ())
}

/// Resolve a variable's declared type, refusing an undeclared name.
fn declared_variable<'a>(spec: &'a FlowSpec, name: &str) -> Result<&'a VarType, SessionError> {
    spec.vars
        .get(name)
        .ok_or_else(|| SessionError::UndeclaredVariable {
            name: name.to_owned(),
        })
}

/// Validate a predicate's variable references and non-empty boolean lists.
fn validate_predicate(
    spec: &FlowSpec,
    node_id: &str,
    predicate: &Predicate,
) -> Result<(), SessionError> {
    if let Some((left, right)) = comparison_operands(predicate) {
        validate_expr(spec, left)?;
        return validate_expr(spec, right);
    }
    match *predicate {
        Predicate::Const(_) => Ok(()),
        Predicate::And(ref items) | Predicate::Or(ref items) => {
            if items.is_empty() {
                let refusal = Err(SessionError::MalformedFlow {
                    cause: format!("node {node_id:?} has an empty predicate"),
                });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_predicate: returning an error to the caller");
                return refusal;
            }
            for item in items {
                validate_predicate(spec, node_id, item)?;
            }
            Ok(())
        }
        Predicate::Not(ref inner) => validate_predicate(spec, node_id, inner),
        // Every comparison returned above, through the one list of them.
        Predicate::Eq(..)
        | Predicate::Ne(..)
        | Predicate::Lt(..)
        | Predicate::Le(..)
        | Predicate::Gt(..)
        | Predicate::Ge(..) => Ok(()),
    }
}

/// Validate one predicate expression.
fn validate_expr(spec: &FlowSpec, expression: &ValueExpr) -> Result<(), SessionError> {
    if let ValueExpr::Var(ref name) = *expression
        && !spec.vars.contains_key(name)
    {
        let refusal = Err(SessionError::UndeclaredVariable { name: name.clone() });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_expr: returning an error to the caller");
        return refusal;
    }
    Ok(())
}

/// Validate one template's syntax, its declared names, and its static floor.
///
/// The floor is the part of the expansion that is known without running the
/// flow: the literal bytes, which every render must contain whatever the
/// variables turn out to hold. A template whose literals alone exceed the
/// record ceiling can never render, so it is refused here rather than at the
/// node that would speak it. That is the only static size claim available —
/// the rest of the expansion depends on answers this cannot see — and it is
/// exactly the claim that is true unconditionally.
fn validate_template(
    spec: &FlowSpec,
    node_id: &str,
    template: &str,
    field: &'static str,
    limits: ResourceLimits,
) -> Result<(), SessionError> {
    let compiled = compile_for_node(node_id, field, template)?;
    let mut literals = 0usize;
    for part in compiled.parts() {
        match *part {
            TemplatePart::Literal(text) => {
                literals = literals.checked_add(text.len()).ok_or(
                    SessionError::TemplateExpansionTooLarge {
                        node: node_id.to_owned(),
                        bytes: usize::MAX,
                        limit: limits.get(ResourceAxis::Record),
                    },
                )?;
            }
            TemplatePart::Variable(name) => {
                if !spec.vars.contains_key(name) {
                    let refusal = Err(SessionError::UndeclaredVariable {
                        name: name.to_owned(),
                    });
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_template: returning an error to the caller");
                    return refusal;
                }
            }
        }
    }
    let record_bytes = limits.get(ResourceAxis::Record);
    if literals > record_bytes {
        let refusal = Err(SessionError::TemplateExpansionTooLarge {
            node: node_id.to_owned(),
            bytes: literals,
            limit: record_bytes,
        });
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "validate_template: returning an error to the caller");
        return refusal;
    }
    Ok(())
}

/// Validate that one target node exists.
pub(super) fn check_target(
    nodes: &BTreeMap<NodeId, NodeKind>,
    from: &str,
    target: &str,
) -> Result<(), SessionError> {
    if nodes.contains_key(target) {
        Ok(())
    } else {
        Err(SessionError::InvalidTransitionTarget {
            from: from.to_owned(),
            target: target.to_owned(),
        })
    }
}
