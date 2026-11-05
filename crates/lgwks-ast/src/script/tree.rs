//! The script tree [`parse`](super::parse) answers with.
//!
//! Every word is already read: a bound is a number or the expression the
//! runtime checks, a duration is nanoseconds or an expression of type
//! `Duration`, every step carries its structural label, and every `run` call
//! inside a line of Rust is a [`Call`]. A caller generating code or drawing a
//! map works from these values and never reads a token of the language again,
//! which is what keeps the macro and every tool on one reading (SL-2).
//!
//! Only the parser builds these values, so their fields are private to this
//! crate and read through accessors; the enums are `#[non_exhaustive]`, so the
//! language can grow a word without a caller's exhaustive match compiling into
//! a silent misreading.

use std::fmt;

use proc_macro2::{Delimiter, Span, TokenStream, TokenTree};

use super::lexicon::Kind;

mod blocks;
mod branch;
mod calls;
mod feeds;
mod map;
mod serve;
mod shape;
mod vocab;

pub use blocks::{Block, Each, Flow, For, Retry, Step, Within};
pub use branch::{Branch, IfChain, Together};
pub use calls::{Domain, Outside};
pub use feeds::{Justify, Overflow, Whenever};
pub use map::{MapStep, write_map};
pub use serve::{Route, RouteMethod, Serve, ServePort};
pub use shape::{FlowShape, Passthrough, StepShape};
pub use vocab::{Admit, Approval, Compensate, Graph, Node, Producer, Ready};

/// A whole `script!` block: its flows, in source order.
///
/// # Examples
/// ```
/// use lgwks_ast::script::parse;
///
/// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
/// assert_eq!(script.flows().len(), 1);
/// assert!(script.to_string().starts_with("flow sizes(pages: Vec<String>) -> Vec<usize>  @1"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Script {
    /// Every flow the block declares.
    pub(crate) flows: Vec<Flow>,
}

impl Script {
    /// Every flow the block declares.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// assert_eq!(script.flows()[0].shape().name(), "sizes");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn flows(&self) -> &[Flow] {
        self.flows.as_slice()
    }
}

impl fmt::Display for Script {
    /// The architecture map, through [`write_map`]: the one rendering the
    /// runtime's `Architecture` uses for the `ARCHITECTURE` the macro emits.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_map(
            formatter,
            self.flows.iter().map(|flow| {
                (
                    flow.shape.signature.as_str(),
                    flow.shape.line,
                    flow.shape.steps.as_slice(),
                )
            }),
        )
    }
}

/// One branch of an `if` chain.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Statement, parse};
///
/// let script = parse("flow route(code: u16) -> u8:\n    if code < 400:\n        give back 0\n    else if code < 500:\n        give back 1\n    else:\n        give back 2\n".parse()?)?;
/// let Some(Statement::If(chain)) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is an if chain".into());
/// };
/// let first = &chain.branches()[0];
/// assert!(first.condition().is_some());
/// assert_eq!(first.body().statements().len(), 1);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct IfBranch {
    /// The condition; `None` for the final `else:`.
    pub(crate) condition: Option<Code>,
    /// The branch's body.
    pub(crate) body: Block,
}

impl IfBranch {
    /// The condition; `None` for the final `else:`.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow route(code: u16) -> u8:\n    if code < 400:\n        give back 0\n    else if code < 500:\n        give back 1\n    else:\n        give back 2\n".parse()?)?;
    /// let Some(Statement::If(chain)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an if chain".into());
    /// };
    /// assert!(chain.branches()[0].condition().is_some());
    /// assert!(chain.branches()[2].condition().is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn condition(&self) -> Option<&Code> {
        self.condition.as_ref()
    }

    /// The statements this branch runs when it is taken.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow route(code: u16) -> u8:\n    if code < 400:\n        give back 0\n    else if code < 500:\n        give back 1\n    else:\n        give back 2\n".parse()?)?;
    /// let Some(Statement::If(chain)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an if chain".into());
    /// };
    /// assert!(matches!(chain.branches()[0].body().statements()[0], Statement::GiveBack(_)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn body(&self) -> &Block {
        &self.body
    }
}

/// A line of Rust with its `run`, `observe` and `act` calls read out of it.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Statement, parse};
///
/// let script = parse("flow outer(site: &Site):\n    run crawl::pages(site)\n".parse()?)?;
/// let Some(Statement::Run(call)) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is a run".into());
/// };
/// assert!(!call.arguments().is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Default)]
pub struct Code {
    /// The line's tokens, in order, with every `run`, `observe` and `act` call
    /// one fragment.
    pub(crate) fragments: Vec<Fragment>,
}

