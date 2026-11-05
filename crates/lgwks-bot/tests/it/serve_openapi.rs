//! `serve` emits an OpenAPI 3.1 document from the route registry (#830).
//!
//! Ports of the kernel openapi suite (`canvas-backend/src/openapi.rs` tests,
//! `tests/router_construction.rs`) and the contract parity boundaries
//! (`tests/architecture/test_contract_parity_boundaries.rs`), re-targeted at
//! the `lgwks_bot` serve route registry: every route carries a descriptor, a
//! route without one fails the check, and the emitted document validates as
//! OpenAPI 3.1. Each test names the requirement it proves in its doc comment.
//! The exchange shape and the counting tools live once in
//! `crate::serve_edge`; the demo apps routes live once in
//! `crate::apps_harness`.

#![cfg(all(feature = "edge", feature = "script"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use lgwks_bot::cap::Cap;
use lgwks_bot::rt::apps::{AppStore, HeldGrants, PlanLimits, apps_routes};
use lgwks_bot::rt::serve::openapi::{
    AuthPosture, DocStatus, RouteDescriptor, check_every_route_described, emit_openapi,
};
use lgwks_bot::rt::serve::{RouteMethod, TypedRoute};
use lgwks_std::json::Value;

use crate::apps_harness::demo_registry;
use crate::scratch::Scratch;
use crate::serve_edge::{counting_route, post, with_serve};

/// The demo apps routes, as the production builder serves them.
fn demo_apps_routes() -> Result<Vec<TypedRoute>, String> {
    let store = Arc::new(AppStore::new(PlanLimits::default()));
    let registry = Arc::new(demo_registry()?);
    let held: HeldGrants = Arc::new(|tenant: &str| {
        if tenant == "root" {
            std::vec![Cap::net()]
        } else {
            std::vec::Vec::new()
        }
    });
    Ok(apps_routes(store, registry, held))
}

/// A stub descriptor for a test route below the tenant prefix.
fn stub_descriptor(path: &str) -> Result<RouteDescriptor, String> {
    RouteDescriptor::new(
        RouteMethod::Post,
        path,
        "serve_openapi::test_route",
        "A test route of the OpenAPI parity suite.",
        AuthPosture::TenantEdge,
        DocStatus::Stub,
    )
    .map_err(|error| format!("the test descriptor is refused: {error}"))
}

/// Render `routes`, or the refusal as the suite reports it.
fn emit(routes: &[TypedRoute]) -> Result<Value, String> {
    emit_openapi(routes).map_err(|error| format!("emit is refused: {error}"))
}

/// A handler answering `ok` to every request.
async fn ok_body(_tenant: String, _body: Vec<u8>) -> Result<Vec<u8>, (u16, String)> {
    Ok(b"ok".to_vec())
}

/// The lowercase OpenAPI method key the emitter renders for `method`.
///
/// A test-local mirror of the emitter's key: an unknown verb renders no
/// lookup, so the operation assertion fails naming the route instead of
/// panicking on a verb the emitter refuses.
fn method_key(method: RouteMethod) -> &'static str {
    match method {
        RouteMethod::Get => "get",
        RouteMethod::Post => "post",
        _ => "unknown",
    }
}

/// The operation `route` renders in `document`, or the refusal naming it.
fn operation_of<'document>(
    document: &'document Value,
    route: &TypedRoute,
) -> Result<&'document Value, String> {
    document
        .get("paths")
        .and_then(|paths| paths.get(route.path()))
        .and_then(|item| item.get(method_key(route.method())))
        .ok_or_else(|| {
            format!(
                "the document names no operation for {} {}",
                route.method().as_str(),
                route.path()
            )
        })
}

