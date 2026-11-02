//! The `serve` and `route` nodes: the tenant HTTP edge as script words.
//!
//! A `serve` is a [`Construct::Serve`](super::Construct::Serve): the edge's
//! port and in-flight ceiling with its `route`s. A `route` is one typed route
//! of its enclosing `serve`, read like a `node` of a `graph`.

use super::{Block, Bound, Code, Site};

/// Where a `serve` reads its port from.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ServePort {
    /// `from env VAR`: the port comes from the named environment variable.
    Env(String),
    /// A port expression written before `,`.
    Expr(Code),
}

/// `serve on port ..., at most N runs in flight:` and its `route`s.
#[derive(Debug, Clone)]
pub struct Serve {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The structural label of the serve's scope.
    pub(crate) label: String,
    /// Where the port comes from.
    pub(crate) port: ServePort,
    /// The written bound on runs in flight.
    pub(crate) bound: Bound,
    /// The routes, in the order they are written.
    pub(crate) routes: Vec<Route>,
}

/// A route's verb: `GET` or `POST`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RouteMethod {
    /// `GET`.
    Get,
    /// `POST`.
    Post,
}

impl RouteMethod {
    /// The verb as written: `GET` or `POST`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match *self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// `route POST /drafts (body: DraftRequest) -> Draft:` inside a `serve`.
#[derive(Debug, Clone)]
pub struct Route {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The structural label of the route's scope.
    pub(crate) label: String,
    /// The verb this route answers.
    pub(crate) method: RouteMethod,
    /// The path as written: rooted at `/`.
    pub(crate) path: String,
    /// The tokens inside `(body: Type)`.
    pub(crate) body_ty: Code,
    /// The output type after `->`.
    pub(crate) output_ty: Code,
    /// The body run for each request.
    pub(crate) body: Block,
}

impl Serve {
    /// The structural label of the serve's scope.
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// Where the port comes from.
    #[must_use]
    pub const fn port(&self) -> &ServePort {
        &self.port
    }

    /// The written bound on runs in flight.
    #[must_use]
    pub const fn bound(&self) -> &Bound {
        &self.bound
    }

    /// The routes, in the order they are written.
    #[must_use]
    pub const fn routes(&self) -> &[Route] {
        self.routes.as_slice()
    }
}

impl Route {
    /// The structural label of the route's scope.
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// The verb this route answers.
    #[must_use]
    pub const fn method(&self) -> RouteMethod {
        self.method
    }

    /// The path as written: rooted at `/`.
    #[must_use]
    pub const fn path(&self) -> &str {
        self.path.as_str()
    }

    /// What the edge decodes each request into before the handler runs.
    #[must_use]
    pub const fn body_ty(&self) -> &Code {
        &self.body_ty
    }

    /// What the handler's answer is encoded as.
    #[must_use]
    pub const fn output_ty(&self) -> &Code {
        &self.output_ty
    }

    /// The body run for each request.
    #[must_use]
    pub const fn body(&self) -> &Block {
        &self.body
    }
}