impl Code {
    /// The line's tokens, in order, with every `run`, `observe` and `act` call
    /// one fragment.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow outer(site: &Site):\n    run crawl::pages(site)\n".parse()?)?;
    /// let Some(Statement::Run(call)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a run".into());
    /// };
    /// assert_eq!(call.arguments().fragments().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn fragments(&self) -> &[Fragment] {
        self.fragments.as_slice()
    }
}

/// `run path(args)`: another flow called in this scope.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Statement, parse};
///
/// let script = parse("flow outer(site: &Site):\n    run crawl::pages(site)\n".parse()?)?;
/// let Some(Statement::Run(call)) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is a run".into());
/// };
/// assert_eq!((call.callee(), call.label()), ("pages", "pages"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Call {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The callee's path as written, `::` and all.
    pub(crate) path: TokenStream,
    /// The callee's last segment, which names its scope.
    pub(crate) callee: String,
    /// The structural label: the callee, numbered among same-named calls in one scope. A call whose label is not the callee's name enters a scope of that label, so two calls never share a key.
    pub(crate) label: String,
    /// The arguments.
    pub(crate) arguments: Code,
}

impl Call {
    /// The callee's path as written, `::` and all.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow outer(site: &Site):\n    run crawl::pages(site)\n".parse()?)?;
    /// let Some(Statement::Run(call)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a run".into());
    /// };
    /// assert_eq!(call.path().to_string(), "crawl :: pages");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn path(&self) -> &TokenStream {
        &self.path
    }

    /// The callee's last segment, which names its scope.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow outer(site: &Site):\n    run crawl::pages(site)\n".parse()?)?;
    /// let Some(Statement::Run(call)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a run".into());
    /// };
    /// assert_eq!(call.callee(), "pages");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn callee(&self) -> &str {
        self.callee.as_str()
    }

    /// The structural label: the callee, numbered among same-named calls in one scope. A call whose label is not the callee's name enters a scope of that label, so two calls never share a key.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow outer(site: &Site):\n    run crawl::pages(site)\n".parse()?)?;
    /// let Some(Statement::Run(call)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a run".into());
    /// };
    /// assert_eq!(call.label(), "pages");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// What is passed to the callee, between its parentheses.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow outer(site: &Site):\n    run crawl::pages(site)\n".parse()?)?;
    /// let Some(Statement::Run(call)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a run".into());
    /// };
    /// assert_eq!(call.arguments().fragments().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn arguments(&self) -> &Code {
        &self.arguments
    }
}

/// Where a step stands in the script: its word, its source line and the line
/// as written.
///
/// Every construct that enters a scope at run time carries one, so the trail a
/// run leaves points each entry back at the line that produced it (SL-6, #386).
/// It is never part of a step label. Labels are structural: a line number in
/// one would change every key below a line inserted above it, and a run resumed
/// across that deploy would replay nothing.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Kind, Statement, parse};
///
/// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is an each".into());
/// };
/// let site = each.site();
/// assert_eq!((site.kind(), site.line()), (Kind::Each, 2));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// The word the line starts with.
    pub(crate) kind: Kind,
    /// The 1-based source line, as the compiler reports it.
    pub(crate) line: usize,
    /// The line as written, without its `:`.
    pub(crate) text: String,
}

impl Site {
    /// The site of the step `shape` maps: the same word, line and text, so the
    /// map and the trail cannot describe one line two ways.
    pub(crate) fn of(shape: &StepShape) -> Self {
        Self {
            kind: shape.kind,
            line: shape.line,
            text: shape.detail.clone(),
        }
    }

    /// The word the line starts with.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Kind, Statement, parse};
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an each".into());
    /// };
    /// assert_eq!(each.site().kind(), Kind::Each);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn kind(&self) -> Kind {
        self.kind
    }

    /// The 1-based source line, as the compiler reports it.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an each".into());
    /// };
    /// assert_eq!(each.site().line(), 2);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }

    /// The line as written, without its `:`.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an each".into());
    /// };
    /// assert_eq!(each.site().text(), "each page in pages");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn text(&self) -> &str {
        self.text.as_str()
    }
}

/// `site()` for every construct that enters a scope at run time, declared once:
/// the field is the same on each, and so is what reading it means.
macro_rules! site_of {
    ($($construct:ident),* $(,)?) => {$(
        impl $construct {
            /// Where this construct's line stands in the script.
            #[must_use]
            pub const fn site(&self) -> &Site {
                &self.site
            }
        }
    )*};
}