/// A described cost-probe route at `path`.
fn cost_route(path: &str) -> Result<TypedRoute, String> {
    let descriptor = RouteDescriptor::new(
        RouteMethod::Post,
        path,
        "serve_openapi::cost_route",
        "A cost-probe route of the OpenAPI parity suite.",
        AuthPosture::TenantEdge,
        DocStatus::Stub,
    )
    .map_err(|error| format!("the cost descriptor is refused: {error}"))?;
    Ok(TypedRoute::new(RouteMethod::Post, path, ok_body).describe(descriptor))
}

/// Requirement: registry parity; every production route carries a descriptor.
///
/// Ports the kernel completeness gate (`ENDPOINTS` versus `build_router`): the
/// serve registry is the routes themselves, and the check refuses the first
/// undescribed one naming its method and path.
#[test]
fn serve_openapi_every_route_has_a_descriptor() -> Result<(), String> {
    let routes = demo_apps_routes()?;
    check_every_route_described(routes.as_slice())
        .map_err(|error| format!("a production route is undescribed: {error}"))?;
    for route in routes.iter() {
        let present = route.descriptor().is_some();
        assert!(
            present,
            "route {} {} carries no descriptor",
            route.method().as_str(),
            route.path()
        );
    }
    Ok(())
}

/// Requirement: one entry per method and path.
///
/// Ports the kernel `no_duplicate_method_path`: two routes answering one verb
/// and path would render one operation shadowing the other.
#[test]
fn serve_openapi_no_duplicate_method_path() -> Result<(), String> {
    let routes = demo_apps_routes()?;
    let mut keys: Vec<(String, String)> = routes
        .iter()
        .map(|route| (route.method().as_str().to_owned(), route.path().to_owned()))
        .collect();
    keys.sort();
    for pair in keys.windows(2) {
        let first = pair
            .first()
            .ok_or_else(|| String::from("a pair window holds no first key"))?;
        let second = pair
            .get(1)
            .ok_or_else(|| String::from("a pair window holds no second key"))?;
        assert!(
            first != second,
            "duplicate route for {} {}",
            first.0,
            first.1
        );
    }
    Ok(())
}

/// Requirement: every registry path is absolute.
///
/// Ports the kernel `every_path_is_absolute`: a relative path renders an
/// OpenAPI document no client generator can import.
#[test]
fn serve_openapi_every_path_is_absolute() -> Result<(), String> {
    let routes = demo_apps_routes()?;
    for route in routes.iter() {
        assert!(
            route.path().starts_with('/'),
            "route path is not absolute: {:?}",
            route.path()
        );
    }
    Ok(())
}

/// Requirement: the emitted document validates as an OpenAPI 3.1 skeleton.
///
/// Ports the kernel `emit_is_valid_openapi_skeleton`: version `3.1.0`, a
/// titled info section, a paths object, at least one method per path item,
/// every operation declaring a non-empty responses map, and exactly one
/// operation per registry route.
#[test]
fn serve_openapi_emit_is_valid_openapi_31_skeleton() -> Result<(), String> {
    let routes = demo_apps_routes()?;
    let document = emit(routes.as_slice())?;
    assert_eq!(
        document.get("openapi").and_then(|version| version.as_str()),
        Some("3.1.0"),
        "the document declares OpenAPI 3.1.0"
    );
    assert!(
        document
            .get("info")
            .and_then(|info| info.get("title"))
            .and_then(|title| title.as_str())
            .is_some(),
        "the document names its title"
    );
    let paths = document
        .get("paths")
        .ok_or_else(|| String::from("the document names no paths"))?;
    let items = paths
        .as_object()
        .ok_or_else(|| String::from("paths is not an object"))?;
    let mut operations = 0usize;
    for item in items.values() {
        let methods = item
            .as_object()
            .ok_or_else(|| String::from("a path item is not an object"))?;
        assert!(!methods.is_empty(), "a path item names no method");
        for operation in methods.values() {
            operations = operations.saturating_add(1);
            let responses = operation
                .get("responses")
                .ok_or_else(|| String::from("an operation names no responses"))?;
            let answered = match responses.as_object() {
                Some(map) => !map.is_empty(),
                None => false,
            };
            assert!(answered, "an operation declares no response");
        }
    }
    assert!(!items.is_empty(), "the document names no path");
    assert_eq!(operations, routes.len(), "one operation per registry route");
    Ok(())
}

