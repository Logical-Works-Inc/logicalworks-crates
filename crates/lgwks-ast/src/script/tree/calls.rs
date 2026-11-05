//! The nodes of the steps that call out of a flow: `observe` and `act`
//! against a registered domain, and the outside words `fetch`, `ask` and
//! `send`. Each keeps its arguments as written; what they mean is the
//! runtime's, read through the registry or the capability the word declares.

use super::super::lexicon::Kind;
use super::{Code, Site};

/// `observe domain::id of target` or `act domain::id on target with value`: a
/// registry identifier called in this scope, as a step of its own (#388).
///
/// The identifier is kept as written, `::` and all, because it is the key the
/// host's `DomainRegistry` declares and the text a refusal names; it is never
/// resolved here. Resolution is the runtime's, at the flow's entry, against
/// the registry the run was given, so a flow refers to a domain by the same
/// string a `BotSpec` does and the two cannot drift.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Fragment, Statement, parse, Kind};
///
/// let script = parse("flow open(repo: &str) -> u16:\n    let open: u16 = observe github::pr_status of repo\n    give back open\n".parse()?)?;
/// let (Some(Statement::Let(line)) | Some(Statement::Rust(line))) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is Rust".into());
/// };
/// let Some(Fragment::Domain(domain)) = line.fragments().iter().find(|f| matches!(f, Fragment::Domain(_))) else {
///     return Err("the line calls out".into());
/// };
/// assert_eq!((domain.kind(), domain.id()), (Kind::Observe, "github::pr_status"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Domain {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// Whether it observes (`observe`) or acts (`act`).
    pub(crate) kind: Kind,
    /// The registry identifier, as written without spaces: `github::pr_status`.
    pub(crate) id: String,
    /// The structural label: the identifier's last segment, numbered among
    /// same-named steps in one scope.
    pub(crate) label: String,
    /// What the domain is built from: after `of` for `observe`, between `on`
    /// and `with` for `act`.
    pub(crate) target: Code,
    /// What an action is handed, after `with`; `None` for `observe`.
    pub(crate) value: Option<Code>,
}

impl Domain {
    /// Whether it observes ([`Kind::Observe`]) or acts ([`Kind::Act`]).
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Fragment, Statement, parse, Kind};
    ///
    /// let script = parse("flow page(count: u16):\n    act notify::page on \"oncall\" with count\n".parse()?)?;
    /// let (Some(Statement::Let(line)) | Some(Statement::Rust(line))) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is Rust".into());
    /// };
    /// let Some(Fragment::Domain(domain)) = line.fragments().iter().find(|f| matches!(f, Fragment::Domain(_))) else {
    ///     return Err("the line calls out".into());
    /// };
    /// assert_eq!(domain.kind(), Kind::Act);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn kind(&self) -> Kind {
        self.kind
    }

    /// The registry identifier, as written without spaces.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Fragment, Statement, parse};
    ///
    /// let script = parse("flow open(repo: &str) -> u16:\n    let open: u16 = observe github::pr_status of repo\n    give back open\n".parse()?)?;
    /// let (Some(Statement::Let(line)) | Some(Statement::Rust(line))) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is Rust".into());
    /// };
    /// let Some(Fragment::Domain(domain)) = line.fragments().iter().find(|f| matches!(f, Fragment::Domain(_))) else {
    ///     return Err("the line calls out".into());
    /// };
    /// assert_eq!(domain.id(), "github::pr_status");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn id(&self) -> &str {
        self.id.as_str()
    }

    /// The structural label: the identifier's last segment, numbered among
    /// same-named steps in one scope.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Fragment, Statement, parse};
    ///
    /// let script = parse("flow open(repo: &str) -> u16:\n    let open: u16 = observe github::pr_status of repo\n    give back open\n".parse()?)?;
    /// let (Some(Statement::Let(line)) | Some(Statement::Rust(line))) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is Rust".into());
    /// };
    /// let Some(Fragment::Domain(domain)) = line.fragments().iter().find(|f| matches!(f, Fragment::Domain(_))) else {
    ///     return Err("the line calls out".into());
    /// };
    /// assert_eq!(domain.label(), "pr_status");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// What the domain is built from.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Fragment, Statement, parse};
    ///
    /// let script = parse("flow page(count: u16):\n    act notify::page on \"oncall\" with count\n".parse()?)?;
    /// let (Some(Statement::Let(line)) | Some(Statement::Rust(line))) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is Rust".into());
    /// };
    /// let Some(Fragment::Domain(domain)) = line.fragments().iter().find(|f| matches!(f, Fragment::Domain(_))) else {
    ///     return Err("the line calls out".into());
    /// };
    /// assert!(!domain.target().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn target(&self) -> &Code {
        &self.target
    }

    /// What an action is handed; `None` for an observation.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Fragment, Statement, parse};
    ///
    /// let script = parse("flow page(count: u16):\n    act notify::page on \"oncall\" with count\n".parse()?)?;
    /// let (Some(Statement::Let(line)) | Some(Statement::Rust(line))) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is Rust".into());
    /// };
    /// let Some(Fragment::Domain(domain)) = line.fragments().iter().find(|f| matches!(f, Fragment::Domain(_))) else {
    ///     return Err("the line calls out".into());
    /// };
    /// assert!(domain.value().is_some());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn value(&self) -> Option<&Code> {
        self.value.as_ref()
    }
}

