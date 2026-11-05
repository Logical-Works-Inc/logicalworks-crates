//! The lexicon: every word of `script!`, one row each (SL-1, #380).
//!
//! The parser dispatches on this table, and `lgwks_macros` renders its
//! documentation from it: the block table in that crate's documentation, the
//! word table in its README and the one-page lexicon are each checked against
//! the table by that crate's tests, so a word cannot be added, renamed or
//! re-described in one place and not the other.
//!
//! A word is reachable only through its row. [`Kind`] is constructed nowhere
//! but in [`LEXICON`], so a kind with no row is a variant the compiler reports
//! as never constructed, which the workspace's `-D warnings` refuses; and the
//! parser dispatches on the kind of the row a line's first token names, so a
//! line whose word has no row is plain Rust and never reaches a word's reader.
//!
//! The particles inside a form (`in`, `up to`, `times`, `waiting`, `at most`,
//! `with`, `of`, `on`) are part of that word's grammar, read by its reader;
//! they are not words, and a line cannot start with one.

use proc_macro2::TokenTree;

use super::lines::{Line, ident};

mod vocab;
mod words;

/// What a word does, which is what the parser dispatches on.
///
/// # Examples
/// ```
/// assert_eq!(lgwks_ast::script::Kind::Retry.spelling(), "retry");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Kind {
    /// `flow name(inputs) -> Output:`
    Flow,
    /// `each x in xs:`
    Each,
    /// `within 2s:`
    Within,
    /// `retry up to 3 times:`
    Retry,
    /// `together:`
    Together,
    /// `step name:`
    Step,
    /// `for x in xs:`
    For,
    /// `if cond:`
    If,
    /// `else:` / `else if cond:`
    Else,
    /// `let x = <block>:`
    Let,
    /// `run other(args)`
    Run,
    /// `observe domain::id of target`
    Observe,
    /// `act domain::id on target with value`
    Act,
    /// `give back value`
    GiveBack,
    /// `fail with reason`
    Fail,
    /// `graph:` and its nodes
    Graph,
    /// `node name needs other:` inside a `graph`
    Node,
    /// `saga name:`
    Saga,
    /// `compensate step:` inside a `saga`
    Compensate,
    /// `whenever x in batches, buffering N, drop oldest:`
    Whenever,
    /// `justify with (rules):`
    Justify,
    /// `remember name:`
    Remember,
    /// `admit from model through (gate):`
    Admit,
    /// `ready x from (readiness) within 30s:`
    Ready,
    /// `await approval <label> within 30s:`
    Approval,
    /// `fetch <url> [with (options)]`
    Fetch,
    /// `ask <model> with (request)`
    Ask,
    /// `send <receiver> with <payload>`
    Send,
    /// `serve on port ..., at most N runs in flight:` and its routes
    Serve,
    /// `route METHOD /path (body: Type) -> Type:` inside a `serve`
    Route,
}

impl Kind {
    /// The token a line starts with to say this word.
    ///
    /// # Examples
    /// ```
    /// assert_eq!(lgwks_ast::script::Kind::GiveBack.spelling(), "give");
    /// ```
    #[must_use]
    pub const fn spelling(self) -> &'static str {
        match self {
            Self::Flow => "flow",
            Self::Each => "each",
            Self::Within => "within",
            Self::Retry => "retry",
            Self::Together => "together",
            Self::Step => "step",
            Self::For => "for",
            Self::If => "if",
            Self::Else => "else",
            Self::Let => "let",
            Self::Run => "run",
            Self::Observe => "observe",
            Self::Act => "act",
            Self::GiveBack => "give",
            Self::Fail => "fail",
            Self::Graph => "graph",
            Self::Node => "node",
            Self::Saga => "saga",
            Self::Compensate => "compensate",
            Self::Whenever => "whenever",
            Self::Justify => "justify",
            Self::Remember => "remember",
            Self::Admit => "admit",
            Self::Ready => "ready",
            Self::Approval => "await",
            Self::Fetch => "fetch",
            Self::Ask => "ask",
            Self::Send => "send",
            Self::Serve => "serve",
            Self::Route => "route",
        }
    }
}

