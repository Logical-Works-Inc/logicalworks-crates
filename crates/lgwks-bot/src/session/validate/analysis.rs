//! Graph analyses over a flow: definite assignment and reachability. Split from `validate.rs` (#990).

use super::compile_for_node;
use crate::session::{
    BTreeMap, BTreeSet, FlowSpec, NodeId, NodeKind, Predicate, SessionError, TemplatePart,
    ValueExpr, VecDeque,
};

/// Reject every read of a variable that is not definitely assigned on all
/// paths reaching the reading node.
///
/// A global writer set answers "does some node write this variable", which is
/// not the question a read has to pass: a session starts with an empty
/// [`VarScope`](crate::session::VarScope), so a writer that executes only after the read, or only on a
/// branch the read is not reached through, leaves the read with no value and
/// the interpolator returning [`SessionError::VariableUnset`] on the first
/// step. This is a forward *must* (definite-assignment) analysis over the
/// execution graph instead of a membership test over the document.
///
/// The state of a node is the set of variables assigned on **every** path from
/// the entry to that node, which is a greatest fixed point: every node is
/// seeded with the full declaration set and each step intersects an outgoing
/// state into its successors, so states only shrink and a cycle converges. A
/// node no entry path reaches keeps that top element and is never accused
/// here — reachability is [`reject_unreachable`]'s single verdict, and one
/// defect must not surface as two.
///
/// Supported assignments and guards, stated so the boundary is not guessed at:
/// an [`NodeKind::Ask`] assigns its variable on each outgoing route and
/// nowhere earlier; every other node kind passes its incoming state through
/// unchanged. No symbolic reasoning about [`Predicate`] values is attempted,
/// so a branch is treated as able to take both successors, and a variable
/// assigned on one predecessor of a join but not another is *not* available at
/// the join. Both choices fail toward silence rather than rejecting a flow the
/// runner would have executed.
pub(super) fn reject_uninitialized_reads(spec: &FlowSpec) -> Result<(), SessionError> {
    let universe: BTreeSet<String> = spec.vars.keys().cloned().collect();
    let mut available: BTreeMap<NodeId, BTreeSet<String>> = spec
        .nodes
        .keys()
        .map(|node_id| (node_id.clone(), universe.clone()))
        .collect();
    available.insert(spec.entry.clone(), BTreeSet::new());

    let mut pending: VecDeque<NodeId> = spec.nodes.keys().cloned().collect();
    // Every queued id is a node and every node has an entry, so the lookups below
    // always answer; they are `if let` rather than a branch no flow can reach.
    while let Some(node_id) = pending.pop_front() {
        if let Some(kind) = spec.nodes.get(&node_id)
            && let Some(incoming) = available.get(&node_id).cloned()
        {
            let mut outgoing = incoming;
            if let NodeKind::Ask { ref var, .. } = *kind {
                outgoing.insert(var.clone());
            }
            for target in successor_targets(spec, &node_id, kind) {
                if let Some(state) = available.get_mut(&target) {
                    let before = state.len();
                    state.retain(|name| outgoing.contains(name));
                    if state.len() != before {
                        pending.push_back(target);
                    }
                }
            }
        }
    }

    for (node_id, kind) in &spec.nodes {
        if let Some(state) = available.get(node_id) {
            for name in node_reads(spec, node_id, kind)? {
                if !state.contains(&name) {
                    let refusal = Err(SessionError::VariableReadBeforeInit {
                        node: node_id.clone(),
                        name,
                    });
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "reject_uninitialized_reads: returning an error to the caller");
                    return refusal;
                }
            }
        }
    }
    Ok(())
}

/// Collect every variable a node reads when it executes.
///
/// "Reads" means the references the executor actually resolves, not every name
/// a node mentions. [`NodeKind::Branch`] is the case that separates the two:
/// its `var` field is documented as the branch subject, but the executor
/// destructures `when` alone and never resolves `var` against the scope, so a
/// branch subject that some path leaves unassigned cannot fail a run and is
/// not counted here. The predicate's own [`ValueExpr::Var`] references are
/// where a branch reads, and they are counted.
///
/// Only declared names are returned. An undeclared one is already
/// [`SessionError::UndeclaredVariable`] from `validate_node`, which runs first, so
/// the definite-assignment pass cannot preempt it with a second verdict on the
/// same defect.
fn node_reads(
    spec: &FlowSpec,
    node_id: &str,
    kind: &NodeKind,
) -> Result<BTreeSet<String>, SessionError> {
    match *kind {
        NodeKind::Say { ref text } => declared_template_reads(spec, node_id, text, "say.text"),
        NodeKind::Refer { ref text, .. } => {
            declared_template_reads(spec, node_id, text, "refer.text")
        }
        NodeKind::Branch { ref when, .. } => {
            let mut names = BTreeSet::new();
            collect_predicate_variables(when, &mut names);
            names.retain(|name| spec.vars.contains_key(name));
            Ok(names)
        }
        NodeKind::Ask { .. }
        | NodeKind::Handoff { .. }
        | NodeKind::Route { .. }
        | NodeKind::End => Ok(BTreeSet::new()),
    }
}

