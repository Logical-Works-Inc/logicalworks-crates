//! The block nodes: a flow and the indented blocks its words open (`each`,
//! `within`, `retry`, `step`, `for`), each read once by
//! [`parse`](super::super::parse). The branching nodes, `together` and `if`,
//! are in `branch`.

use proc_macro2::{Ident, TokenStream};

use super::{Bound, Code, Duration, FlowShape, Site, Statement};

/// `[pub] flow name(inputs) [-> Output]:` and its body.
///
/// # Examples
/// ```
/// use lgwks_ast::script::parse;
///
/// let script = parse("/// How many items.\npub flow count(items: Vec<u8>) -> usize:\n    give back items.len()\n".parse()?)?;
/// let flow = &script.flows()[0];
/// assert_eq!(flow.name().to_string(), "count");
/// assert_eq!(flow.shape().signature(), "count(items: Vec<u8>) -> usize");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Flow {
    /// The attributes written above the flow, doc comments included, in order.
    pub(crate) attributes: TokenStream,
    /// What stands before `flow`: `pub`, `pub(crate)`, or nothing.
    pub(crate) visibility: TokenStream,
    /// The flow's name.
    pub(crate) name: Ident,
    /// The tokens inside the inputs' `( )`, as written.
    pub(crate) inputs: TokenStream,
    /// The promised output type; `None` when the flow promises `()`.
    pub(crate) output: Option<TokenStream>,
    /// The flow's body.
    pub(crate) body: Block,
    /// The flow's entry in the architecture map.
    pub(crate) shape: FlowShape,
}

impl Flow {
    /// The attributes written above the flow, doc comments included, in order.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("/// How many items.\npub flow count(items: Vec<u8>) -> usize:\n    give back items.len()\n".parse()?)?;
    /// assert!(script.flows()[0].attributes().to_string().contains("How many items."));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn attributes(&self) -> &TokenStream {
        &self.attributes
    }

    /// What stands before `flow`: `pub`, `pub(crate)`, or nothing.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("/// How many items.\npub flow count(items: Vec<u8>) -> usize:\n    give back items.len()\n".parse()?)?;
    /// assert_eq!(script.flows()[0].visibility().to_string(), "pub");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn visibility(&self) -> &TokenStream {
        &self.visibility
    }

    /// The identifier after `flow`, which names the generated `async fn`.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("/// How many items.\npub flow count(items: Vec<u8>) -> usize:\n    give back items.len()\n".parse()?)?;
    /// assert_eq!(script.flows()[0].name().to_string(), "count");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn name(&self) -> &Ident {
        &self.name
    }

    /// The tokens inside the inputs' `( )`, as written.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("/// How many items.\npub flow count(items: Vec<u8>) -> usize:\n    give back items.len()\n".parse()?)?;
    /// assert_eq!(script.flows()[0].inputs().to_string(), "items : Vec < u8 >");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn inputs(&self) -> &TokenStream {
        &self.inputs
    }

    /// The promised output type; `None` when the flow promises `()`.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("/// How many items.\npub flow count(items: Vec<u8>) -> usize:\n    give back items.len()\n".parse()?)?;
    /// let output = script.flows()[0].output().map(ToString::to_string);
    /// assert_eq!(output.as_deref(), Some("usize"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn output(&self) -> Option<&TokenStream> {
        self.output.as_ref()
    }

    /// The statements under the `flow` header, in source order.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("/// How many items.\npub flow count(items: Vec<u8>) -> usize:\n    give back items.len()\n".parse()?)?;
    /// assert_eq!(script.flows()[0].body().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn body(&self) -> &Block {
        &self.body
    }

    /// The flow's entry in the architecture map.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("/// How many items.\npub flow count(items: Vec<u8>) -> usize:\n    give back items.len()\n".parse()?)?;
    /// assert_eq!(script.flows()[0].shape().name(), "count");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn shape(&self) -> &FlowShape {
        &self.shape
    }
}

