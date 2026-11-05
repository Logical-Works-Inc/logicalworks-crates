//! The feed words' nodes: `whenever` over a stream of batches, with the
//! [`Overflow`] it applies past its buffer, and `justify`, whose rules govern
//! every step of its block.

use proc_macro2::TokenStream;

use super::{Block, Bound, Code, Site};

/// What `whenever` does with an event past its buffer.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse, Overflow};
///
/// let script = parse("flow watch(feed: &Feed) -> u64:\n    let report = whenever tick in feed.next().await, buffering 1024, drop oldest:\n        tick.record()\n    give back report.handled\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Whenever(feed), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a whenever".into());
/// };
/// assert_eq!(feed.overflow(), Overflow::DropOldest);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Overflow {
    /// `drop oldest`: keep the newest events.
    DropOldest,
    /// `drop newest`: keep the events that came first.
    DropNewest,
    /// `refuse`: hand the event back, handling the oldest to make room.
    Refuse,
}

/// `whenever <pattern> in <batches>, buffering <N>, <overflow>:`.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse};
///
/// let script = parse("flow watch(feed: &Feed) -> u64:\n    let report = whenever tick in feed.next().await, buffering 1024, drop oldest:\n        tick.record()\n    give back report.handled\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Whenever(feed), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a whenever".into());
/// };
/// assert_eq!(feed.pattern().to_string(), "tick");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Whenever {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The structural label of the stream's scope.
    pub(crate) label: String,
    /// The pattern each event binds.
    pub(crate) pattern: TokenStream,
    /// The expression awaited for the next batch; `None` ends the stream.
    pub(crate) batches: Code,
    /// The most events buffered unhandled.
    pub(crate) cap: Bound,
    /// What happens to an event past `cap`.
    pub(crate) overflow: Overflow,
    /// The body run for each event.
    pub(crate) body: Block,
}

impl Whenever {
    /// The structural label of the stream's scope.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow watch(feed: &Feed) -> u64:\n    let report = whenever tick in feed.next().await, buffering 1024, drop oldest:\n        tick.record()\n    give back report.handled\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Whenever(feed), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a whenever".into());
    /// };
    /// assert_eq!(feed.label(), "whenever:tick");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// The pattern each event binds.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow watch(feed: &Feed) -> u64:\n    let report = whenever tick in feed.next().await, buffering 1024, drop oldest:\n        tick.record()\n    give back report.handled\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Whenever(feed), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a whenever".into());
    /// };
    /// assert_eq!(feed.pattern().to_string(), "tick");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn pattern(&self) -> &TokenStream {
        &self.pattern
    }

    /// The expression awaited for the next batch: `Some(events)`, or `None`
    /// when the feed has ended.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow watch(feed: &Feed) -> u64:\n    let report = whenever tick in feed.next().await, buffering 1024, drop oldest:\n        tick.record()\n    give back report.handled\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Whenever(feed), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a whenever".into());
    /// };
    /// assert!(!feed.batches().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn batches(&self) -> &Code {
        &self.batches
    }

    /// The most events buffered unhandled.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse, Bound};
    ///
    /// let script = parse("flow watch(feed: &Feed) -> u64:\n    let report = whenever tick in feed.next().await, buffering 1024, drop oldest:\n        tick.record()\n    give back report.handled\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Whenever(feed), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a whenever".into());
    /// };
    /// assert!(matches!(feed.cap(), Bound::Count(1024)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn cap(&self) -> &Bound {
        &self.cap
    }

    /// What happens to an event past the buffer.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse, Overflow};
    ///
    /// let script = parse("flow watch(feed: &Feed) -> u64:\n    let report = whenever tick in feed.next().await, buffering 1024, drop oldest:\n        tick.record()\n    give back report.handled\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Whenever(feed), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a whenever".into());
    /// };
    /// assert_eq!(feed.overflow(), Overflow::DropOldest);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn overflow(&self) -> Overflow {
        self.overflow
    }

    /// The body run for each event.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow watch(feed: &Feed) -> u64:\n    let report = whenever tick in feed.next().await, buffering 1024, drop oldest:\n        tick.record()\n    give back report.handled\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Whenever(feed), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a whenever".into());
    /// };
    /// assert_eq!(feed.body().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn body(&self) -> &Block {
        &self.body
    }
}

/// `justify with (<rules>):`: a block whose every step answers to `rules`.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Construct, Statement, parse};
///
/// let script = parse("flow sync(api: &Api, rules: &Justify) -> u16:\n    let code = justify with (rules):\n        step call:\n            api.call().await.or_fail()?\n    give back code\n".parse()?)?;
/// let Some(Statement::Bind { block: Construct::Justify(justify), .. }) = script.flows()[0].body().statements().first() else {
///     return Err("the first line binds a justify".into());
/// };
/// assert_eq!(justify.rules().to_string(), "rules");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Justify {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// The structural label of the governed scope.
    pub(crate) label: String,
    /// The expression inside the written `( )`, of type `Justify`.
    pub(crate) rules: TokenStream,
    /// The block the rules govern.
    pub(crate) governed: Block,
}

impl Justify {
    /// The structural label of the governed scope.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow sync(api: &Api, rules: &Justify) -> u16:\n    let code = justify with (rules):\n        step call:\n            api.call().await.or_fail()?\n    give back code\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Justify(justify), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a justify".into());
    /// };
    /// assert_eq!(justify.label(), "justify");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// The expression inside the written `( )`, of type `Justify`.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow sync(api: &Api, rules: &Justify) -> u16:\n    let code = justify with (rules):\n        step call:\n            api.call().await.or_fail()?\n    give back code\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Justify(justify), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a justify".into());
    /// };
    /// assert_eq!(justify.rules().to_string(), "rules");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn rules(&self) -> &TokenStream {
        &self.rules
    }

    /// The block the rules govern.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Construct, Statement, parse};
    ///
    /// let script = parse("flow sync(api: &Api, rules: &Justify) -> u16:\n    let code = justify with (rules):\n        step call:\n            api.call().await.or_fail()?\n    give back code\n".parse()?)?;
    /// let Some(Statement::Bind { block: Construct::Justify(justify), .. }) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line binds a justify".into());
    /// };
    /// assert_eq!(justify.governed().statements().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn governed(&self) -> &Block {
        &self.governed
    }
}