/// Return the declared variables a template interpolates.
fn declared_template_reads(
    spec: &FlowSpec,
    node_id: &str,
    text: &str,
    field: &'static str,
) -> Result<BTreeSet<String>, SessionError> {
    let compiled = compile_for_node(node_id, field, text)?;
    let mut names = BTreeSet::new();
    for part in compiled.parts() {
        if let TemplatePart::Variable(name) = *part
            && spec.vars.contains_key(name)
        {
            names.insert(name.to_owned());
        }
    }
    Ok(names)
}

/// The two operands of a comparison predicate, or `None` for anything else.
///
/// Every comparison — `Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge` — binds the same two
/// operands and nothing else, and two walks over a predicate (the one that
/// collects its variables and the one that validates them) have to agree on
/// which variants those are. They read the list from here instead of each
/// restating six variants, which is how a new comparison would be collected by
/// one and skipped by the other.
pub(super) fn comparison_operands(predicate: &Predicate) -> Option<(&ValueExpr, &ValueExpr)> {
    match *predicate {
        Predicate::Eq(ref left, ref right)
        | Predicate::Ne(ref left, ref right)
        | Predicate::Lt(ref left, ref right)
        | Predicate::Le(ref left, ref right)
        | Predicate::Gt(ref left, ref right)
        | Predicate::Ge(ref left, ref right) => Some((left, right)),
        Predicate::Const(_) | Predicate::And(_) | Predicate::Or(_) | Predicate::Not(_) => None,
    }
}

/// Collect the variable references of a predicate expression.
fn collect_predicate_variables(predicate: &Predicate, into: &mut BTreeSet<String>) {
    if let Some((left, right)) = comparison_operands(predicate) {
        collect_expr_variables(left, into);
        collect_expr_variables(right, into);
    } else {
        match *predicate {
            Predicate::Const(_) => {}
            Predicate::And(ref items) | Predicate::Or(ref items) => {
                for item in items {
                    collect_predicate_variables(item, into);
                }
            }
            Predicate::Not(ref inner) => collect_predicate_variables(inner, into),
            // Every comparison was taken above, and `comparison_operands` names
            // all six, so this arm can only be reached if that list and this
            // match have drifted apart — which is the point of naming it there.
            Predicate::Eq(..)
            | Predicate::Ne(..)
            | Predicate::Lt(..)
            | Predicate::Le(..)
            | Predicate::Gt(..)
            | Predicate::Ge(..) => {}
        }
    }
}

/// Collect the variable references of one predicate operand.
fn collect_expr_variables(expression: &ValueExpr, into: &mut BTreeSet<String>) {
    if let ValueExpr::Var(ref name) = *expression {
        into.insert(name.clone());
    }
}

/// Reject a graph node that cannot be reached from the entry.
pub(super) fn reject_unreachable(spec: &FlowSpec) -> Result<(), SessionError> {
    let mut seen = BTreeSet::new();
    let mut pending = VecDeque::from([spec.entry.clone()]);
    while let Some(node_id) = pending.pop_front() {
        if !seen.insert(node_id.clone()) {
            continue;
        }
        // Validation refused a dangling target first, so every queued node exists.
        if let Some(kind) = spec.nodes.get(&node_id) {
            for target in successor_targets(spec, &node_id, kind) {
                if !seen.contains(&target) {
                    pending.push_back(target);
                }
            }
        }
    }
    for node_id in spec.nodes.keys() {
        if !seen.contains(node_id) {
            let refusal = Err(SessionError::UnreachableNode {
                node: node_id.clone(),
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "reject_unreachable: returning an error to the caller");
            return refusal;
        }
    }
    Ok(())
}

/// Collect every statically possible destination from one node.
fn successor_targets(spec: &FlowSpec, node_id: &str, kind: &NodeKind) -> Vec<NodeId> {
    let mut targets = spec.edge_targets(node_id);
    match *kind {
        NodeKind::Ask { ref routes, .. } => targets.extend(routes.values().cloned()),
        NodeKind::Branch {
            ref then,
            ref otherwise,
            ..
        } => {
            targets.push(then.clone());
            targets.push(otherwise.clone());
        }
        NodeKind::Route {
            ref dispatch,
            ref fallback,
        } => {
            targets.push(dispatch.clone());
            targets.push(fallback.clone());
        }
        NodeKind::Say { .. }
        | NodeKind::Handoff { .. }
        | NodeKind::Refer { .. }
        | NodeKind::End => {}
    }
    targets
}