/// The lines of one indented block, already read.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse};
///
/// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is an each".into());
/// };
/// assert_eq!(each.body().statements().len(), 1);
/// assert!(each.body().wants_value());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Block {
    /// Every line, in source order; an `if` chain is one statement.
    pub(crate) statements: Vec<Statement>,
    /// Whether the block's last expression is its value.
    pub(crate) wants_value: bool,
}

impl Block {
    /// Every line, in source order; an `if` chain is one statement.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("/// How many items.\npub flow count(items: Vec<u8>) -> usize:\n    give back items.len()\n".parse()?)?;
    /// assert_eq!(script.flows()[0].body().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn statements(&self) -> &[Statement] {
        self.statements.as_slice()
    }

    /// Whether the block's last expression is its value.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an each".into());
    /// };
    /// assert!(each.body().wants_value(), "`let sizes = each ..:` binds the body's last line");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn wants_value(&self) -> bool {
        self.wants_value
    }
}

/// `each <pattern> in <items>:`, every item concurrently under a bound.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse};
///
/// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is an each".into());
/// };
/// assert_eq!(each.pattern().to_string(), "page");
/// assert!(each.bound().is_none(), "no `at most`, so the machine sizes the fan-out");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Each {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The structural label of the fan-out's scope.
    pub(crate) label: String,
    /// The written bound; `None` when the runtime sizes the fan-out.
    pub(crate) bound: Option<Bound>,
    /// The pattern each item binds.
    pub(crate) pattern: TokenStream,
    /// The items.
    pub(crate) items: Code,
    /// The body run for each item.
    pub(crate) body: Block,
}

impl Each {
    /// The structural label of the fan-out's scope.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an each".into());
    /// };
    /// assert_eq!(each.label(), "each:page");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// The written bound; `None` when the runtime sizes the fan-out.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an each".into());
    /// };
    /// assert!(each.bound().is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn bound(&self) -> Option<&Bound> {
        self.bound.as_ref()
    }

    /// The pattern each item binds.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an each".into());
    /// };
    /// assert_eq!(each.pattern().to_string(), "page");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn pattern(&self) -> &TokenStream {
        &self.pattern
    }

    /// The expression after `in`, iterated for the fan-out.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an each".into());
    /// };
    /// assert!(!each.items().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn items(&self) -> &Code {
        &self.items
    }

    /// The body run for each item.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Each(each), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an each".into());
    /// };
    /// assert_eq!(each.body().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn body(&self) -> &Block {
        &self.body
    }
}

/// `within <duration>:`, the body under a deadline.
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
pub struct Within {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The structural label of the deadline's scope.
    pub(crate) label: String,
    /// The deadline.
    pub(crate) limit: Duration,
    /// The body.
    pub(crate) body: Block,
}

impl Within {
    /// The structural label of the deadline's scope.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow ping(host: &Host) -> u16:\n    let code = within 2s:\n        host.ping().await.or_retry()?\n    give back code\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Within(within), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a within".into());
    /// };
    /// assert_eq!(within.label(), "within");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// The deadline.
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
    #[must_use]
    pub const fn limit(&self) -> &Duration {
        &self.limit
    }

    /// The statements that run under the deadline.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow ping(host: &Host) -> u16:\n    let code = within 2s:\n        host.ping().await.or_retry()?\n    give back code\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Within(within), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a within".into());
    /// };
    /// assert_eq!(within.body().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn body(&self) -> &Block {
        &self.body
    }
}

/// `retry up to <N> times[, waiting <duration>]:`.
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
pub struct Retry {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The structural label of the retry's scope.
    pub(crate) label: String,
    /// The attempt budget.
    pub(crate) attempts: Bound,
    /// The pause between attempts; `None` when none was written.
    pub(crate) waiting: Option<Duration>,
    /// The body each attempt runs.
    pub(crate) body: Block,
}

