//! Admitting JSON of the same tree: the map's steps, walked for bounds and ops.

use lgwks_std::json::{Deserialize, from_slice};

use super::limits::PlanLimits;
use super::refusal::{PlanRefusal, RefusalKind, refuse};
use super::store::{Stats, View, Walked, walk};

/// A plan document: the `flows` of the script map as JSON.
#[derive(Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct JsonPlan {
    /// The flows, in source order.
    flows: Vec<JsonFlow>,
}

/// One flow of a plan document.
#[derive(Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct JsonFlow {
    /// The steps, in source order.
    steps: Vec<JsonStep>,
}

/// One step of a plan document.
#[derive(Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct JsonStep {
    /// The word the step is, as the map spells it.
    kind: String,
    /// The step's subject: the operation it names.
    subject: String,
    /// The steps nested inside it.
    steps: Vec<JsonStep>,
}

/// Read `body` as a plan document: the map, or the first refusal.
///
/// The byte ceiling is charged before anything is decoded; the structural
/// ceilings are measured by the same iterative walk the text form uses.
pub(crate) fn parse_json(body: &[u8], limits: PlanLimits) -> Result<Walked, PlanRefusal> {
    if body.len() > limits.max_bytes() {
        return refuse(PlanRefusal::over_body(body.len(), limits.max_bytes()));
    }
    let plan: JsonPlan = match from_slice(body) {
        Ok(plan) => plan,
        Err(error) => {
            return refuse(PlanRefusal::at(
                String::from("/"),
                String::from("j/"),
                RefusalKind::Malformed,
                format!("the body is not a plan document: {error}"),
            ));
        }
    };
    let mut views = Vec::new();
    for (index, flow) in plan.flows.iter().enumerate() {
        let base = format!("/flows/{index}");
        let flowed = view_of_json(flow.steps.as_slice(), base.as_str())?;
        views.extend(flowed);
    }
    let mut ops = Vec::new();
    let mut stats = Stats::default();
    walk(views.as_slice(), &mut stats, &mut ops);
    Ok(Walked {
        ops,
        edges: Vec::new(),
        stats,
    })
}

/// Whether a JSON step names an operation, fans out, or is refused.
///
/// `None` is a kind no script step spells.
fn classify_json(kind: &str) -> Option<(bool, bool, bool)> {
    match kind {
        "Run" | "Observe" | "Act" | "Fetch" | "Ask" | "Send" => Some((true, false, false)),
        "Node" => Some((true, true, false)),
        "Each" | "Graph" | "Together" => Some((false, false, true)),
        "Flow" | "Step" | "For" | "If" | "Else" | "Let" | "GiveBack" | "Fail" | "Saga"
        | "Compensate" | "Whenever" | "Justify" | "Remember" | "Admit" | "Ready" | "Approval"
        | "Serve" | "Route" | "Within" | "Retry" => Some((false, false, false)),
        _ => None,
    }
}

/// The view of JSON steps, with JSON-pointer locations.
///
/// # Errors
///
/// A step whose kind no script step spells, located at its pointer.
fn view_of_json(steps: &[JsonStep], base: &str) -> Result<Vec<View>, PlanRefusal> {
    let mut views = Vec::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        let pointer = format!("{base}/{index}");
        let Some((collect, node, fan)) = classify_json(step.kind.as_str()) else {
            return refuse(PlanRefusal::at(
                pointer.clone(),
                format!("j{pointer}"),
                RefusalKind::Malformed,
                format!("unknown step kind {:?} at {pointer}", step.kind),
            ));
        };
        views.push(View {
            display: collect.then(|| step.subject.clone()),
            node,
            fan,
            location: pointer.clone(),
            sort: format!("j{pointer}"),
            children: view_of_json(step.steps.as_slice(), pointer.as_str())?,
        });
    }
    Ok(views)
}
