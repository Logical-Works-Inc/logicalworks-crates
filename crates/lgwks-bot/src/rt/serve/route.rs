//! One typed route of the tenant edge: the verb, the path below the tenant
//! prefix, and the handler from a request to response bytes.
//!
//! Every handler has one shape — tenant, `Content-Type` when sent, body — so
//! the edge calls one function whatever the route was built with: a plain
//! [`TypedRoute::new`] handler simply ignores the content type.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::RouteMethod;
use super::openapi::RouteDescriptor;

/// The boxed answer a typed route handler returns.
pub(super) type BoxRouteFuture =
    Pin<Box<dyn Future<Output = Result<Vec<u8>, (u16, String)>> + Send>>;

/// From tenant, content type and body bytes to response bytes, or
/// `(status, message)`.
pub(super) type RouteHandler =
    dyn Fn(String, Option<String>, Vec<u8>) -> BoxRouteFuture + Send + Sync;

/// One typed route: the verb, the path below the tenant prefix, and the
/// handler from tenant, content type and body bytes to response bytes.
#[derive(Clone)]
pub struct TypedRoute {
    /// The verb this route answers.
    pub(super) method: RouteMethod,
    /// Rooted at `/`, as the edge serves it.
    pub(super) path: String,
    /// From tenant, content type and body bytes to response bytes.
    pub(super) handler: Arc<RouteHandler>,
    /// The contract the OpenAPI projection renders, when one was attached.
    descriptor: Option<RouteDescriptor>,
}

impl TypedRoute {
    /// Answer `method` `path` with `handler`.
    ///
    /// The handler receives the request's tenant (never the body) and its
    /// body bytes, and answers the response bytes (a JSON document) or the
    /// status and message to answer (a `400` for a malformed body, `500` for
    /// a failed handler).
    pub fn new<F, Fut>(method: RouteMethod, path: &str, handler: F) -> Self
    where
        F: Fn(String, Vec<u8>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Vec<u8>, (u16, String)>> + Send + 'static,
    {
        Self::new_with_content_type(method, path, move |tenant, _content, body| {
            handler(tenant, body)
        })
    }

    /// Answer `method` `path` with `handler`, which also sees the request's
    /// `Content-Type` value when one was sent: what a route reading two body
    /// forms off one path, like the model-written plan edge, dispatches on.
    pub(crate) fn new_with_content_type<F, Fut>(method: RouteMethod, path: &str, handler: F) -> Self
    where
        F: Fn(String, Option<String>, Vec<u8>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Vec<u8>, (u16, String)>> + Send + 'static,
    {
        let erased = Arc::new(
            move |tenant: String, content: Option<String>, body: Vec<u8>| {
                let boxed: BoxRouteFuture = Box::pin(handler(tenant, content, body));
                boxed
            },
        );
        Self {
            method,
            path: path.to_owned(),
            handler: erased,
            descriptor: None,
        }
    }

    /// Attach `descriptor` to this route, returning it for chaining.
    ///
    /// The parity check refuses a route with no descriptor and one whose
    /// descriptor names another verb or path, so attaching is how a route
    /// joins the emitted document.
    #[must_use]
    pub fn describe(mut self, descriptor: RouteDescriptor) -> Self {
        self.descriptor = Some(descriptor);
        self
    }

    /// The descriptor this route carries, when one was attached.
    #[must_use]
    pub fn descriptor(&self) -> Option<&RouteDescriptor> {
        self.descriptor.as_ref()
    }

    /// The verb this route answers.
    #[must_use]
    pub const fn method(&self) -> RouteMethod {
        self.method
    }

    /// The path below the tenant prefix.
    #[must_use]
    pub fn path(&self) -> &str {
        self.path.as_str()
    }
}
