//! The offline half of `estate apps`: the same plans admitted straight
//! through the library, with every verdict required to equal what the edge
//! answered byte for byte.
//!
//! An operator checking a plan before posting it reads the library verdict;
//! a model posting it reads the edge's. This half proves they are one
//! answer: the refusal bytes, the status, the located refusal arms, the
//! tight-bounds refusal, the JSON form and the run's printed lines.

use lgwks_bot::rt::apps::{
    AppStore, OpRegistry, Outcome, PlanLimits, PlanRefusal, RefusalKind, run_app,
};
use lgwks_bot::script::{Scope, Tenant};

use super::apps::{AppsError, REFUSED, REPAIRED, refused, say};

/// The JSON of the repaired plan, as the script map writes it.
const REPAIRED_JSON: &str = "{\"flows\":[{\"steps\":[{\"kind\":\"Graph\",\"subject\":\"sized to the machine\",\"line\":2,\"steps\":[{\"kind\":\"Node\",\"subject\":\"fetch\",\"line\":3,\"steps\":[]},{\"kind\":\"Node\",\"subject\":\"render\",\"line\":5,\"steps\":[]}]}]}]}";

/// What the edge answered on the journey, for the library to match.
pub(crate) struct EdgeAnswers {
    /// The refusal body the edge answered for [`REFUSED`].
    pub(crate) refused: String,
    /// The app id the edge admitted [`REPAIRED`] under.
    pub(crate) id: String,
    /// The run body the edge answered for that app.
    pub(crate) run: String,
}

/// Refuse with `wanted` versus `found` for `request` unless `held`.
fn require(held: bool, request: &str, wanted: &str, found: String) -> Result<(), AppsError> {
    if held {
        return Ok(());
    }
    refused(AppsError::Answer {
        request: request.to_owned(),
        wanted: wanted.to_owned(),
        found,
    })
}

/// Admit the journey's plans through `apps` and `registry` directly and
/// require each verdict to match `edge`.
pub(crate) fn check(
    apps: &AppStore,
    registry: &OpRegistry,
    edge: &EdgeAnswers,
) -> Result<(), AppsError> {
    let surface = registry
        .surface("acme", &[])
        .map_err(|cause| AppsError::Setup(format!("the acme surface is refused: {cause}")))?;
    let refusal = apps.admit_text(&surface, registry, REFUSED.as_bytes());
    let library = String::from_utf8_lossy(refusal.verdict_json().as_slice()).into_owned();
    require(
        refusal.http_status() == 422 && library == edge.refused,
        "admit_text(REFUSED)",
        "422 with the edge's exact refusal bytes",
        format!("{} {library:?}", refusal.http_status()),
    )?;
    let located = refusal.refusals().iter().any(|arm: &PlanRefusal| {
        arm.kind() == RefusalKind::UnknownOperation && PlanRefusal::location(arm) == "5:9"
    });
    require(
        located,
        "admit_text(REFUSED)",
        "unknown-operation at 5:9",
        library,
    )?;
    say("offline: the library refuses steal at 5:9 with the edge's exact bytes")?;
    let tight = AppStore::new(
        PlanLimits::default()
            .with_max_bytes(4_096)
            .with_max_depth(8)
            .with_max_steps(64)
            .with_max_fan_out(1),
    );
    let bounded = tight.admit_text(&surface, registry, REPAIRED.as_bytes());
    let over = bounded
        .refusals()
        .iter()
        .any(|arm| arm.kind() == RefusalKind::BoundExceeded);
    require(
        over && bounded.http_status() == 422,
        "admit_text(REPAIRED) under fan-out 1",
        "422 bound-exceeded",
        format!("{:?}", bounded.refusals()),
    )?;
    say("offline: a fan-out ceiling of 1 refuses the two-node graph")?;
    let json = apps.admit_json(&surface, registry, REPAIRED_JSON.as_bytes());
    let same = matches!(json, Outcome::Admitted(ref admitted)
        if admitted.operations().iter().map(|op| op.name()).eq(["fetch", "render"]));
    require(
        same,
        "admit_json(REPAIRED_JSON)",
        "admitted fetch then render",
        format!("{json:?}"),
    )?;
    say("offline: the JSON form admits fetch then render")?;
    let scope = Tenant::new("acme")
        .map(Scope::root)
        .map_err(|cause| AppsError::Setup(format!("the acme tenant is refused: {cause}")))?;
    let lines = lgwks_bot::block_on(run_app(&scope, apps, edge.id.as_str()))
        .map_err(|cause| AppsError::Exchange(format!("the library run failed: {cause}")))?;
    let printed: Vec<String> = lines
        .iter()
        .map(|line| String::from_utf8_lossy(line.as_slice()).into_owned())
        .collect();
    let matched = printed.len() == 2 && printed.iter().all(|line| edge.run.contains(line.as_str()));
    require(
        matched,
        "run_app(admitted)",
        "the edge's printed lines",
        format!("{printed:?}"),
    )?;
    say("offline: the library run prints the edge's lines")
}
