//! The `serve` and `route` nodes: the tenant HTTP edge as script words.
//!
//! A `serve` is a [`Construct::Serve`](super::Construct::Serve): the edge's
//! port and in-flight ceiling with its `route`s. A `route` is one typed route
//! of its enclosing `serve`, read like a `node` of a `graph`.

use super::{Block, Bound, Code, Site};

/// Where a `serve` reads its port from.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse, ServePort};
///
/// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a serve".into());
/// };
/// assert!(matches!(serve.port(), ServePort::Env(var) if var == "PORT"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ServePort {
    /// `from env VAR`: the port comes from the named environment variable.
    Env(String),
    /// A port expression written before `,`.
    Expr(Code),
}

/// `serve on port ..., at most N runs in flight:` and its `route`s.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse};
///
/// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a serve".into());
/// };
/// assert_eq!(serve.routes().len(), 1);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
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
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse, RouteMethod};
///
/// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a serve".into());
/// };
/// let route = serve.routes().first().ok_or("a route")?;
/// assert_eq!(route.method(), RouteMethod::Post);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
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
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::RouteMethod;
    ///
    /// assert_eq!(RouteMethod::Get.as_str(), "GET");
    /// assert_eq!(RouteMethod::Post.as_str(), "POST");
    /// ```
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match *self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// `route POST /drafts (body: DraftRequest) -> Draft:` inside a `serve`.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse};
///
/// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a serve".into());
/// };
/// let route = serve.routes().first().ok_or("a route")?;
/// assert_eq!((route.method().as_str(), route.path()), ("POST", "/drafts"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
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
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a serve".into());
    /// };
    /// assert_eq!(serve.label(), "serve");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// Where the port comes from.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse, ServePort};
    ///
    /// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a serve".into());
    /// };
    /// assert!(matches!(serve.port(), ServePort::Env(var) if var == "PORT"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn port(&self) -> &ServePort {
        &self.port
    }

    /// The written bound on runs in flight.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse, Bound};
    ///
    /// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a serve".into());
    /// };
    /// assert!(matches!(serve.bound(), Bound::Count(1000)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn bound(&self) -> &Bound {
        &self.bound
    }

    /// The routes, in the order they are written.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a serve".into());
    /// };
    /// assert_eq!(serve.routes()[0].path(), "/drafts");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn routes(&self) -> &[Route] {
        self.routes.as_slice()
    }
}

impl Route {
    /// The structural label of the route's scope.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a serve".into());
    /// };
    /// let route = serve.routes().first().ok_or("a route")?;
    /// assert_eq!(route.label(), "route:post");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// The verb this route answers.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse, RouteMethod};
    ///
    /// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a serve".into());
    /// };
    /// let route = serve.routes().first().ok_or("a route")?;
    /// assert_eq!(route.method(), RouteMethod::Post);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn method(&self) -> RouteMethod {
        self.method
    }

    /// The path as written: rooted at `/`.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a serve".into());
    /// };
    /// let route = serve.routes().first().ok_or("a route")?;
    /// assert_eq!(route.path(), "/drafts");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn path(&self) -> &str {
        self.path.as_str()
    }

    /// What the edge decodes each request into before the handler runs.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a serve".into());
    /// };
    /// let route = serve.routes().first().ok_or("a route")?;
    /// assert!(!route.body_ty().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn body_ty(&self) -> &Code {
        &self.body_ty
    }

    /// What the handler's answer is encoded as.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a serve".into());
    /// };
    /// let route = serve.routes().first().ok_or("a route")?;
    /// assert!(!route.output_ty().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn output_ty(&self) -> &Code {
        &self.output_ty
    }

    /// The body run for each request.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow drafts() -> u16:\n    let served = serve on port from env PORT, at most 1000 runs in flight:\n        route POST /drafts (body: DraftRequest) -> Draft:\n            give back draft_reply(body).await?\n    give back served.port()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Serve(serve), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a serve".into());
    /// };
    /// let route = serve.routes().first().ok_or("a route")?;
    /// assert_eq!(route.body().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn body(&self) -> &Block {
        &self.body
    }
}
