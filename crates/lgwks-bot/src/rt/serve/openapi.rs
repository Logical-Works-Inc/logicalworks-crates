//! The OpenAPI 3.1 projection of the serve route registry (#830).
//!
//! Every [`TypedRoute`](crate::rt::serve::TypedRoute) carries a
//! [`RouteDescriptor`](crate::rt::serve::openapi::RouteDescriptor);
//! [`emit_openapi`](crate::rt::serve::openapi::emit_openapi) renders the
//! described set as an OpenAPI 3.1 document, and
//! [`check_every_route_described`](crate::rt::serve::openapi::check_every_route_described)
//! refuses the first route that carries none or whose descriptor drifted from
//! it. The registry is authoritative and the document is a projection of it,
//! never a second registry: an undescribed, drifted, duplicated, relative or
//! over-ceiling route is a typed refusal, and the emitter refuses it rather
//! than silently dropping it.
//!
//! A stub entry (every production route until its schemas are pinned) renders
//! a synthesized placeholder response marked `x-doc-status: stub`, so the
//! emitted document stays structurally valid while the end-pass fills real
//! schemas in. Filling a stub is additive: the descriptor grows a response
//! list and the generator renders it; the parity checks do not change.

use lgwks_std::json::{Map, Value, json};

use super::RouteMethod;
use super::TypedRoute;

/// The most routes one document renders.
const MAX_ROUTES: usize = 256;

/// The most bytes a descriptor path holds.
const MAX_PATH_BYTES: usize = 256;

/// The most bytes a descriptor handler name holds.
const MAX_HANDLER_BYTES: usize = 256;

/// The most bytes a descriptor summary holds.
const MAX_SUMMARY_BYTES: usize = 512;

/// Refuse with `error`, logged as every refusal is.
fn refused<T>(error: OpenApiError) -> Result<T, OpenApiError> {
    lgwks_std::trace::debug!(error = ?error, "openapi: the route set is refused");
    Err(error)
}

/// Where a route sits behind the tenant edge, rendered as `x-auth-posture`.
///
/// # Examples
///
/// ```text
/// x-auth-posture: "tenant"
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthPosture {
    /// Tenant-scoped: the tenant comes from the request path, never the body.
    TenantEdge,
    /// Unauthenticated discovery surface carrying no tenant data.
    Discovery,
}

impl AuthPosture {
    /// The wire form rendered into the document.
    fn as_str(self) -> &'static str {
        match self {
            Self::TenantEdge => "tenant",
            Self::Discovery => "discovery",
        }
    }
}

/// Whether a route's documentation is pinned yet.
///
/// # Examples
///
/// ```text
/// x-doc-status: "stub"
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocStatus {
    /// Schemas not yet pinned; renders a synthesized placeholder response.
    Stub,
    /// Schemas pinned; renders the documented shape.
    Documented,
}

impl DocStatus {
    /// The wire form rendered into the document.
    fn as_str(self) -> &'static str {
        match self {
            Self::Stub => "stub",
            Self::Documented => "documented",
        }
    }
}

/// Why a route set cannot become an OpenAPI document.
///
/// # Examples
///
/// ```text
/// undescribed route GET /health: every route must carry a descriptor
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpenApiError {
    /// A route carries no descriptor.
    UndescribedRoute {
        /// The verb the route answers.
        method: String,
        /// The path below the tenant prefix.
        path: String,
    },
    /// A descriptor names another route's verb or path.
    MismatchedDescriptor {
        /// The verb the route answers.
        route_method: String,
        /// The path the route answers.
        route_path: String,
        /// The verb the descriptor names.
        descriptor_method: String,
        /// The path the descriptor names.
        descriptor_path: String,
    },
    /// A descriptor path is not rooted at `/`.
    RelativePath {
        /// The offending path.
        path: String,
    },
    /// A descriptor field is empty or past its byte ceiling.
    BadField {
        /// Which field refused.
        field: &'static str,
        /// Its length in bytes.
        len: usize,
        /// The ceiling refused against.
        max: usize,
    },
    /// One verb and path is registered twice.
    DuplicateRoute {
        /// The verb answered twice.
        method: String,
        /// The path answered twice.
        path: String,
    },
    /// More than `MAX_ROUTES` routes were offered.
    TooManyRoutes {
        /// How many routes were offered.
        len: usize,
        /// The ceiling refused against.
        max: usize,
    },
    /// A route answers a verb the document cannot render.
    UnsupportedMethod {
        /// The verb as written.
        method: String,
    },
}