/// Requirement: a stub renders a valid placeholder instead of an empty map.
///
/// Ports the kernel `stub_responses_synthesize_a_default`: the apps routes
/// ship as stubs, and every one still carries a `default` response with a
/// description plus its maturity marker.
#[test]
fn serve_openapi_stub_responses_synthesize_a_default() -> Result<(), String> {
    let routes = demo_apps_routes()?;
    let document = emit(routes.as_slice())?;
    for route in routes.iter() {
        let operation = operation_of(&document, route)?;
        assert_eq!(
            operation
                .get("x-doc-status")
                .and_then(|status| status.as_str()),
            Some("stub"),
            "a stub operation carries its maturity marker"
        );
        let default = operation
            .get("responses")
            .and_then(|responses| responses.get("default"))
            .ok_or_else(|| String::from("a stub operation names no default response"))?;
        assert!(
            default
                .get("description")
                .and_then(|text| text.as_str())
                .is_some(),
            "a stub default response carries a description"
        );
    }
    Ok(())
}

/// Requirement: fail-closed parity; an undescribed route is refused.
///
/// Ports the parity gate's blocker shape (`Phase0Blocker` on divergence): the
/// check and the emitter both refuse an undescribed route, naming its method
/// and path, instead of silently dropping it from the document.
#[test]
fn serve_openapi_parity_boundaries_refuse_an_undescribed_route() -> Result<(), String> {
    let undescribed = TypedRoute::new(RouteMethod::Post, "/drafts", ok_body);
    let routes = std::vec![undescribed];
    match check_every_route_described(routes.as_slice()) {
        Ok(()) => {
            return Err(String::from("an undescribed route passed the parity check"));
        }
        Err(error) => {
            let text = error.to_string();
            assert!(
                text.contains("/drafts"),
                "the refusal names the route path: {text}"
            );
        }
    }
    match emit_openapi(routes.as_slice()) {
        Ok(_) => {
            return Err(String::from(
                "an undescribed route was emitted into the document",
            ));
        }
        Err(error) => {
            let text = error.to_string();
            assert!(
                text.contains("/drafts"),
                "the emit refusal names the route path: {text}"
            );
        }
    }
    Ok(())
}

/// Requirement: parity boundaries refuse duplicates, relative paths and drift.
///
/// Ports the parity gate's boundary bands: a descriptor for another path, a
/// relative path and a twice-registered method and path are each a typed
/// refusal, and a descriptor whose verb or path drifted from its route is
/// refused rather than rendered.
#[test]
fn serve_openapi_parity_boundaries_refuse_duplicates_and_relative_paths() -> Result<(), String> {
    let descriptor = stub_descriptor("/drafts")?;
    let first = TypedRoute::new(RouteMethod::Post, "/drafts", ok_body).describe(descriptor);
    let descriptor = stub_descriptor("/drafts")?;
    let second = TypedRoute::new(RouteMethod::Post, "/drafts", ok_body).describe(descriptor);
    let routes = std::vec![first, second];
    match emit_openapi(routes.as_slice()) {
        Ok(_) => {
            return Err(String::from("a duplicated method and path was emitted"));
        }
        Err(error) => {
            let text = error.to_string();
            assert!(
                text.contains("/drafts"),
                "the duplicate refusal names the path: {text}"
            );
        }
    }
    match RouteDescriptor::new(
        RouteMethod::Post,
        "drafts",
        "serve_openapi::test_route",
        "A relative test path.",
        AuthPosture::TenantEdge,
        DocStatus::Stub,
    ) {
        Ok(_) => {
            return Err(String::from("a relative path was admitted"));
        }
        Err(error) => {
            let text = error.to_string();
            assert!(
                text.contains("drafts"),
                "the relative-path refusal names the path: {text}"
            );
        }
    }
    let drifted = stub_descriptor("/other")?;
    let route = TypedRoute::new(RouteMethod::Post, "/drafts", ok_body).describe(drifted);
    let routes = std::vec![route];
    match check_every_route_described(routes.as_slice()) {
        Ok(()) => {
            return Err(String::from(
                "a descriptor drifted from its route passed the check",
            ));
        }
        Err(error) => {
            let text = error.to_string();
            assert!(
                text.contains("/drafts"),
                "the drift refusal names the route path: {text}"
            );
        }
    }
    Ok(())
}