/// Where a word may stand.
///
/// # Examples
/// ```
/// let each = lgwks_ast::script::LEXICON.iter().find(|w| w.kind() == lgwks_ast::script::Kind::Each).ok_or("each")?;
/// assert_eq!(each.position(), lgwks_ast::script::Position::Block);
/// # Ok::<(), &str>(())
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Position {
    /// Opens a flow at the top level of a script.
    Flow,
    /// Opens an indented block: its line ends in `:`.
    Block,
    /// Starts a line that opens no block.
    Line,
    /// Anywhere inside a line of Rust.
    Inline,
}

impl Position {
    /// The name the README table gives this position.
    ///
    /// # Examples
    /// ```
    /// assert_eq!(lgwks_ast::script::Position::Inline.name(), "inline");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Flow => "flow",
            Self::Block => "block",
            Self::Line => "line",
            Self::Inline => "inline",
        }
    }
}

/// The axis a guarantee serves (SL-4).
///
/// The nine axes decide what a word must guarantee before it is admitted; they
/// are a design requirement, not a score. Only the axes some word's guarantee
/// serves today are variants here: a variant no row names would be dead code
/// the workspace's `-D warnings` refuses, and adding it is part of admitting
/// the word that first serves it.
///
/// # Examples
/// ```
/// let each = lgwks_ast::script::LEXICON.iter().find(|w| w.kind() == lgwks_ast::script::Kind::Each).ok_or("each")?;
/// assert!(each.guarantees().iter().any(|g| g.axis() == lgwks_ast::script::Axis::Hyperscale));
/// # Ok::<(), &str>(())
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Axis {
    /// Holds for any input and any schedule, not only the common one.
    Generalized,
    /// The caller depends on a typed answer, never on the callee's internals.
    Decoupled,
    /// Nothing outlives the run, and a rerun reproduces its keys.
    Ephemeral,
    /// Bounded in work, time and retries, however wide the input.
    Hyperscale,
    /// Idiomatic Rust: no panic, no hidden control flow.
    Idiomatic,
    /// Every key and every call stays inside one tenant.
    MultiTenant,
}

impl Axis {
    /// The axis as the README names it.
    ///
    /// # Examples
    /// ```
    /// assert_eq!(lgwks_ast::script::Axis::MultiTenant.name(), "multi-tenant");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Generalized => "generalized",
            Self::Decoupled => "decoupled",
            Self::Ephemeral => "ephemeral",
            Self::Hyperscale => "hyperscale",
            Self::Idiomatic => "idiomatic",
            Self::MultiTenant => "multi-tenant",
        }
    }
}