site_of!(
    Each, Within, Retry, Step, For, Call, Domain, Outside, Graph, Node, Compensate, Whenever,
    Justify, Admit, Ready, Approval, Serve, Route,
);

impl Block {
    /// Whether the last line always leaves the block, so no value follows it.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow route(code: u16) -> u8:\n    if code < 400:\n        give back 0\n    else if code < 500:\n        give back 1\n    else:\n        give back 2\n".parse()?)?;
    /// let Some(Statement::If(chain)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an if chain".into());
    /// };
    /// let body = script.flows()[0].body();
    /// assert!(body.diverges(), "every branch of its last line gives back");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn diverges(&self) -> bool {
        self.statements.last().is_some_and(Statement::diverges)
    }

    /// The statement that is the block's value: the last one, when the block
    /// wants a value and that statement is an expression.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an each".into());
    /// };
    /// assert!(each.body().tail().is_some(), "`page.len()` is the body's value");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn tail(&self) -> Option<&Statement> {
        self.statements
            .last()
            .filter(|last| self.wants_value && last.is_expression())
    }
}

impl Code {
    /// Whether the code has no tokens at all.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow outer(site: &Site):\n    run crawl::pages(site)\n".parse()?)?;
    /// let Some(Statement::Run(call)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a run".into());
    /// };
    /// assert!(!call.arguments().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }
}

/// One line of a block, or one `if` chain.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Statement, parse};
///
/// let script = parse("/// How many items.\npub flow count(items: Vec<u8>) -> usize:\n    give back items.len()\n".parse()?)?;
/// let line = &script.flows()[0].body().statements()[0];
/// assert!(matches!(line, Statement::GiveBack(_)));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Statement {
    /// A line of Rust passed through as written, with its `run` calls read.
    Rust(Code),
    /// A `let` line that opens no block: Rust, ending its statement.
    Let(Code),
    /// A line that is exactly one `run` call, whose result is the line's.
    Run(Call),
    /// `let <pattern> = <block word>:`: the block's value, bound.
    Bind {
        /// The pattern before the `=`.
        pattern: TokenStream,
        /// The block whose value is bound.
        block: Construct,
    },
    /// A block word whose result is the line's.
    Construct(Construct),
    /// `together:` and its branches.
    Together(Together),
    /// `for <pattern> in <items>:`.
    For(For),
    /// `if cond:` with its `else if cond:` and `else:` branches.
    If(IfChain),
    /// `give back <value>`: leaves the flow with the value.
    GiveBack(Code),
    /// `fail [transiently] with <reason>`: leaves the flow with a typed error.
    Fail {
        /// Whether the failure is worth repeating (`fail transiently`).
        transient: bool,
        /// The reason, as written.
        reason: Code,
    },
    /// `compensate <step>:` inside a `saga`: registers the undo of a step.
    Compensate(Compensate),
}

impl Statement {
    /// Whether the statement is an expression that could be a block's value.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an each".into());
    /// };
    /// assert!(each.body().statements()[0].is_expression());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn is_expression(&self) -> bool {
        match *self {
            Self::Rust(_) | Self::Run(_) | Self::Construct(_) => true,
            Self::If(ref chain) => !chain.leaves && chain.wants_value,
            Self::Let(_)
            | Self::Bind { .. }
            | Self::Together(_)
            | Self::For(_)
            | Self::GiveBack(_)
            | Self::Fail { .. }
            | Self::Compensate(_) => false,
        }
    }

    /// Whether the statement always leaves the block it is in.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("/// How many items.\npub flow count(items: Vec<u8>) -> usize:\n    give back items.len()\n".parse()?)?;
    /// assert!(script.flows()[0].body().statements()[0].diverges());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn diverges(&self) -> bool {
        match *self {
            Self::GiveBack(_) | Self::Fail { .. } => true,
            Self::If(ref chain) => chain.leaves,
            Self::Rust(_)
            | Self::Let(_)
            | Self::Run(_)
            | Self::Bind { .. }
            | Self::Construct(_)
            | Self::Together(_)
            | Self::For(_)
            | Self::Compensate(_) => false,
        }
    }

    /// Whether the statement's value is a flow result that needs `?` to
    /// become the value: a block word or a `run`.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow outer(site: &Site):\n    run crawl::pages(site)\n".parse()?)?;
    /// let Some(Statement::Run(call)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a run".into());
    /// };
    /// assert!(script.flows()[0].body().statements()[0].is_fallible());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn is_fallible(&self) -> bool {
        matches!(*self, Self::Run(_) | Self::Construct(_))
    }
}