/// Requirement: registry and document agree in both directions.
///
/// Ports the parity gate's two-sided conformance: every registry route renders
/// into the document under its own method and path, and the operation count
/// equals the route count, so no entry exists without a route.
#[test]
fn serve_openapi_parity_document_matches_registry() -> Result<(), String> {
    let routes = demo_apps_routes()?;
    let document = emit(routes.as_slice())?;
    for route in routes.iter() {
        let found = operation_of(&document, route).is_ok();
        assert!(
            found,
            "registry route {} {} renders no operation",
            route.method().as_str(),
            route.path()
        );
    }
    let paths = document
        .get("paths")
        .and_then(|paths| paths.as_object())
        .ok_or_else(|| String::from("the document names no paths object"))?;
    let mut operations = 0usize;
    for item in paths.values() {
        let methods = item
            .as_object()
            .ok_or_else(|| String::from("a path item is not an object"))?;
        operations = operations.saturating_add(methods.len());
    }
    assert_eq!(
        operations,
        routes.len(),
        "no document entry exists without a registry route"
    );
    Ok(())
}

/// Requirement: posture parity; every operation declares its trust boundary.
///
/// Ports the parity gate's security-posture check (divergence is always a
/// blocker): every rendered operation carries `x-auth-posture`, and the
/// mutating tenant-edge routes all read `tenant`. The OpenAPI version itself
/// is the version-parity pin: exactly `3.1.0`, never drifted.
#[test]
fn serve_openapi_every_operation_declares_its_posture() -> Result<(), String> {
    let routes = demo_apps_routes()?;
    let document = emit(routes.as_slice())?;
    for route in routes.iter() {
        let operation = operation_of(&document, route)?;
        assert!(
            operation
                .get("operationId")
                .and_then(|handler| handler.as_str())
                .is_some(),
            "an operation names its handler"
        );
        assert_eq!(
            operation
                .get("x-auth-posture")
                .and_then(|posture| posture.as_str()),
            Some("tenant"),
            "a tenant-edge operation declares its posture"
        );
    }
    Ok(())
}

/// Requirement: acceptable divergence; a stub is documented, not blocked.
///
/// Ports the parity gate's acceptable-divergence verdict (minor drift within
/// policy): routes whose schemas are not yet pinned still emit, marked
/// `x-doc-status: stub`, so the end-pass fills them without changing the
/// generator or the gate.
#[test]
fn serve_openapi_stub_status_is_acceptable_not_a_blocker() -> Result<(), String> {
    let descriptor = stub_descriptor("/drafts")?;
    let route = TypedRoute::new(RouteMethod::Post, "/drafts", ok_body).describe(descriptor);
    let routes = std::vec![route];
    let document =
        emit_openapi(routes.as_slice()).map_err(|error| format!("a stub is blocked: {error}"))?;
    assert_eq!(
        document["paths"]["/drafts"]["post"]
            .get("x-doc-status")
            .and_then(|status| status.as_str()),
        Some("stub"),
        "a stub operation is marked, not blocked"
    );
    Ok(())
}