impl std::fmt::Display for OpenApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::UndescribedRoute {
                ref method,
                ref path,
            } => {
                write!(
                    formatter,
                    "lgwks_bot: the route {method} {path} carries no descriptor"
                )
            }
            Self::MismatchedDescriptor {
                ref route_method,
                ref route_path,
                ref descriptor_method,
                ref descriptor_path,
            } => {
                write!(
                    formatter,
                    "lgwks_bot: the descriptor {descriptor_method} {descriptor_path} drifted from its route {route_method} {route_path}"
                )
            }
            Self::RelativePath { ref path } => {
                write!(
                    formatter,
                    "lgwks_bot: the descriptor path is not absolute: {path:?}"
                )
            }
            Self::BadField { field, len, max } => {
                write!(
                    formatter,
                    "lgwks_bot: the descriptor {field} is {len} bytes, past the ceiling {max}"
                )
            }
            Self::DuplicateRoute {
                ref method,
                ref path,
            } => {
                write!(
                    formatter,
                    "lgwks_bot: the route {method} {path} is registered twice"
                )
            }
            Self::TooManyRoutes { len, max } => {
                write!(
                    formatter,
                    "lgwks_bot: {len} routes were offered, past the ceiling {max}"
                )
            }
            Self::UnsupportedMethod { ref method } => {
                write!(
                    formatter,
                    "lgwks_bot: the route answers a verb the document cannot render: {method}"
                )
            }
        }
    }
}

impl std::error::Error for OpenApiError {}

/// One route's contract: the verb, path, handler, summary and posture the
/// emitted document renders for it.
///
/// # Examples
///
/// ```text
/// GET /health -> handler: "health", posture: "discovery"
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RouteDescriptor {
    /// The verb the route answers; must match the route's own verb.
    method: RouteMethod,
    /// The path below the tenant prefix; must match the route's own path.
    path: String,
    /// The handler identity rendered as the operationId.
    handler: String,
    /// One human sentence rendered as the summary.
    summary: String,
    /// The trust boundary rendered as `x-auth-posture`.
    auth: AuthPosture,
    /// The maturity rendered as `x-doc-status`.
    status: DocStatus,
}

impl RouteDescriptor {
    /// Describe `method` `path`, answered by `handler` and summarized by
    /// `summary` under `auth` at maturity `status`.
    ///
    /// # Examples
    ///
    /// ```text
    /// RouteDescriptor::new(Get, "/status", "status", "Status.", Discovery, Stub)
    /// ```
    ///
    /// # Errors
    ///
    /// [`OpenApiError::BadField`] when a field is empty or past its byte
    /// ceiling; [`OpenApiError::RelativePath`] when the path is not rooted
    /// at `/`.
    pub fn new(
        method: RouteMethod,
        path: &str,
        handler: &str,
        summary: &str,
        auth: AuthPosture,
        status: DocStatus,
    ) -> Result<Self, OpenApiError> {
        let path_len = path.len();
        if path_len == 0 || path_len > MAX_PATH_BYTES {
            return refused(OpenApiError::BadField {
                field: "path",
                len: path_len,
                max: MAX_PATH_BYTES,
            });
        }
        if !path.starts_with('/') {
            return refused(OpenApiError::RelativePath {
                path: path.to_owned(),
            });
        }
        let handler_len = handler.len();
        if handler_len == 0 || handler_len > MAX_HANDLER_BYTES {
            return refused(OpenApiError::BadField {
                field: "handler",
                len: handler_len,
                max: MAX_HANDLER_BYTES,
            });
        }
        let summary_len = summary.len();
        if summary_len == 0 || summary_len > MAX_SUMMARY_BYTES {
            return refused(OpenApiError::BadField {
                field: "summary",
                len: summary_len,
                max: MAX_SUMMARY_BYTES,
            });
        }
        Ok(Self {
            method,
            path: path.to_owned(),
            handler: handler.to_owned(),
            summary: summary.to_owned(),
            auth,
            status,
        })
    }

    /// The verb the descriptor names.
    ///
    /// # Examples
    ///
    /// ```text
    /// descriptor.method() == RouteMethod::Get
    /// ```
    #[must_use]
    pub const fn method(&self) -> RouteMethod {
        self.method
    }

    /// The path the descriptor names.
    ///
    /// # Examples
    ///
    /// ```text
    /// descriptor.path() == "/health"
    /// ```
    #[must_use]
    pub fn path(&self) -> &str {
        self.path.as_str()
    }
}

/// The lowercase form of `method` used as an OpenAPI path-item key.
///
/// # Errors
///
/// [`OpenApiError::UnsupportedMethod`] for a verb added after `GET` and
/// `POST`; the document refuses it rather than rendering a key no client
/// generator reads.
fn operation_key(method: RouteMethod) -> Result<&'static str, OpenApiError> {
    match method {
        RouteMethod::Get => Ok("get"),
        RouteMethod::Post => Ok("post"),
        _ => refused(OpenApiError::UnsupportedMethod {
            method: method.as_str().to_owned(),
        }),
    }
}