/// The test that proves a guarantee: a compile-fail case or a deterministic
/// simulation, the two kinds of evidence SL-4 accepts.
///
/// # Examples
/// ```
/// let each = lgwks_ast::script::LEXICON.iter().find(|w| w.kind() == lgwks_ast::script::Kind::Each).ok_or("each")?;
/// assert!(each.guarantees().iter().all(|g| matches!(g.evidence(), lgwks_ast::script::Evidence::Sim { .. })));
/// # Ok::<(), &str>(())
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum Evidence {
    /// A case of `lgwks_macros`' compile-fail table (`REFUSALS` in its
    /// `src/tests.rs`), named by its case.
    Refusal(&'static str),
    /// A seeded simulation: a `#[test]`, or a family a `band_family!` sweeps,
    /// in a `sim_` file named from the workspace root.
    Sim {
        /// The file, from the workspace root.
        file: &'static str,
        /// The test or family.
        test: &'static str,
    },
}

/// One thing that holds wherever a word is used, and the test that proves it.
///
/// # Examples
/// ```
/// let each = lgwks_ast::script::LEXICON.iter().find(|w| w.kind() == lgwks_ast::script::Kind::Each).ok_or("each")?;
/// let first = each.guarantees().first().ok_or("one guarantee")?;
/// assert_eq!((first.axis(), first.claim()), (lgwks_ast::script::Axis::Hyperscale, "bounded fan-out"));
/// # Ok::<(), &str>(())
/// ```
#[derive(Debug)]
pub struct Guarantee {
    /// The axis it serves.
    axis: Axis,
    /// What holds, in one clause.
    claim: &'static str,
    /// The test that fails if it stops holding.
    evidence: Evidence,
}

impl Guarantee {
    /// Which axis this guarantee is evidence for: one of the six [`Axis`]
    /// variants, the axes some word serves today.
    ///
    /// # Examples
    /// ```
    /// let each = lgwks_ast::script::LEXICON.iter().find(|w| w.kind() == lgwks_ast::script::Kind::Each).ok_or("each")?;
    /// assert_eq!(each.guarantees()[0].axis(), lgwks_ast::script::Axis::Hyperscale);
    /// # Ok::<(), &str>(())
    /// ```
    #[must_use]
    pub const fn axis(&self) -> Axis {
        self.axis
    }

    /// What holds, in one clause.
    ///
    /// # Examples
    /// ```
    /// let each = lgwks_ast::script::LEXICON.iter().find(|w| w.kind() == lgwks_ast::script::Kind::Each).ok_or("each")?;
    /// assert_eq!(each.guarantees()[0].claim(), "bounded fan-out");
    /// # Ok::<(), &str>(())
    /// ```
    #[must_use]
    pub const fn claim(&self) -> &'static str {
        self.claim
    }

    /// The test that fails if it stops holding.
    ///
    /// # Examples
    /// ```
    /// let each = lgwks_ast::script::LEXICON.iter().find(|w| w.kind() == lgwks_ast::script::Kind::Each).ok_or("each")?;
    /// let lgwks_ast::script::Evidence::Sim { test, .. } = each.guarantees()[0].evidence() else { return Err("a sim") };
    /// assert_eq!(*test, "fan_bounded");
    /// # Ok::<(), &str>(())
    /// ```
    #[must_use]
    pub const fn evidence(&self) -> &Evidence {
        &self.evidence
    }
}

/// A guarantee proved by a seeded simulation in the named `sim_` file.
const fn script_sim_in(
    file: &'static str,
    axis: Axis,
    claim: &'static str,
    test: &'static str,
) -> Guarantee {
    Guarantee {
        axis,
        claim,
        evidence: Evidence::Sim { file, test },
    }
}

/// A guarantee proved by a compile-fail case.
const fn refused(axis: Axis, claim: &'static str, case: &'static str) -> Guarantee {
    Guarantee {
        axis,
        claim,
        evidence: Evidence::Refusal(case),
    }
}

/// A domain word's row: inline, and calling the runtime function it is
/// spelled as.
///
/// `observe` and `act` differ only in what they read and promise, so the
/// row's fixed part is written once here.
const fn domain_word(
    kind: Kind,
    primitive: &'static str,
    forms: &'static [Form],
    refuses: &'static [&'static str],
    guarantees: &'static [Guarantee],
    example: &'static str,
) -> Word {
    Word {
        kind,
        position: Position::Inline,
        primitive,
        forms,
        refuses,
        guarantees,
        example,
    }
}

/// One written form of a word and what it means.
///
/// # Examples
/// ```
/// let each = lgwks_ast::script::LEXICON.iter().find(|w| w.kind() == lgwks_ast::script::Kind::Each).ok_or("each")?;
/// assert_eq!(each.forms()[0].written(), "`each x in xs:`");
/// # Ok::<(), &str>(())
/// ```
#[derive(Debug)]
pub struct Form {
    /// The form as an author writes it.
    written: &'static str,
    /// What it does, in one clause.
    means: &'static str,
}

