//! The architecture map's entries: a flow's [`FlowShape`], the
//! [`StepShape`] of every step inside it, and the lines it passes through as
//! plain Rust. [`write_map`](super::write_map) prints them.

use super::super::lexicon::Kind;

/// A flow's entry in the architecture map.
///
/// # Examples
/// ```
/// use lgwks_ast::script::parse;
///
/// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
/// let shape = script.flows()[0].shape();
/// assert_eq!((shape.name(), shape.line()), ("sizes", 1));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowShape {
    /// The flow's name.
    pub(crate) name: String,
    /// `name(inputs) -> Output`, as written.
    pub(crate) signature: String,
    /// The 1-based source line of the `flow` header, as the compiler reports it.
    pub(crate) line: usize,
    /// The steps inside, in source order.
    pub(crate) steps: Vec<StepShape>,
    /// The lines passed through as plain Rust, in source order (SL-3).
    pub(crate) passthrough: Vec<Passthrough>,
}

impl FlowShape {
    /// The identifier after `flow`, as the architecture map lists it.
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
    pub const fn name(&self) -> &str {
        self.name.as_str()
    }

    /// `name(inputs) -> Output`, as written.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// assert_eq!(script.flows()[0].shape().signature(), "sizes(pages: Vec<String>) -> Vec<usize>");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn signature(&self) -> &str {
        self.signature.as_str()
    }

    /// The source line of the `flow` header.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// assert_eq!(script.flows()[0].shape().line(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }

    /// The steps inside, in source order.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Kind, parse};
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// let kinds: Vec<Kind> = script.flows()[0].shape().steps().iter().map(|step| step.kind()).collect();
    /// assert_eq!(kinds, [Kind::Each, Kind::GiveBack]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn steps(&self) -> &[StepShape] {
        self.steps.as_slice()
    }

    /// The lines this flow passes through as plain Rust, in source order: the
    /// lines no word reads, which the word-admission census counts (SL-7).
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("flow tidy(mut items: Vec<u8>) -> usize:\n    items.sort();\n    give back items.len()\n".parse()?)?;
    /// let kept = &script.flows()[0].shape().passthrough()[0];
    /// assert_eq!(kept.line(), 2);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn passthrough(&self) -> &[Passthrough] {
        self.passthrough.as_slice()
    }
}

/// A line a flow passes through as written: no word, and no `run`,
/// `observe` or `act` inside it (SL-3).
///
/// # Examples
/// ```
/// use lgwks_ast::script::parse;
///
/// let script = parse("flow tidy(mut items: Vec<u8>) -> usize:\n    items.sort();\n    give back items.len()\n".parse()?)?;
/// let kept = &script.flows()[0].shape().passthrough()[0];
/// assert_eq!((kept.line(), kept.text()), (2, "items.sort();"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passthrough {
    /// The 1-based source line, as the compiler reports it.
    pub(crate) line: usize,
    /// The line as written.
    pub(crate) text: String,
}

impl Passthrough {
    /// The 1-based source line, as the compiler reports it.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("flow tidy(mut items: Vec<u8>) -> usize:\n    items.sort();\n    give back items.len()\n".parse()?)?;
    /// assert_eq!(script.flows()[0].shape().passthrough()[0].line(), 2);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }

    /// The line as written.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("flow tidy(mut items: Vec<u8>) -> usize:\n    items.sort();\n    give back items.len()\n".parse()?)?;
    /// assert_eq!(script.flows()[0].shape().passthrough()[0].text(), "items.sort();");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn text(&self) -> &str {
        self.text.as_str()
    }
}

/// One step in the architecture map.
///
/// # Examples
/// ```
/// use lgwks_ast::script::{Kind, parse};
///
/// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
/// let step = &script.flows()[0].shape().steps()[0];
/// assert_eq!((step.kind(), step.line()), (Kind::Each, 2));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepShape {
    /// The word the step is: never `flow`, whose entry is a [`FlowShape`], and
    /// never `let`, whose entry is the block it binds.
    pub(crate) kind: Kind,
    /// The step's subject: the bound, the deadline, the attempt count, the step's name, the callee, or the branch count.
    pub(crate) subject: String,
    /// The line as written.
    pub(crate) detail: String,
    /// The source line.
    pub(crate) line: usize,
    /// The steps inside, in source order.
    pub(crate) children: Vec<Self>,
}

impl StepShape {
    /// The word the step is: never `flow`, whose entry is a [`FlowShape`], and
    /// never `let`, whose entry is the block it binds.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::{Kind, parse};
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// assert_eq!(script.flows()[0].shape().steps()[0].kind(), Kind::Each);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn kind(&self) -> Kind {
        self.kind
    }

    /// The step's subject: the bound, the deadline, the attempt count, the step's name, the callee, or the branch count.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// assert_eq!(script.flows()[0].shape().steps()[0].subject(), "sized to the machine");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn subject(&self) -> &str {
        self.subject.as_str()
    }

    /// The line as written.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// assert_eq!(script.flows()[0].shape().steps()[0].detail(), "each page in pages");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn detail(&self) -> &str {
        self.detail.as_str()
    }

    /// The 1-based source line the step starts on, as the compiler reports it.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// assert_eq!(script.flows()[0].shape().steps()[0].line(), 2);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }

    /// The steps inside, in source order.
    ///
    /// # Examples
    /// ```
    /// use lgwks_ast::script::parse;
    ///
    /// let script = parse("flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n".parse()?)?;
    /// assert!(script.flows()[0].shape().steps()[0].children().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn children(&self) -> &[Self] {
        self.children.as_slice()
    }
}
