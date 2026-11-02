//! Model-written plans over the tenant HTTP edge (#786), over loopback.
//!
//! Each test names the requirement it proves in its doc comment. A plan is
//! admitted against the tenant's operation registry: unknown operations,
//! missing capabilities and exceeded bounds are refused with their locations,
//! and an admitted plan runs its handlers through the graph scheduler. The
//! exchange shape lives once in `crate::serve_edge`.

#![cfg(all(feature = "edge", feature = "script"))]

use std::net::SocketAddr;
use std::sync::Arc;

use lgwks_bot::cap::Cap;
use lgwks_bot::rt::apps::{AppStore, HeldGrants, PlanLimits, apps_routes};

use crate::apps_harness::demo_registry;
use crate::scratch::Scratch;
use crate::serve_edge::{post, post_with, with_serve};

/// A plan naming an operation no registry holds.
const REFUSED: &str = "flow app():\n    let out = graph:\n        node fetch:\n            \"fetching\"\n        node steal needs fetch:\n            \"stealing\"\n";

/// A plan naming a registered operation its tenant was not granted.
const NEEDS_GRANT: &str =
    "flow app():\n    let out = graph:\n        node preview:\n            \"previewing\"\n";

/// A plan the registry admits: two operations, one needing the other.
const REPAIRED: &str = "flow app():\n    let out = graph:\n        node fetch:\n            \"fetching\"\n        node render needs fetch:\n            \"rendering\"\n";

/// The JSON of the repaired plan, as the script map writes it.
const REPAIRED_JSON: &str = "{\"flows\":[{\"steps\":[{\"kind\":\"Graph\",\"subject\":\"sized to the machine\",\"line\":2,\"steps\":[{\"kind\":\"Node\",\"subject\":\"fetch\",\"line\":3,\"steps\":[]},{\"kind\":\"Node\",\"subject\":\"render\",\"line\":5,\"steps\":[]}]}]}]}";

/// Serve the apps routes over a fresh store bounded by `limits`, and run
/// `probe` against the bound address.
fn serve_apps<T>(
    name: &str,
    limits: PlanLimits,
    probe: impl FnOnce(SocketAddr) -> Result<T, String>,
) -> Result<T, String> {
    let scratch = Scratch::new(name).map_err(|error| error.to_string())?;
    let store = Arc::new(AppStore::new(limits));
    let registry = Arc::new(demo_registry()?);
    let held: HeldGrants = Arc::new(|tenant: &str| {
        if tenant == "root" {
            vec![Cap::net()]
        } else {
            Vec::new()
        }
    });
    let routes = apps_routes(store, registry, held);
    with_serve(routes, &scratch, 1_000, |addr, _stop| probe(addr)).map(|(out, _report)| out)
}

/// Log `why` and fail the check with it, so a failing exchange leaves a record.
fn fail<T>(why: String) -> Result<T, String> {
    lgwks_std::trace::debug!(why = why.as_str(), "apps-test: a check failed");
    Err(why)
}

/// The run request for the app id a verdict answers.
fn run_request(verdict: &str) -> Result<String, String> {
    let key = "\"app\":\"";
    let start = verdict
        .find(key)
        .ok_or_else(|| format!("no app id in {verdict:?}"))?;
    let rest = &verdict[start.saturating_add(key.len())..];
    let end = rest
        .find('"')
        .ok_or_else(|| format!("unterminated app id in {verdict:?}"))?;
    Ok(format!("{{\"app\":\"{}\"}}", &rest[..end]))
}

/// Admit the repaired plan for `tenant`, requiring `200`, and answer the run
/// request naming its app id.
fn admit_repaired(addr: SocketAddr, tenant: &str) -> Result<String, String> {
    let (status, _, verdict) = post(addr, tenant, "/apps", REPAIRED.as_bytes(), None)?;
    if status != 200 {
        return fail(format!(
            "{tenant} admitted the repaired plan with {status}, not 200"
        ));
    }
    run_request(verdict.as_str())
}

/// Post `body` to `/apps` for `acme`, require `status`, and require the
/// answer to name every one of `wanted`.
fn refused_naming(
    addr: SocketAddr,
    body: &str,
    status: u16,
    wanted: &[&str],
) -> Result<(), String> {
    let (found, _, answer) = post(addr, "acme", "/apps", body.as_bytes(), None)?;
    if found != status {
        return fail(format!(
            "the plan answered {found}, not {status}: {answer:?}"
        ));
    }
    for name in wanted {
        if !answer.contains(name) {
            return fail(format!("the refusal names no {name}: {answer:?}"));
        }
    }
    Ok(())
}

/// Requirement: a refused plan names its unknown operation and location.
///
/// Proves INV-BOT-188: the full refusal list names `unknown-operation` with
/// the operation and its `line:col`, gathered rather than stopped at the
/// first.
#[test]
fn a_refused_plan_names_its_unknown_operation() -> Result<(), String> {
    serve_apps("apps-refused", PlanLimits::default(), |addr| {
        refused_naming(
            addr,
            REFUSED,
            422,
            &["unknown-operation", "steal", "\"location\":\"5:9\""],
        )
    })
}