impl Form {
    /// The form as an author writes it, as Markdown.
    ///
    /// # Examples
    /// ```
    /// let each = lgwks_ast::script::LEXICON.iter().find(|w| w.kind() == lgwks_ast::script::Kind::Each).ok_or("each")?;
    /// assert!(each.forms().iter().any(|f| f.written().contains("at most")));
    /// # Ok::<(), &str>(())
    /// ```
    #[must_use]
    pub const fn written(&self) -> &'static str {
        self.written
    }

    /// What it does, in one clause, as Markdown.
    ///
    /// # Examples
    /// ```
    /// let each = lgwks_ast::script::LEXICON.iter().find(|w| w.kind() == lgwks_ast::script::Kind::Each).ok_or("each")?;
    /// assert!(each.forms()[0].means().starts_with("every item"));
    /// # Ok::<(), &str>(())
    /// ```
    #[must_use]
    pub const fn means(&self) -> &'static str {
        self.means
    }
}

/// One word of the language: the six things SL-1 names, plus where it stands.
///
/// # Examples
/// ```
/// let each = lgwks_ast::script::LEXICON.iter().find(|w| w.kind() == lgwks_ast::script::Kind::Each).ok_or("each")?;
/// assert_eq!(each.kind().spelling(), "each");
/// # Ok::<(), &str>(())
/// ```
#[derive(Debug)]
pub struct Word {
    /// What the word does; its spelling is [`Kind::spelling`].
    kind: Kind,
    /// Where it may stand.
    position: Position,
    /// The `lgwks_bot::script` primitive the word expands to a call of.
    primitive: &'static str,
    /// Its written forms.
    forms: &'static [Form],
    /// What the word itself refuses.
    refuses: &'static [&'static str],
    /// What holds wherever the word is used, each with its proof (SL-4).
    guarantees: &'static [Guarantee],
    /// A whole script using the word.
    example: &'static str,
}

impl Word {
    /// What the word does; its spelling is [`Kind::spelling`].
    ///
    /// # Examples
    /// ```
    /// assert_eq!(lgwks_ast::script::LEXICON[0].kind(), lgwks_ast::script::Kind::Flow);
    /// ```
    #[must_use]
    pub const fn kind(&self) -> Kind {
        self.kind
    }

    /// Where it may stand.
    ///
    /// # Examples
    /// ```
    /// assert_eq!(lgwks_ast::script::LEXICON[0].position(), lgwks_ast::script::Position::Flow);
    /// ```
    #[must_use]
    pub const fn position(&self) -> Position {
        self.position
    }

    /// The `lgwks_bot::script` primitive the word expands to a call of, as
    /// Markdown: code in backticks, and prose, such as a word that expands to
    /// plain Rust, as prose.
    ///
    /// # Examples
    /// ```
    /// assert_eq!(lgwks_ast::script::LEXICON[0].primitive(), "`Scope::enter`");
    /// ```
    #[must_use]
    pub const fn primitive(&self) -> &'static str {
        self.primitive
    }

    /// Its written forms. A word documented inside another word's form
    /// (`else` in `if`'s) has none of its own.
    ///
    /// # Examples
    /// ```
    /// assert!(lgwks_ast::script::LEXICON.iter().all(|w| w.kind() == lgwks_ast::script::Kind::Else || !w.forms().is_empty()));
    /// ```
    #[must_use]
    pub const fn forms(&self) -> &'static [Form] {
        self.forms
    }

    /// What the word itself refuses, beyond the refusals every passthrough
    /// line carries.
    ///
    /// # Examples
    /// ```
    /// let each = lgwks_ast::script::LEXICON.iter().find(|w| w.kind() == lgwks_ast::script::Kind::Each).ok_or("each")?;
    /// assert!(each.refuses().contains(&"a bound outside `1..=65536`"));
    /// # Ok::<(), &str>(())
    /// ```
    #[must_use]
    pub const fn refuses(&self) -> &'static [&'static str] {
        self.refuses
    }

    /// What holds wherever the word is used, each with its proof (SL-4).
    ///
    /// # Examples
    /// ```
    /// assert!(lgwks_ast::script::LEXICON.iter().all(|w| !w.guarantees().is_empty()));
    /// ```
    #[must_use]
    pub const fn guarantees(&self) -> &'static [Guarantee] {
        self.guarantees
    }

    /// A whole script using the word, which `lgwks_macros`' tests expand.
    ///
    /// # Examples
    /// ```
    /// assert!(lgwks_ast::script::LEXICON.iter().all(|w| w.example().starts_with("flow ")));
    /// ```
    #[must_use]
    pub const fn example(&self) -> &'static str {
        self.example
    }
}

