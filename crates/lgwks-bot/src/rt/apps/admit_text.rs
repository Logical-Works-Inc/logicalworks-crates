//! Admitting `script!` text: the one parser's tree, walked for bounds and ops.

use std::str::FromStr;

use lgwks_ast::script::{Block, BranchValue, Construct, Graph, Kind, Statement, StepShape};
use lgwks_deps::proc_macro2::TokenStream;

use super::limits::PlanLimits;
use super::refusal::{PlanRefusal, RefusalKind, order, point, refuse};
use super::store::{Stats, View, Walked, walk};

/// Read `body` as `script!` text: the tree, or the first refusal.
///
/// The byte count and the line count are charged before anything is parsed,
/// so an oversized body is refused before it is buffered past the limit.
/// What this returns is walked but unauthorized: bounds, operations and
/// edges for the store to admit.
pub(crate) fn parse_text(body: &[u8], limits: PlanLimits) -> Result<Walked, PlanRefusal> {
    if body.len() > limits.max_bytes() {
        return refuse(PlanRefusal::over_body(body.len(), limits.max_bytes()));
    }
    let text = match std::str::from_utf8(body) {
        Ok(text) => text,
        Err(error) => {
            let (line, col) = line_col_of(body, error.valid_up_to());
            return refuse(PlanRefusal::at(
                point(line, col),
                order(line, col),
                RefusalKind::Malformed,
                String::from("the plan body is not UTF-8 text"),
            ));
        }
    };
    if lines_of(text) > limits.max_steps() {
        return refuse(PlanRefusal::at(
            String::from("1:1"),
            order(1, 1),
            RefusalKind::BoundExceeded,
            format!(
                "the plan holds {} lines, past the ceiling of {} steps",
                lines_of(text),
                limits.max_steps()
            ),
        ));
    }
    let tokens = match TokenStream::from_str(text) {
        Ok(tokens) => tokens,
        Err(error) => {
            return refuse(PlanRefusal::at(
                String::from("1:1"),
                order(1, 1),
                RefusalKind::Malformed,
                format!("the plan does not tokenize: {error}"),
            ));
        }
    };
    let script = match lgwks_ast::script::parse(tokens) {
        Ok(script) => script,
        Err(refusal) => {
            return refuse(PlanRefusal::at(
                point(refusal.line(), refusal.column().saturating_add(1)),
                order(refusal.line(), refusal.column().saturating_add(1)),
                RefusalKind::Malformed,
                format!("the plan does not parse: {}", refusal.message()),
            ));
        }
    };
    let rows: Vec<&str> = text.lines().collect();
    let top: Vec<&StepShape> = script
        .flows()
        .iter()
        .flat_map(|flow| flow.shape().steps())
        .collect();
    let views = view_of_shapes(top.as_slice(), rows.as_slice());
    let mut ops = Vec::new();
    let mut stats = Stats::default();
    walk(views.as_slice(), &mut stats, &mut ops);
    let mut edges = Vec::new();
    for flow in script.flows() {
        harvest(flow.body(), &mut edges);
    }
    Ok(Walked { ops, edges, stats })
}

/// How many lines `text` holds.
fn lines_of(text: &str) -> usize {
    text.bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        .saturating_add(1)
}

/// The line and column of byte `at`: 1-based, columns in characters.
fn line_col_of(body: &[u8], at: usize) -> (usize, usize) {
    let end = at.min(body.len());
    let head = &body[..end];
    let line = head
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        .saturating_add(1);
    let tail = match head.iter().rposition(|byte| *byte == b'\n') {
        Some(pos) => &head[pos.saturating_add(1)..],
        None => head,
    };
    let col = String::from_utf8_lossy(tail)
        .chars()
        .count()
        .saturating_add(1);
    (line, col)
}

/// The 1-based column of source `line`: its first non-blank character.
fn column_of(rows: &[&str], line: usize) -> usize {
    match rows.get(line.saturating_sub(1)) {
        Some(row) => row
            .chars()
            .take_while(|cell| *cell == ' ' || *cell == '\t')
            .count()
            .saturating_add(1),
        None => 1,
    }
}

/// Whether a text step names an operation, and whether it fans out.
fn classify(step: &StepShape) -> (Option<String>, bool, bool) {
    let display = match step.kind() {
        Kind::Run
        | Kind::Observe
        | Kind::Act
        | Kind::Fetch
        | Kind::Ask
        | Kind::Send
        | Kind::Node => Some(step.subject().to_owned()),
        _ => None,
    };
    let node = step.kind() == Kind::Node;
    let fan = matches!(step.kind(), Kind::Each | Kind::Graph | Kind::Together);
    (display, node, fan)
}

/// The view of text steps, with `line:col` locations.
fn view_of_shapes(steps: &[&StepShape], rows: &[&str]) -> Vec<View> {
    let mut views = Vec::with_capacity(steps.len());
    for step in steps {
        let col = column_of(rows, step.line());
        let (display, node, fan) = classify(step);
        let mut children = Vec::new();
        for child in step.children() {
            children.extend(view_of_shapes(&[child], rows));
        }
        views.push(View {
            display,
            node,
            fan,
            location: point(step.line(), col),
            sort: order(step.line(), col),
            children,
        });
    }
    views
}

/// The graph edges a text plan declares: node names with their needs.
fn harvest(block: &Block, out: &mut Vec<(String, Vec<String>)>) {
    for statement in block.statements() {
        match *statement {
            Statement::Bind {
                block: Construct::Graph(ref found),
                ..
            } => harvest_graph(found, out),
            Statement::Bind { ref block, .. } => walk_construct(block, out),
            Statement::Construct(Construct::Graph(ref found)) => harvest_graph(found, out),
            Statement::Construct(ref other) => walk_construct(other, out),
            Statement::Together(ref together) => {
                for branch in together.branches() {
                    if let BranchValue::Construct(ref constructed) = *branch.value() {
                        walk_construct(constructed, out);
                    }
                }
            }
            Statement::For(ref each) => harvest(each.body(), out),
            Statement::Compensate(ref compensate) => harvest(compensate.undo(), out),
            Statement::If(ref chain) => {
                for branch in chain.branches() {
                    harvest(branch.body(), out);
                }
            }
            _ => {}
        }
    }
}

/// Walk one block word for the graphs nested inside it.
fn walk_construct(constructed: &Construct, out: &mut Vec<(String, Vec<String>)>) {
    match *constructed {
        Construct::Graph(ref found) => harvest_graph(found, out),
        Construct::Each(ref each) => harvest(each.body(), out),
        Construct::Within(ref within) => harvest(within.body(), out),
        Construct::Retry(ref retry) => harvest(retry.body(), out),
        Construct::Step(ref step) => harvest(step.body(), out),
        Construct::Whenever(ref whenever) => harvest(whenever.body(), out),
        Construct::Justify(ref justify) => harvest(justify.governed(), out),
        Construct::Admit(ref admit) => harvest(admit.payload(), out),
        Construct::Ready(ref ready) => harvest(ready.body(), out),
        Construct::Approval(ref approval) => harvest(approval.body(), out),
        Construct::Saga(ref saga) => harvest(saga.body(), out),
        Construct::Serve(ref serve) => {
            for route in serve.routes() {
                harvest(route.body(), out);
            }
        }
        _ => {}
    }
}

/// Keep one graph's nodes with their needs, and walk their bodies.
fn harvest_graph(found: &Graph, out: &mut Vec<(String, Vec<String>)>) {
    for node in found.nodes() {
        out.push((node.name().to_owned(), node.needs().to_vec()));
        harvest(node.body(), out);
    }
}
