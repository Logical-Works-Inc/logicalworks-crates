//! The structured words' nodes: `graph` and its `node`s, `compensate` inside
//! a `saga`, `admit` and `ready`; `whenever` and `justify` are in `feeds`.
//!
//! A `saga` is a [`Step`](super::Step): a name and a body, read as one; what
//! makes it a saga is the [`Construct::Saga`](super::Construct::Saga) it is
//! wrapped in and the `compensate` lines its body may hold. A `remember` is a
//! `Step` the same way, wrapped in [`Construct::Remember`](super::Construct::Remember).

use proc_macro2::TokenStream;

use super::{Block, Bound, Duration, Site};

/// `graph[, at most (limit) at once]:` and the `node`s under it.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse};
///
/// let script = parse("flow ship(repo: &Repo) -> usize:\n    let report = graph:\n        node build:\n            repo.build().await.or_fail()?\n        node test needs build:\n            repo.test().await.or_fail()?\n    give back report.succeeded().len()\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Graph(graph), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a graph".into());
/// };
/// assert_eq!(graph.nodes().len(), 2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Graph {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The structural label of the graph's scope.
    pub(crate) label: String,
    /// The written bound on nodes running at once; `None` when the runtime
    /// sizes it to the machine.
    pub(crate) bound: Option<Bound>,
    /// The nodes, in the order they are written.
    pub(crate) nodes: Vec<Node>,
}

impl Graph {
    /// The structural label of the graph's scope.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow ship(repo: &Repo) -> usize:\n    let report = graph:\n        node build:\n            repo.build().await.or_fail()?\n        node test needs build:\n            repo.test().await.or_fail()?\n    give back report.succeeded().len()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Graph(graph), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a graph".into());
    /// };
    /// assert_eq!(graph.label(), "graph");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// The written bound on nodes running at once; `None` when the runtime
    /// sizes it to the machine.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow ship(repo: &Repo) -> usize:\n    let report = graph:\n        node build:\n            repo.build().await.or_fail()?\n        node test needs build:\n            repo.test().await.or_fail()?\n    give back report.succeeded().len()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Graph(graph), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a graph".into());
    /// };
    /// assert!(graph.bound().is_none(), "no `at most`, so the machine sizes it");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn bound(&self) -> Option<&Bound> {
        self.bound.as_ref()
    }

    /// The nodes, in the order they are written.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow ship(repo: &Repo) -> usize:\n    let report = graph:\n        node build:\n            repo.build().await.or_fail()?\n        node test needs build:\n            repo.test().await.or_fail()?\n    give back report.succeeded().len()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Graph(graph), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a graph".into());
    /// };
    /// let names: Vec<&str> = graph.nodes().iter().map(|node| node.name()).collect();
    /// assert_eq!(names, ["build", "test"]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn nodes(&self) -> &[Node] {
        self.nodes.as_slice()
    }
}

/// `node <name>[ needs <a>, <b>]:` inside a `graph`.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse};
///
/// let script = parse("flow ship(repo: &Repo) -> usize:\n    let report = graph:\n        node build:\n            repo.build().await.or_fail()?\n        node test needs build:\n            repo.test().await.or_fail()?\n    give back report.succeeded().len()\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Graph(graph), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a graph".into());
/// };
/// let node = graph.nodes().last().ok_or("a node")?;
/// assert_eq!((node.name(), node.needs()), ("test", &["build".to_string()][..]));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Node {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The node's name, unique in its graph; also its scope's label.
    pub(crate) name: String,
    /// The nodes that must succeed before this one starts, as written.
    pub(crate) needs: Vec<String>,
    /// The body, run once.
    pub(crate) body: Block,
}

impl Node {
    /// The node's name, unique in its graph; also its scope's label.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow ship(repo: &Repo) -> usize:\n    let report = graph:\n        node build:\n            repo.build().await.or_fail()?\n        node test needs build:\n            repo.test().await.or_fail()?\n    give back report.succeeded().len()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Graph(graph), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a graph".into());
    /// };
    /// let node = graph.nodes().last().ok_or("a node")?;
    /// assert_eq!(node.name(), "test");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn name(&self) -> &str {
        self.name.as_str()
    }

    /// The nodes that must succeed before this one starts, as written.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow ship(repo: &Repo) -> usize:\n    let report = graph:\n        node build:\n            repo.build().await.or_fail()?\n        node test needs build:\n            repo.test().await.or_fail()?\n    give back report.succeeded().len()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Graph(graph), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a graph".into());
    /// };
    /// let node = graph.nodes().last().ok_or("a node")?;
    /// assert_eq!(node.needs(), ["build"]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn needs(&self) -> &[String] {
        self.needs.as_slice()
    }

    /// The body, run once.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow ship(repo: &Repo) -> usize:\n    let report = graph:\n        node build:\n            repo.build().await.or_fail()?\n        node test needs build:\n            repo.test().await.or_fail()?\n    give back report.succeeded().len()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Graph(graph), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a graph".into());
    /// };
    /// let node = graph.nodes().last().ok_or("a node")?;
    /// assert_eq!(node.body().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn body(&self) -> &Block {
        &self.body
    }
}