/// A written form and what it means.
const fn form(written: &'static str, means: &'static str) -> Form {
    Form { written, means }
}

/// Every word, in the order the documentation lists them.
///
/// # Examples
/// ```
/// let words: Vec<&str> = lgwks_ast::script::LEXICON.iter().map(|w| w.kind().spelling()).collect();
/// assert_eq!(&words[..3], ["flow", "each", "within"]);
/// ```
pub const LEXICON: [Word; 30] = [
    words::FLOW,
    words::EACH,
    words::WITHIN,
    words::RETRY,
    words::TOGETHER,
    words::STEP,
    words::FOR,
    words::IF,
    words::ELSE,
    words::LET,
    words::RUN,
    words::OBSERVE,
    words::ACT,
    words::GIVE_BACK,
    words::FAIL,
    vocab::GRAPH,
    vocab::NODE,
    vocab::SAGA,
    vocab::COMPENSATE,
    vocab::WHENEVER,
    vocab::JUSTIFY,
    vocab::REMEMBER,
    vocab::ADMIT,
    vocab::READY,
    vocab::APPROVAL,
    vocab::FETCH,
    vocab::ASK,
    vocab::SEND,
    vocab::SERVE,
    vocab::ROUTE,
];

/// The row `token` names, when it is a word.
///
/// The identifier is rendered once and compared with each spelling, rather
/// than asked of each row, so a lookup costs one rendering however long the
/// table grows.
pub(crate) fn word(token: &TokenTree) -> Option<&'static Word> {
    let name = ident(token)?.to_string();
    LEXICON.iter().find(|row| row.kind.spelling() == name)
}

/// Whether `token` is the word `kind`.
pub(crate) fn is(token: &TokenTree, kind: Kind) -> bool {
    word(token).is_some_and(|row| row.kind == kind)
}

/// The word a line starts with, if it starts with one.
pub(crate) fn kind_of(line: &Line) -> Option<Kind> {
    line.tokens.first().and_then(word).map(|row| row.kind)
}

/// The block words, as the refusal for an unknown block lists them: in lexicon
/// order, `else` written with the `if` it belongs to, `node` and `compensate`
/// and `route` left to the `graph`, `saga` and `serve` they stand in, and `let`,
/// in the form that opens a block, last after an "or".
pub(crate) fn block_words() -> String {
    let mut words: Vec<String> = LEXICON
        .iter()
        .filter(|row| row.position == Position::Block)
        .filter_map(|row| match row.kind {
            Kind::Else | Kind::Node | Kind::Compensate | Kind::Route | Kind::Let => None,
            Kind::If => Some(format!(
                "`{}`/`{}`",
                Kind::If.spelling(),
                Kind::Else.spelling()
            )),
            kind => Some(format!("`{}`", kind.spelling())),
        })
        .collect();
    words.push(format!("`{} x = <block>:`", Kind::Let.spelling()));
    match words.pop() {
        Some(last) if !words.is_empty() => format!("{}, or {last}", words.join(", ")),
        Some(last) => last,
        None => String::new(),
    }
}
