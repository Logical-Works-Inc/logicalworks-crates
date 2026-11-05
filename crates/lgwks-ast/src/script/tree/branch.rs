//! The branching nodes: `together:` and its branches, and the `if` chain,
//! each read once by [`parse`](super::super::parse).

use proc_macro2::TokenStream;

use super::{BranchValue, IfBranch};

/// `together:`, every branch concurrently on this task.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Statement, parse};
///
/// let script = parse("flow both(left: &Api, right: &Api):\n    together:\n        let up = run probe(left)\n        let down = run probe(right)\n".parse()?)?;
/// let Some(Statement::Together(together)) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is together".into());
/// };
/// assert_eq!(together.branches().len(), 2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Together {
    /// The branches, in source order.
    pub(crate) branches: Vec<Branch>,
}

impl Together {
    /// The branches, in source order.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow both(left: &Api, right: &Api):\n    together:\n        let up = run probe(left)\n        let down = run probe(right)\n".parse()?)?;
    /// let Some(Statement::Together(together)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is together".into());
    /// };
    /// let bound: Vec<String> = together.branches().iter().filter_map(|b| b.pattern()).map(ToString::to_string).collect();
    /// assert_eq!(bound, ["up", "down"]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn branches(&self) -> &[Branch] {
        self.branches.as_slice()
    }
}

/// One line under `together:`.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{BranchValue, Statement, parse};
///
/// let script = parse("flow both(left: &Api, right: &Api):\n    together:\n        let up = run probe(left)\n        let down = run probe(right)\n".parse()?)?;
/// let Some(Statement::Together(together)) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is together".into());
/// };
/// let first = &together.branches()[0];
/// assert!(matches!(first.value(), BranchValue::Run(call) if call.callee() == "probe"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Branch {
    /// The pattern a `let` branch binds; `None` for a branch that binds nothing.
    pub(crate) pattern: Option<TokenStream>,
    /// What the branch runs.
    pub(crate) value: BranchValue,
}

impl Branch {
    /// The pattern a `let` branch binds; `None` for a branch that binds nothing.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow both(left: &Api, right: &Api):\n    together:\n        let up = run probe(left)\n        let down = run probe(right)\n".parse()?)?;
    /// let Some(Statement::Together(together)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is together".into());
    /// };
    /// let pattern = together.branches()[0].pattern().map(ToString::to_string);
    /// assert_eq!(pattern.as_deref(), Some("up"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn pattern(&self) -> Option<&TokenStream> {
        self.pattern.as_ref()
    }

    /// What the branch runs.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{BranchValue, Statement, parse};
    ///
    /// let script = parse("flow both(left: &Api, right: &Api):\n    together:\n        let up = run probe(left)\n        let down = run probe(right)\n".parse()?)?;
    /// let Some(Statement::Together(together)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is together".into());
    /// };
    /// assert!(matches!(together.branches()[1].value(), BranchValue::Run(_)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn value(&self) -> &BranchValue {
        &self.value
    }
}

/// An `if` chain.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Statement, parse};
///
/// let script = parse("flow route(code: u16) -> u8:\n    if code < 400:\n        give back 0\n    else if code < 500:\n        give back 1\n    else:\n        give back 2\n".parse()?)?;
/// let Some(Statement::If(chain)) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is an if chain".into());
/// };
/// assert_eq!(chain.branches().len(), 3);
/// assert!(script.flows()[0].body().diverges(), "every branch gives back");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct IfChain {
    /// `if`, then each `else if`, then the `else` if one ends the chain.
    pub(crate) branches: Vec<IfBranch>,
    /// Whether the chain is its block's value: it is the last line of a block that wants one, and an `else:` ends it.
    pub(crate) wants_value: bool,
    /// Whether every branch leaves the block, which makes the chain a way out rather than a value.
    pub(crate) leaves: bool,
}

impl IfChain {
    /// `if`, then each `else if`, then the `else` if one ends the chain.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow route(code: u16) -> u8:\n    if code < 400:\n        give back 0\n    else if code < 500:\n        give back 1\n    else:\n        give back 2\n".parse()?)?;
    /// let Some(Statement::If(chain)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an if chain".into());
    /// };
    /// assert!(chain.branches()[2].condition().is_none(), "the last branch is `else:`");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn branches(&self) -> &[IfBranch] {
        self.branches.as_slice()
    }

    /// Whether the chain is its block's value: it is the last line of a block that wants one, and an `else:` ends it.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Statement, parse};
    ///
    /// let script = parse("flow route(code: u16) -> u8:\n    if code < 400:\n        give back 0\n    else if code < 500:\n        give back 1\n    else:\n        give back 2\n".parse()?)?;
    /// let Some(Statement::If(chain)) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is an if chain".into());
    /// };
    /// assert!(chain.wants_value(), "the flow's last line, and an `else:` ends it");
    /// assert!(!script.flows()[0].body().statements()[0].is_expression(), "it leaves, so it is no value");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn wants_value(&self) -> bool {
        self.wants_value
    }
}