/// `fetch <url> [with (options)]`, `ask <model> with (request)` or
/// `send <receiver> with <payload>`: an outside call in this scope, as a step
/// of its own (#783).
///
/// Like [`Domain`], the call's arguments are kept as written: the URL, model
/// or receiver expression and the options, request or payload beside it. What
/// they mean — the deadline, the retry budget, the size bound, the token
/// budget — is the runtime's, which reads the same values through the
/// capability the word declares, so a flow refers to an outside call by the
/// same strings its admission checked.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Fragment, Statement, parse, Kind};
///
/// let script = parse("flow call(url: &str, options: Options) -> u16:\n    let answered = fetch url with (options)\n    give back answered.status()\n".parse()?)?;
/// let (Some(Statement::Let(line)) | Some(Statement::Rust(line))) = script.flows()[0].body().statements().first() else {
///     return Err("the first line is Rust".into());
/// };
/// let Some(Fragment::Outside(call)) = line.fragments().iter().find(|f| matches!(f, Fragment::Outside(_))) else {
///     return Err("the line calls out".into());
/// };
/// assert_eq!(call.kind(), Kind::Fetch);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Outside {
    /// Where it stands in the script; read through `site()`.
    pub(crate) site: Site,
    /// Whether it fetches, asks or sends.
    pub(crate) kind: Kind,
    /// The structural label: the word, numbered among same-named steps in one
    /// scope.
    pub(crate) label: String,
    /// What the call reaches: the URL, the model or the receiver.
    pub(crate) target: Code,
    /// The word's declared bounds — the options, request or payload — or
    /// `None` for a `fetch` that inherits them from its enclosing blocks.
    pub(crate) options: Option<Code>,
}

impl Outside {
    /// Whether it fetches, asks or sends.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Fragment, Statement, parse, Kind};
    ///
    /// let script = parse("flow call(url: &str, options: Options) -> u16:\n    let answered = fetch url with (options)\n    give back answered.status()\n".parse()?)?;
    /// let (Some(Statement::Let(line)) | Some(Statement::Rust(line))) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is Rust".into());
    /// };
    /// let Some(Fragment::Outside(call)) = line.fragments().iter().find(|f| matches!(f, Fragment::Outside(_))) else {
    ///     return Err("the line calls out".into());
    /// };
    /// assert_eq!(call.kind(), Kind::Fetch);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn kind(&self) -> Kind {
        self.kind
    }

    /// The structural label: the word, numbered among same-named steps in one
    /// scope.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Fragment, Statement, parse};
    ///
    /// let script = parse("flow call(url: &str, options: Options) -> u16:\n    let answered = fetch url with (options)\n    give back answered.status()\n".parse()?)?;
    /// let (Some(Statement::Let(line)) | Some(Statement::Rust(line))) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is Rust".into());
    /// };
    /// let Some(Fragment::Outside(call)) = line.fragments().iter().find(|f| matches!(f, Fragment::Outside(_))) else {
    ///     return Err("the line calls out".into());
    /// };
    /// assert_eq!(call.label(), "fetch");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn label(&self) -> &str {
        self.label.as_str()
    }

    /// What the call reaches: the URL, the model or the receiver.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Fragment, Statement, parse};
    ///
    /// let script = parse("flow call(url: &str, options: Options) -> u16:\n    let answered = fetch url with (options)\n    give back answered.status()\n".parse()?)?;
    /// let (Some(Statement::Let(line)) | Some(Statement::Rust(line))) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is Rust".into());
    /// };
    /// let Some(Fragment::Outside(call)) = line.fragments().iter().find(|f| matches!(f, Fragment::Outside(_))) else {
    ///     return Err("the line calls out".into());
    /// };
    /// assert!(!call.target().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn target(&self) -> &Code {
        &self.target
    }

    /// The word's declared bounds, or `None` for a `fetch` that inherits them.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Fragment, Statement, parse};
    ///
    /// let script = parse("flow call(url: &str, options: Options) -> u16:\n    let answered = fetch url with (options)\n    give back answered.status()\n".parse()?)?;
    /// let (Some(Statement::Let(line)) | Some(Statement::Rust(line))) = script.flows()[0].body().statements().first() else {
    ///     return Err("the first line is Rust".into());
    /// };
    /// let Some(Fragment::Outside(call)) = line.fragments().iter().find(|f| matches!(f, Fragment::Outside(_))) else {
    ///     return Err("the line calls out".into());
    /// };
    /// assert!(call.options().is_some());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn options(&self) -> Option<&Code> {
        self.options.as_ref()
    }
}