/// Requirement: a missing capability names its operation and grant.
///
/// Proves INV-BOT-188: a plan can never name a capability its tenant was
/// not granted, and the refusal says which grant repairs it.
#[test]
fn a_missing_capability_names_its_operation_and_grant() -> Result<(), String> {
    serve_apps("apps-grant", PlanLimits::default(), |addr| {
        refused_naming(
            addr,
            NEEDS_GRANT,
            422,
            &["missing-capability", "preview", "bot.net"],
        )
    })
}

/// Requirement: an oversized body is refused before any parse.
///
/// Proves INV-BOT-188: the byte ceiling is charged before anything is
/// buffered past the limit, answered `413`.
#[test]
fn an_oversized_body_is_413_before_any_parse() -> Result<(), String> {
    serve_apps(
        "apps-wide",
        PlanLimits::default().with_max_bytes(16),
        |addr| refused_naming(addr, REPAIRED, 413, &["bound-exceeded"]),
    )
}

/// Requirement: a repaired plan admits, runs, and prints every line.
///
/// Proves INV-BOT-188: `{"app": "<id>"}` admits, and running prints one
/// line per operation through the graph scheduler.
#[test]
fn a_repaired_plan_admits_and_runs_its_lines() -> Result<(), String> {
    serve_apps("apps-run", PlanLimits::default(), |addr| {
        let asked = admit_repaired(addr, "acme")?;
        let (status, _, body) = post(addr, "acme", "/apps/run", asked.as_bytes(), None)?;
        if status != 200 {
            return fail(format!("a run answered {status}, not 200"));
        }
        let fetch = body
            .find("\"FETCH\"")
            .ok_or_else(|| format!("no fetch line in {body:?}"))?;
        let render = body
            .find("\"redner\"")
            .ok_or_else(|| format!("no render line in {body:?}"))?;
        if render < fetch {
            return fail(format!("the lines print out of order: {body:?}"));
        }
        Ok(())
    })
}

/// Requirement: the JSON body form admits and runs through the edge.
///
/// Proves INV-BOT-188: `POST /apps` decides by `Content-Type` — a JSON body
/// goes to the document reader — and runs the same operations.
#[test]
fn a_json_plan_admits_and_runs_over_http() -> Result<(), String> {
    serve_apps("apps-json", PlanLimits::default(), |addr| {
        let json = [("content-type", "application/json")];
        let (status, _, verdict) =
            post_with(addr, "acme", "/apps", REPAIRED_JSON.as_bytes(), &json, None)?;
        if status != 200 {
            return fail(format!(
                "a JSON plan answered {status}, not 200: {verdict:?}"
            ));
        }
        let asked = run_request(verdict.as_str())?;
        let (status, _, body) = post(addr, "acme", "/apps/run", asked.as_bytes(), None)?;
        if status != 200 || !(body.contains("\"FETCH\"") && body.contains("\"redner\"")) {
            return fail(format!("a JSON run answered {status}: {body:?}"));
        }
        Ok(())
    })
}

/// Requirement: the same plan twice answers the same bytes.
///
/// Proves INV-BOT-188: admission is deterministic — the verdict bytes are
/// identical for one plan and surface.
#[test]
fn the_same_plan_twice_answers_the_same_bytes() -> Result<(), String> {
    serve_apps("apps-twice", PlanLimits::default(), |addr| {
        let first = admit_repaired(addr, "acme")?;
        let second = admit_repaired(addr, "acme")?;
        if first != second {
            return fail(format!(
                "one plan answered two verdicts: {first:?} and {second:?}"
            ));
        }
        Ok(())
    })
}

/// Requirement: two tenants holding one app id stay isolated.
///
/// Proves INV-BOT-188: app ids are scoped by `(tenant, id)` — the second
/// tenant finds nothing until it admits, and then runs its own copy.
#[test]
fn two_tenants_holding_one_app_id_stay_isolated() -> Result<(), String> {
    serve_apps("apps-tenant", PlanLimits::default(), |addr| {
        let asked = admit_repaired(addr, "acme")?;
        let (status, _, _) = post(
            addr,
            "other",
            "/apps/run",
            asked.as_bytes(),
            Some("other-before"),
        )?;
        if status != 404 {
            return fail(format!("the other tenant ran acme's app: {status}"));
        }
        let other = admit_repaired(addr, "other")?;
        if other != asked {
            return fail(format!(
                "tenants gave one plan two ids: {asked:?} and {other:?}"
            ));
        }
        let (status, _, body) = post(
            addr,
            "other",
            "/apps/run",
            asked.as_bytes(),
            Some("other-after"),
        )?;
        if status != 200 || !body.contains("\"FETCH\"") {
            return fail(format!(
                "the other tenant did not run its own copy: {status} {body:?}"
            ));
        }
        let (status, _, body) = post(addr, "acme", "/apps/run", asked.as_bytes(), None)?;
        if status != 200 || !body.contains("\"redner\"") {
            return fail(format!("acme lost its copy: {status} {body:?}"));
        }
        Ok(())
    })
}