/// `compensate <step>:` inside a `saga`: the undo of a finished step.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse};
///
/// let script = parse("flow book(trip: &Trip) -> Ticket:\n    let ticket = saga booking:\n        step charge:\n            trip.charge().await.or_fail()?\n        compensate charge:\n            trip.refund().await.or_fail()?\n    give back ticket\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Saga(saga), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a saga".into());
/// };
/// let Some(Statement::Compensate(undo)) = saga.body().statements().get(1) else {
///     return Err("the saga's second line compensates".into());
/// };
/// assert_eq!(undo.undoes(), "charge");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Compensate {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The step the undo reverses: a `step` written earlier in the saga.
    pub(crate) undoes: String,
    /// The undo itself.
    pub(crate) undo: Block,
}

impl Compensate {
    /// The step the undo reverses: a `step` written earlier in the saga.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow book(trip: &Trip) -> Ticket:\n    let ticket = saga booking:\n        step charge:\n            trip.charge().await.or_fail()?\n        compensate charge:\n            trip.refund().await.or_fail()?\n    give back ticket\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Saga(saga), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a saga".into());
    /// };
    /// let Some(Statement::Compensate(undo)) = saga.body().statements().get(1) else {
    ///     return Err("the saga's second line compensates".into());
    /// };
    /// assert_eq!(undo.undoes(), "charge");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn undoes(&self) -> &str {
        self.undoes.as_str()
    }

    /// The block that reverses the step, run in `undo:<step>` only when a
    /// later line of the saga fails.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow book(trip: &Trip) -> Ticket:\n    let ticket = saga booking:\n        step charge:\n            trip.charge().await.or_fail()?\n        compensate charge:\n            trip.refund().await.or_fail()?\n    give back ticket\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Saga(saga), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a saga".into());
    /// };
    /// let Some(Statement::Compensate(undo)) = saga.body().statements().get(1) else {
    ///     return Err("the saga's second line compensates".into());
    /// };
    /// assert_eq!(undo.undo().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn undo(&self) -> &Block {
        &self.undo
    }
}

/// Who produced the bytes an `admit` reads: the particle after `from`.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse, Producer};
///
/// let script = parse("flow plan(reply: &[u8], gate: &Gate) -> usize:\n    let plan = admit from model through (gate):\n        reply\n    give back plan.wanted().len()\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Admit(admit), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a admit".into());
/// };
/// assert_eq!(admit.producer(), Producer::Model);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Producer {
    /// `from model`: a model's output.
    Model,
    /// `from tool`: a tool's output.
    Tool,
    /// `from document`: a document's text.
    Document,
}

/// `admit from <producer> through (<gate>):`: the block's value is the
/// payload, admitted as a plan.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse};
///
/// let script = parse("flow plan(reply: &[u8], gate: &Gate) -> usize:\n    let plan = admit from model through (gate):\n        reply\n    give back plan.wanted().len()\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Admit(admit), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a admit".into());
/// };
/// assert_eq!(admit.gate().to_string(), "gate");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Admit {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The structural label of the admission's step.
    pub(crate) label: String,
    /// Who produced the payload.
    pub(crate) producer: Producer,
    /// The expression inside the written `( )`, of type `Gate`.
    pub(crate) gate: TokenStream,
    /// The block whose value is the payload's bytes.
    pub(crate) payload: Block,
}

impl Admit {
    /// The structural label of the admission's step.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow plan(reply: &[u8], gate: &Gate) -> usize:\n    let plan = admit from model through (gate):\n        reply\n    give back plan.wanted().len()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Admit(admit), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a admit".into());
    /// };
    /// assert_eq!(admit.label(), "admit");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// Who produced the payload.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse, Producer};
    ///
    /// let script = parse("flow plan(reply: &[u8], gate: &Gate) -> usize:\n    let plan = admit from model through (gate):\n        reply\n    give back plan.wanted().len()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Admit(admit), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a admit".into());
    /// };
    /// assert_eq!(admit.producer(), Producer::Model);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn producer(&self) -> Producer {
        self.producer
    }

    /// The expression inside the written `( )`, of type `Gate`.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow plan(reply: &[u8], gate: &Gate) -> usize:\n    let plan = admit from model through (gate):\n        reply\n    give back plan.wanted().len()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Admit(admit), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a admit".into());
    /// };
    /// assert_eq!(admit.gate().to_string(), "gate");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn gate(&self) -> &TokenStream {
        &self.gate
    }

    /// The block whose value is the payload's bytes.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow plan(reply: &[u8], gate: &Gate) -> usize:\n    let plan = admit from model through (gate):\n        reply\n    give back plan.wanted().len()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Admit(admit), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a admit".into());
    /// };
    /// assert_eq!(admit.payload().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn payload(&self) -> &Block {
        &self.payload
    }
}