impl Retry {
    /// The structural label of the retry's scope.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow fetch(site: &Site) -> Page:\n    let page = retry up to 3 times, waiting 200ms:\n        site.get(scope.key()).await.or_retry()?\n    give back page\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Retry(retry), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a retry".into());
    /// };
    /// assert_eq!(retry.label(), "retry");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// How many times the block may run, the first try included.
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
    #[must_use]
    pub const fn attempts(&self) -> &Bound {
        &self.attempts
    }

    /// The pause between attempts; `None` when none was written.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Duration, Statement, parse};
    ///
    /// let script = parse("flow fetch(site: &Site) -> Page:\n    let page = retry up to 3 times, waiting 200ms:\n        site.get(scope.key()).await.or_retry()?\n    give back page\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Retry(retry), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a retry".into());
    /// };
    /// assert!(matches!(retry.waiting(), Some(Duration::Nanos(200_000_000))));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn waiting(&self) -> Option<&Duration> {
        self.waiting.as_ref()
    }

    /// The body each attempt runs.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow fetch(site: &Site) -> Page:\n    let page = retry up to 3 times, waiting 200ms:\n        site.get(scope.key()).await.or_retry()?\n    give back page\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Retry(retry), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a retry".into());
    /// };
    /// assert_eq!(retry.body().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn body(&self) -> &Block {
        &self.body
    }
}

/// `step <name>:`, a named scope.
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
pub struct Step {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The structural label: the name, numbered among same-named siblings.
    pub(crate) label: String,
    /// The body.
    pub(crate) body: Block,
}

impl Step {
    /// The structural label: the name, numbered among same-named siblings.
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
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// The statements that run inside the named scope.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow deploy(target: &Host):\n    step upload:\n        target.upload().await.or_fail()?\n".parse()?)?;
    /// let Some(Statement::Construct(Construct::Step(step))) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a step".into());
    /// };
    /// assert_eq!(step.body().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn body(&self) -> &Block {
        &self.body
    }
}

/// `for <pattern> in <items>:`, sequentially, one scope per item.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Statement, parse};
///
/// let script = parse("flow notify(users: Vec<User>):\n    for user in users:\n        run email(user)\n".parse()?)?;
/// let Some(Statement::For(each_user)) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is a for".into());
/// };
/// assert_eq!(each_user.pattern().to_string(), "user");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct For {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The structural label every item's scope is numbered under.
    pub(crate) label: String,
    /// The pattern each item binds.
    pub(crate) pattern: TokenStream,
    /// The items.
    pub(crate) items: Code,
    /// The body run for each item.
    pub(crate) body: Block,
}

impl For {
    /// The structural label every item's scope is numbered under.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow notify(users: Vec<User>):\n    for user in users:\n        run email(user)\n".parse()?)?;
    /// let Some(Statement::For(each_user)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a for".into());
    /// };
    /// assert_eq!(each_user.label(), "for:user");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// The pattern each item binds.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow notify(users: Vec<User>):\n    for user in users:\n        run email(user)\n".parse()?)?;
    /// let Some(Statement::For(each_user)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a for".into());
    /// };
    /// assert_eq!(each_user.pattern().to_string(), "user");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn pattern(&self) -> &TokenStream {
        &self.pattern
    }

    /// The expression after `in`, walked one item at a time.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow notify(users: Vec<User>):\n    for user in users:\n        run email(user)\n".parse()?)?;
    /// let Some(Statement::For(each_user)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a for".into());
    /// };
    /// assert!(!each_user.items().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn items(&self) -> &Code {
        &self.items
    }

    /// The body run for each item.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow notify(users: Vec<User>):\n    for user in users:\n        run email(user)\n".parse()?)?;
    /// let Some(Statement::For(each_user)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is a for".into());
    /// };
    /// assert!(matches!(each_user.body().statements()[0], Statement::Run(_)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn body(&self) -> &Block {
        &self.body
    }
}