/// A block word with its own async body: `each`, `within`, `retry`, `step`,
/// `graph`, `saga`, `whenever`, `justify`, `remember`, `admit`, `ready`,
/// `await approval`, `serve`.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse};
///
/// let script = parse("flow deploy(target: &Host):\n    step upload:\n        target.upload().await.or_fail()?\n".parse()?)?;
/// let Some(Statement::Construct(Construct::Step(step))) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is a step".into());
/// };
/// assert_eq!(step.label(), "upload");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Construct {
    /// `each <pattern> in <items>[, at most (limit) at once]:`.
    Each(Each),
    /// `within <duration>:`.
    Within(Within),
    /// `retry up to <N> times[, waiting <duration>]:`.
    Retry(Retry),
    /// `step <name>:`.
    Step(Step),
    /// `graph[, at most (limit) at once]:` and its `node`s.
    Graph(Graph),
    /// `saga <name>:`, whose body may `compensate` its steps.
    Saga(Step),
    /// `whenever <pattern> in <batches>, buffering <N>, <overflow>:`.
    Whenever(Whenever),
    /// `justify with (<rules>):`.
    Justify(Justify),
    /// `remember <name>:`, whose value is recorded once per run.
    Remember(Step),
    /// `admit from <producer> through (<gate>):`.
    Admit(Admit),
    /// `ready <pattern> from (<readiness>) within <duration>:`.
    Ready(Ready),
    /// `await approval <label> within <duration>:`.
    Approval(Approval),
    /// `serve on port ..., at most N runs in flight:` and its `route`s.
    Serve(Serve),
}

/// What a `together:` branch runs.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{BranchValue, Statement, parse};
///
/// let script = parse("flow both(left: &Api, right: &Api):\n    together:\n        let up = run probe(left)\n        let down = run probe(right)\n".parse()?)?;
/// let Some(Statement::Together(together)) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is together".into());
/// };
/// assert!(together.branches().iter().all(|b| matches!(b.value(), BranchValue::Run(_))));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum BranchValue {
    /// A block word.
    Construct(Construct),
    /// Exactly one `run` call.
    Run(Call),
    /// A line of Rust whose value is the branch's.
    Rust(Code),
}

/// A count a word takes: `up to <N>`, `at most (<limit>)`.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Bound, Construct, Statement, parse};
///
/// let script = parse("flow fetch(site: &Site) -> Page:\n    let page = retry up to 3 times, waiting 200ms:\n        site.get(scope.key()).await.or_retry()?\n    give back page\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Retry(retry), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is a retry".into());
/// };
/// assert!(matches!(retry.attempts(), Bound::Count(3)));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Bound {
    /// A literal, already checked against the word's range.
    Count(u64),
    /// An expression the runtime checks, ready to splice: the inside of a
    /// written `( )`, or the written tokens in `( )` of their own.
    Expr(TokenStream),
}

/// A duration a word takes: `within <D>`, `waiting <D>`.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Duration, Statement, parse};
///
/// let script = parse("flow ping(host: &Host) -> u16:\n    let code = within 2s:\n        host.ping().await.or_retry()?\n    give back code\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Within(within), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is a within".into());
/// };
/// assert!(matches!(within.limit(), Duration::Nanos(2_000_000_000)));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Duration {
    /// A literal such as `250ms` or `1.5s`, in nanoseconds; never zero.
    Nanos(u64),
    /// The expression inside a written `( )`, of type `Duration`.
    Expr(TokenStream),
}

/// One piece of a line of Rust.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Fragment, Statement, parse};
///
/// let script = parse("flow outer(site: &Site) -> usize:\n    let pages = run crawl(site)\n    give back pages\n".parse()?)?;
/// let Some(Statement::Let(line)) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is a let".into());
/// };
/// assert!(line.fragments().iter().any(|f| matches!(f, Fragment::Run(call) if call.callee() == "crawl")));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Fragment {
    /// A token passed through as written.
    Token(TokenTree),
    /// A bracketed group, whose inside is read the same way.
    Group {
        /// The bracket.
        delimiter: Delimiter,
        /// The group's span, which a rebuilt group keeps.
        span: Span,
        /// What is inside.
        inner: Code,
    },
    /// A `run` call.
    Run(Call),
    /// An `observe` or `act` of a registry domain.
    Domain(Domain),
    /// A `fetch`, `ask` or `send` outside call.
    Outside(Outside),
}