/// `ready <pattern> from (<readiness>) within <duration>:`.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse};
///
/// let script = parse("flow rows(db: &Readiness<Pool>) -> u64:\n    let n = ready pool from (db) within 30s:\n        pool.awaited().count()\n    give back n\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Ready(ready), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a ready".into());
/// };
/// assert_eq!(ready.pattern().to_string(), "pool");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Ready {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The structural label of the wait's scope.
    pub(crate) label: String,
    /// The pattern the released value binds.
    pub(crate) pattern: TokenStream,
    /// The expression inside the written `( )`, of type `Readiness`.
    pub(crate) readiness: TokenStream,
    /// The longest the wait may take.
    pub(crate) limit: Duration,
    /// The body run once the readiness releases.
    pub(crate) body: Block,
}

impl Ready {
    /// The structural label of the wait's scope.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow rows(db: &Readiness<Pool>) -> u64:\n    let n = ready pool from (db) within 30s:\n        pool.awaited().count()\n    give back n\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Ready(ready), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a ready".into());
    /// };
    /// assert_eq!(ready.label(), "ready");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// The pattern the released value binds.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow rows(db: &Readiness<Pool>) -> u64:\n    let n = ready pool from (db) within 30s:\n        pool.awaited().count()\n    give back n\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Ready(ready), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a ready".into());
    /// };
    /// assert_eq!(ready.pattern().to_string(), "pool");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn pattern(&self) -> &TokenStream {
        &self.pattern
    }

    /// The expression inside the written `( )`, of type `Readiness`.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow rows(db: &Readiness<Pool>) -> u64:\n    let n = ready pool from (db) within 30s:\n        pool.awaited().count()\n    give back n\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Ready(ready), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a ready".into());
    /// };
    /// assert_eq!(ready.readiness().to_string(), "db");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn readiness(&self) -> &TokenStream {
        &self.readiness
    }

    /// The longest the wait may take.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse, Duration};
    ///
    /// let script = parse("flow rows(db: &Readiness<Pool>) -> u64:\n    let n = ready pool from (db) within 30s:\n        pool.awaited().count()\n    give back n\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Ready(ready), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a ready".into());
    /// };
    /// assert!(matches!(ready.limit(), Duration::Nanos(30_000_000_000)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn limit(&self) -> &Duration {
        &self.limit
    }

    /// The body run once the readiness releases.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow rows(db: &Readiness<Pool>) -> u64:\n    let n = ready pool from (db) within 30s:\n        pool.awaited().count()\n    give back n\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Ready(ready), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a ready".into());
    /// };
    /// assert_eq!(ready.body().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn body(&self) -> &Block {
        &self.body
    }
}

/// `await approval <label> within <duration>:`.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse};
///
/// let script = parse("flow ship(payload: Vec<u8>) -> Vec<u8>:\n    let approved = await approval ship within 30s:\n        payload\n    give back approved.payload().to_vec()\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Approval(approval), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a approval".into());
/// };
/// assert_eq!(approval.name(), "ship");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Approval {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The structural label of the wait's scope.
    pub(crate) label: String,
    /// The written label naming the approval.
    pub(crate) name: String,
    /// The longest the decision may take.
    pub(crate) limit: Duration,
    /// The body whose value is the payload's bytes.
    pub(crate) body: Block,
}

impl Approval {
    /// The structural label of the wait's scope.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow ship(payload: Vec<u8>) -> Vec<u8>:\n    let approved = await approval ship within 30s:\n        payload\n    give back approved.payload().to_vec()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Approval(approval), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a approval".into());
    /// };
    /// assert_eq!(approval.label(), "approval");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// The written label naming the approval.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow ship(payload: Vec<u8>) -> Vec<u8>:\n    let approved = await approval ship within 30s:\n        payload\n    give back approved.payload().to_vec()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Approval(approval), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a approval".into());
    /// };
    /// assert_eq!(approval.name(), "ship");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn name(&self) -> &str {
        self.name.as_str()
    }

    /// The longest the decision may take.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse, Duration};
    ///
    /// let script = parse("flow ship(payload: Vec<u8>) -> Vec<u8>:\n    let approved = await approval ship within 30s:\n        payload\n    give back approved.payload().to_vec()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Approval(approval), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a approval".into());
    /// };
    /// assert!(matches!(approval.limit(), Duration::Nanos(30_000_000_000)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn limit(&self) -> &Duration {
        &self.limit
    }

    /// The body whose value is the payload's bytes.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow ship(payload: Vec<u8>) -> Vec<u8>:\n    let approved = await approval ship within 30s:\n        payload\n    give back approved.payload().to_vec()\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Approval(approval), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a approval".into());
    /// };
    /// assert_eq!(approval.body().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn body(&self) -> &Block {
        &self.body
    }
}