/// Requirement: the serve router constructs without panicking and dispatches.
///
/// Ports the kernel `router_constructs_without_panic`: axum route-string
/// drift panics at build time, so the test builds the production route set,
/// serves it over loopback, and drives an unknown path (404) plus a typed
/// refusal (unknown app, 404) through the real ingress.
#[test]
fn serve_openapi_router_constructs_and_serves() -> Result<(), String> {
    let routes = demo_apps_routes()?;
    let store = Scratch::new("serve-openapi-router").map_err(|error| error.to_string())?;
    with_serve(routes, &store, 1_000, |addr, _stop| {
        let (missing_status, _, _) = post(addr, "acme", "/nope", b"{}", None)?;
        assert_eq!(
            missing_status, 404,
            "an unknown path is a 404 past a constructed router"
        );
        let (unknown_status, _, _) = post(
            addr,
            "acme",
            "/apps/run",
            b"{\"app\":\"no-such-app\"}",
            None,
        )?;
        assert_eq!(
            unknown_status, 404,
            "an unknown app is a typed 404 through the ingress"
        );
        Ok(())
    })
    .map(|(_, _report)| ())
}

/// Requirement: tenant isolation over one described route set.
///
/// The descriptor carries no tenant data, so two tenants hammering one
/// described route keep their own counts: the parity counterpart of the
/// kernel identity-parity band (different identity classes never share).
#[test]
fn serve_openapi_two_tenants_share_one_described_route_set() -> Result<(), String> {
    let (store, counter) = crate::serve_edge::counted("serve-openapi-isolated")?;
    let route = counting_route(&counter).describe(stub_descriptor("/drafts")?);
    with_serve(std::vec![route], &store, 1_000, |addr, _stop| {
        for tenant in ["acme", "other"] {
            let (status, _, _) = post(addr, tenant, "/drafts", b"{}", None)?;
            assert_eq!(status, 200, "{tenant} is answered through its own host");
        }
        Ok(())
    })
    .map(|(_, _report)| ())?;
    assert_eq!(
        counter.runs(),
        2,
        "both tenants ran once each on the described route"
    );
    Ok(())
}

/// Requirement: registration and emit costs are linear and bounded.
///
/// The registry holds at most 256 routes and emit walks it once; 120
/// described routes must register and emit inside one second, and the 257th
/// route is refused before allocation. The measured durations ride in the
/// failure messages, so a regression prints what it cost.
#[test]
fn serve_openapi_emit_cost_is_linear_and_bounded() -> Result<(), String> {
    let started = Instant::now();
    let mut routes = std::vec::Vec::new();
    for index in 0..120u32 {
        let path = format!("/r{index:03}");
        routes.push(cost_route(path.as_str())?);
    }
    let registered = started.elapsed();
    let ceiling = Duration::from_secs(1);
    assert!(
        registered <= ceiling,
        "registering 120 routes cost {registered:?}, past {ceiling:?}"
    );
    let emitting = Instant::now();
    let document = emit(routes.as_slice())?;
    let emitted = emitting.elapsed();
    assert!(
        emitted <= ceiling,
        "emitting 120 routes cost {emitted:?}, past {ceiling:?}"
    );
    let paths = document
        .get("paths")
        .and_then(|paths| paths.as_object())
        .ok_or_else(|| String::from("the cost document names no paths object"))?;
    assert_eq!(paths.len(), 120, "every cost route renders one path");
    let mut overflow = routes;
    for index in 0..137u32 {
        let path = format!("/o{index:03}");
        overflow.push(cost_route(path.as_str())?);
    }
    match emit_openapi(overflow.as_slice()) {
        Ok(_) => {
            return Err(String::from(
                "a 257-route registry was emitted past its bound",
            ));
        }
        Err(error) => {
            let text = error.to_string();
            assert!(
                text.contains("257"),
                "the bound refusal names the count: {text}"
            );
        }
    }
    Ok(())
}
