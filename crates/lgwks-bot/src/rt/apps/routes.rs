//! The `POST /apps` and `POST /apps/run` routes for the tenant HTTP edge.

// ── the edge ─────────────────────────────────────────────────────────────────

use std::sync::Arc;

use lgwks_std::json::{Deserialize, Serialize, from_slice, to_vec};

use crate::rt::serve::{RouteMethod, TypedRoute};
use crate::script::{Scope, Tenant};

use super::run_app;
use super::store::{AppStore, OpRegistry, Outcome};

/// The `{"app": "<id>"}` an admission answers.
#[derive(Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct AdmitOk<'a> {
    /// The digest of the exact bytes admitted, in hex.
    app: &'a str,
}

/// One refusal as the edge answers it.
#[derive(Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct RefusalOut<'a> {
    /// `line:col` for text, a JSON pointer for JSON.
    location: &'a str,
    /// The arm that fired.
    kind: &'a str,
    /// What it names.
    message: &'a str,
}

/// The refusal list an admission answers.
#[derive(Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct RefusalsOut<'a> {
    /// Every refusal, sorted.
    refusals: Vec<RefusalOut<'a>>,
}

impl<'a> RefusalsOut<'a> {
    /// The edge form of `refusals`.
    fn of(refusals: &'a [super::refusal::PlanRefusal]) -> Self {
        Self {
            refusals: refusals
                .iter()
                .map(|refusal| RefusalOut {
                    location: refusal.location(),
                    kind: refusal.kind().label(),
                    message: refusal.message(),
                })
                .collect(),
        }
    }
}

/// The printed lines a run answers.
#[derive(Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct LinesOut {
    /// One line per executed operation, in declaration order.
    lines: Vec<String>,
}

/// The `{"app": "<id>"}` a run asks for.
#[derive(Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
struct RunAsk {
    /// The digest of the plan to run.
    app: String,
}

/// The `{"error": "..."}` a run answers when it cannot run.
#[derive(Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct RunFailed<'a> {
    /// What went wrong.
    error: &'a str,
}

/// Encode `value` as JSON, or an empty refusal list when it cannot encode.
fn encode<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    match to_vec(value) {
        Ok(bytes) => bytes,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "apps: the verdict does not encode");
            b"{\"refusals\":[]}".to_vec()
        }
    }
}

/// The verdict as the edge answers it.
pub(crate) fn encode_verdict(outcome: &Outcome) -> Vec<u8> {
    match *outcome {
        Outcome::Admitted(ref admitted) => encode(&AdmitOk { app: admitted.id() }),
        Outcome::Refused(ref refused) => encode(&RefusalsOut::of(refused.refusals())),
    }
}

/// Whether `content_type` names a JSON body.
fn is_json(content_type: Option<&str>) -> bool {
    let Some(value) = content_type else {
        return false;
    };
    let first = match value.split(';').next() {
        Some(first) => first,
        None => return false,
    };
    first.trim().eq_ignore_ascii_case("application/json")
}

/// Who holds what: the capabilities each tenant already holds.
pub type HeldGrants = Arc<dyn Fn(&str) -> Vec<crate::cap::Cap> + Send + Sync>;

/// The `POST /apps` and `POST /apps/run` routes for the tenant HTTP edge.
///
/// `registry` holds the operations every tenant admits and runs;
/// `held` names the capabilities each tenant already holds. Admitting answers
/// `200` with `{"app": "<id>"}` or the refusal list with `400`, `413` or
/// `422`; running answers `200` with the printed lines, or `404` and `500`
/// typed by [`RunKind`](super::run::RunKind).
#[must_use]
pub fn apps_routes(
    store: Arc<AppStore>,
    registry: Arc<OpRegistry>,
    held: HeldGrants,
) -> Vec<TypedRoute> {
    let admitting = Arc::clone(&store);
    let providing = Arc::clone(&registry);
    let holding = Arc::clone(&held);
    let admit = TypedRoute::new_with_content_type(
        RouteMethod::Post,
        "/apps",
        move |tenant: String, content: Option<String>, body: Vec<u8>| {
            let admitting = Arc::clone(&admitting);
            let providing = Arc::clone(&providing);
            let holding = Arc::clone(&holding);
            async move {
                let granted = holding(tenant.as_str());
                let surface = match providing.surface(tenant.as_str(), granted.as_slice()) {
                    Ok(surface) => surface,
                    Err(error) => {
                        lgwks_std::trace::debug!(error = ?error, "apps: the tenant surface is refused");
                        return Err((500u16, format!("the tenant surface is refused: {error}")));
                    }
                };
                let outcome = if is_json(content.as_deref()) {
                    admitting.admit_json(&surface, &providing, body.as_slice())
                } else {
                    admitting.admit_text(&surface, &providing, body.as_slice())
                };
                if outcome.http_status() == 200 {
                    Ok(outcome.verdict_json())
                } else {
                    let verdict =
                        String::from_utf8_lossy(outcome.verdict_json().as_slice()).into_owned();
                    lgwks_std::trace::debug!(
                        status = outcome.http_status(),
                        "apps: the plan is refused"
                    );
                    Err((outcome.http_status(), verdict))
                }
            }
        },
    );
    let running = Arc::clone(&store);
    let run = TypedRoute::new(
        RouteMethod::Post,
        "/apps/run",
        move |tenant: String, body: Vec<u8>| {
            let running = Arc::clone(&running);
            async move {
                let asked: RunAsk = match from_slice(body.as_slice()) {
                    Ok(asked) => asked,
                    Err(error) => {
                        lgwks_std::trace::debug!(error = ?error, "apps: the run request does not decode");
                        return Err((400u16, format!("the run request does not decode: {error}")));
                    }
                };
                let scope = match Tenant::new(tenant.as_str()) {
                    Ok(tenant) => Scope::root(tenant),
                    Err(error) => {
                        lgwks_std::trace::debug!(error = ?error, "apps: the tenant is refused");
                        return Err((500u16, format!("the tenant is refused: {error}")));
                    }
                };
                match run_app(&scope, &running, asked.app.as_str()).await {
                    Ok(lines) => {
                        let printed = lines
                            .iter()
                            .map(|line| String::from_utf8_lossy(line.as_slice()).into_owned())
                            .collect();
                        Ok(encode(&LinesOut { lines: printed }))
                    }
                    Err(error) => {
                        let status = match error.kind() {
                            super::run::RunKind::UnknownApp => 404,
                            _ => 500,
                        };
                        let body = encode(&RunFailed {
                            error: error.detail(),
                        });
                        let message = String::from_utf8_lossy(body.as_slice()).into_owned();
                        lgwks_std::trace::debug!(error = ?error, "apps: the run is refused");
                        Err((status, message))
                    }
                }
            }
        },
    );
    vec![admit, run]
}