/// The descriptor `route` carries, or the refusal naming the route.
fn described(route: &TypedRoute) -> Result<&RouteDescriptor, OpenApiError> {
    match route.descriptor() {
        Some(descriptor) => Ok(descriptor),
        None => refused(OpenApiError::UndescribedRoute {
            method: route.method().as_str().to_owned(),
            path: route.path().to_owned(),
        }),
    }
}

/// Refuse the first route that carries no descriptor, or whose descriptor
/// names another verb or path.
///
/// # Examples
///
/// ```text
/// check_every_route_described(&routes)?;
/// ```
///
/// # Errors
///
/// [`OpenApiError::UndescribedRoute`] for a route with no descriptor;
/// [`OpenApiError::MismatchedDescriptor`] for a descriptor drifted from its
/// route.
pub fn check_every_route_described(routes: &[TypedRoute]) -> Result<(), OpenApiError> {
    for route in routes.iter() {
        let descriptor = described(route)?;
        if descriptor.method() != route.method() || descriptor.path() != route.path() {
            return refused(OpenApiError::MismatchedDescriptor {
                route_method: route.method().as_str().to_owned(),
                route_path: route.path().to_owned(),
                descriptor_method: descriptor.method().as_str().to_owned(),
                descriptor_path: descriptor.path().to_owned(),
            });
        }
    }
    Ok(())
}

/// Render `routes` as an OpenAPI 3.1 document.
///
/// # Examples
///
/// ```json
/// { "openapi": "3.1.0", "info": { "title": "API" }, "paths": {} }
/// ```
///
/// # Errors
///
/// Whatever [`check_every_route_described`] refuses, plus
/// [`OpenApiError::TooManyRoutes`] past `MAX_ROUTES` and
/// [`OpenApiError::DuplicateRoute`] for a verb and path registered twice.
pub fn emit_openapi(routes: &[TypedRoute]) -> Result<Value, OpenApiError> {
    if routes.len() > MAX_ROUTES {
        return refused(OpenApiError::TooManyRoutes {
            len: routes.len(),
            max: MAX_ROUTES,
        });
    }
    check_every_route_described(routes)?;
    let mut seen: Vec<(String, String)> = Vec::with_capacity(routes.len());
    for route in routes.iter() {
        let key = (route.method().as_str().to_owned(), route.path().to_owned());
        if seen.contains(&key) {
            let (method, path) = key;
            return refused(OpenApiError::DuplicateRoute { method, path });
        }
        seen.push(key);
    }
    let mut path_items: std::collections::BTreeMap<String, Map> = std::collections::BTreeMap::new();
    let mut documented = 0usize;
    for route in routes.iter() {
        let descriptor = described(route)?;
        if descriptor.status == DocStatus::Documented {
            documented = documented.saturating_add(1);
        }
        let mut responses: Map = Map::new();
        match descriptor.status {
            DocStatus::Stub => {
                responses.insert(
                    "default".to_owned(),
                    json!({"description": "Not yet documented (stub)."}),
                );
            }
            DocStatus::Documented => {
                responses.insert("200".to_owned(), json!({"description": "Documented."}));
            }
        }
        let mut operation: Map = Map::new();
        operation.insert(
            "summary".to_owned(),
            Value::from(descriptor.summary.as_str()),
        );
        operation.insert(
            "operationId".to_owned(),
            Value::from(descriptor.handler.as_str()),
        );
        operation.insert(
            "x-auth-posture".to_owned(),
            Value::from(descriptor.auth.as_str()),
        );
        operation.insert(
            "x-doc-status".to_owned(),
            Value::from(descriptor.status.as_str()),
        );
        operation.insert("responses".to_owned(), Value::Object(responses));
        path_items
            .entry(descriptor.path.clone())
            .or_default()
            .insert(
                operation_key(descriptor.method)?.to_owned(),
                Value::Object(operation),
            );
    }
    let mut paths: Map = Map::new();
    for (path, item) in path_items {
        paths.insert(path, Value::Object(item));
    }
    let total = routes.len();
    Ok(json!({
        "openapi": "3.1.0",
        "info": {
            "title": "lgwks_bot serve edge API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": format!(
                "The tenant HTTP edge of lgwks_bot. Source of truth: the serve route registry. {documented}/{total} operations fully documented; the remainder are stubs."
            ),
        },
        "paths": Value::Object(paths),
        "components": {"schemas": {}},
    }))
}
