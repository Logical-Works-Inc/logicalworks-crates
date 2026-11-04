//! Spans, severities, and the diagnostics a tool reports on code.
//!
//! The parser's refusal type is `ParseError`; this module is what a
//! tool actually renders. A [`Diagnostic`] carries the four things a report line
//! needs and the refusal type does not: which file, where in it, how bad, and
//! what to say.
//!
//! The positional data is not invented. ast-grep already computes a byte range
//! and a line/byte-column pair for every node (`Node::range`, `Node::start_pos`),
//! and nothing in this crate consumed it; that is the gap this module fills.
//! Spans are built from those values, and a span that would leave the source is
//! clamped rather than trusted, because a grammar is third-party code and its
//! arithmetic is not this crate's to vouch for.
//!
//! Line and column are **1-based** here, matching what an editor shows and what
//! every diagnostic printer assumes. ast-grep counts from zero; the conversion
//! happens once, here, so a caller never has to remember which convention a
//! given number is in. `byte` is always the raw zero-based offset, because that
//! is what [`str::get`] and slicing take.
//!
//! The walk is the same cursor walk `inspect_ast` uses, so collecting
//! diagnostics over a wide tree retains memory proportional to depth rather
//! than to fan-out.

use std::ops::Range;
use std::path::{Path, PathBuf};

#[cfg(feature = "parser")]
use ast_grep_core::tree_sitter::LanguageExt;

#[cfg(feature = "parser")]
use crate::AstNode;

/// One position in a source file.
///
/// 1-based `line` and `column`, zero-based `byte`. `column` counts characters
/// rather than bytes, so a line containing multi-byte text reports the column a
/// person would count; `byte` is the offset a slice takes.
///
/// # Examples
///
/// ```
/// use lgwks_ast::Pos;
///
/// let first = Pos::new(0, 0, 0);
/// assert_eq!((first.line, first.column, first.byte), (1, 1, 0));
/// assert!(first < Pos::new(1, 0, 10));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct Pos {
    /// 1-based line number.
    pub line: usize,
    /// 1-based character column.
    pub column: usize,
    /// Zero-based byte offset from the start of the file.
    pub byte: usize,
}

impl Pos {
    /// Build a position from zero-based counts, normalising to the 1-based
    /// convention this crate reports.
    ///
    /// A zero line or column becomes 1, because position zero is what the
    /// underlying grammar emits for the first line's first column, and a report
    /// reading "line 0" is a defect in the report rather than in the file.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_ast::Pos;
    ///
    /// let pos = Pos::new(2, 4, 30);
    /// assert_eq!((pos.line, pos.column, pos.byte), (3, 5, 30));
    /// ```
    #[must_use]
    pub fn new(line: usize, column: usize, byte: usize) -> Self {
        Self {
            line: line.saturating_add(1),
            column: column.saturating_add(1),
            byte,
        }
    }
}

/// A half-open byte range, with the 1-based line/column of each end.
///
/// # Examples
///
/// ```
/// use lgwks_ast::diagnostic::end_of;
///
/// let span = end_of("fn main() {}\n");
/// assert_eq!(span.start, span.end);
/// assert_eq!(span.start.line, 2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Span {
    /// First position covered.
    pub start: Pos,
    /// One past the last position covered.
    pub end: Pos,
}

impl Span {
    /// The byte range this span covers, for slicing a source file.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_ast::diagnostic::end_of;
    ///
    /// let source = "let x = 1;";
    /// assert_eq!(end_of(source).byte_range(), source.len()..source.len());
    /// ```
    #[must_use]
    pub fn byte_range(&self) -> Range<usize> {
        self.start.byte..self.end.byte
    }

    /// A zero-width span at `pos`: the point a whole-file condition applies at.
    ///
    /// A refusal about a file's size is not a position inside the file, so it is
    /// reported at the end of the file rather than at some invented interior
    /// line. A renderer should not underline an empty span.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_ast::{Pos, Span};
    ///
    /// let span = Span::at(Pos::new(0, 3, 3));
    /// assert!(span.is_empty());
    /// assert_eq!(span.byte_range(), 3..3);
    /// ```
    #[must_use]
    pub fn at(pos: Pos) -> Self {
        Self {
            start: pos,
            end: pos,
        }
    }

    /// Whether this span covers no bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_ast::{Pos, Span};
    ///
    /// assert!(Span::at(Pos::new(0, 0, 0)).is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start.byte == self.end.byte
    }
}

/// How much a diagnostic matters.
///
/// The split is between *the parse was refused* and *the parse succeeded and
/// recovered*. A refusal is [`Severity::Error`] and there is no tree to inspect.
/// A recovery node is [`Severity::Warning`]: the tool got an answer, and the
/// answer may be wrong from that point on. Collapsing the two loses the only
/// information a reader needs to decide whether to trust the rest of the parse.
///
/// # Examples
///
/// ```
/// use lgwks_ast::{Diagnostic, Pos, Severity, Span};
///
/// let found = Diagnostic::new("unused import", Span::at(Pos::new(0, 0, 0)));
/// assert_eq!(found.severity(), Severity::Warning);
/// assert_eq!(found.with_severity(Severity::Error).severity(), Severity::Error);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Severity {
    /// The source was refused; no usable tree exists.
    Error,
    /// The tree was produced but carries a recovery node.
    Warning,
}

impl Severity {
    /// The conventional single-character label for this severity.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_ast::Severity;
    ///
    /// assert_eq!(Severity::Error.label(), 'E');
    /// assert_eq!(Severity::Warning.label(), 'W');
    /// ```
    #[must_use]
    pub fn label(self) -> char {
        match self {
            Self::Error => 'E',
            Self::Warning => 'W',
        }
    }
}

/// One reportable finding against a source file.
///
/// The fields are readable but not publicly writable. A finding is a claim
/// about a source location: the message, the severity and the span have to
/// describe *one* thing at *one* place. Public mutable fields would let a
/// caller re-point `span` while leaving a message that describes the old
/// location, and the result still renders as well-formed output — a plausible
/// finding at an implausible place, which is worse than no finding. Build with
/// [`Diagnostic::new`]; [`Diagnostic::with_severity`] is the one adjustment this
/// type supports, and it exists because escalating or dropping a warning is a
/// decision a policy layer makes, not a field poke.
///
/// # Examples
///
/// ```
/// use lgwks_ast::{Diagnostic, Pos, Severity, Span};
///
/// let found = Diagnostic::new("expected a semicolon", Span::at(Pos::new(4, 9, 80)))
///     .in_file("src/lib.rs")
///     .with_severity(Severity::Error);
/// assert_eq!(found.render(), "src/lib.rs:5:10: E: expected a semicolon");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Diagnostic {
    /// The file the finding is against. This is a label carried through the
    /// report; this crate does not read the file and does not check that the
    /// bytes it parsed are the bytes this path holds now.
    file: PathBuf,
    /// How much the finding matters.
    severity: Severity,
    /// What to say, in one line.
    message: String,
    /// Where in the file the finding is.
    span: Span,
}

impl Diagnostic {
    /// A warning-level finding at `span`.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_ast::{Diagnostic, Pos, Severity, Span};
    ///
    /// let found = Diagnostic::new("unused variable", Span::at(Pos::new(0, 4, 4)));
    /// assert_eq!(found.message(), "unused variable");
    /// assert_eq!(found.severity(), Severity::Warning);
    /// ```
    #[must_use]
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            file: PathBuf::new(),
            severity: Severity::Warning,
            message: message.into(),
            span,
        }
    }

    /// The same finding, reported against `file`.
    ///
    /// A label only: this crate does not read the path, so nothing here checks
    /// that the bytes it parsed are the bytes that path holds now.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use lgwks_ast::{Diagnostic, Pos, Span};
    ///
    /// let found = Diagnostic::new("unused variable", Span::at(Pos::new(0, 0, 0))).in_file("src/main.rs");
    /// assert_eq!(found.file(), Path::new("src/main.rs"));
    /// ```
    #[must_use]
    pub fn in_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = file.into();
        self
    }

    /// The same finding at a different severity.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_ast::{Diagnostic, Pos, Severity, Span};
    ///
    /// let found = Diagnostic::new("deny lint", Span::at(Pos::new(0, 0, 0))).with_severity(Severity::Error);
    /// assert_eq!(found.severity(), Severity::Error);
    /// ```
    #[must_use]
    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// The file this finding is reported against.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use lgwks_ast::{Diagnostic, Pos, Span};
    ///
    /// let bare = Diagnostic::new("no file yet", Span::at(Pos::new(0, 0, 0)));
    /// assert_eq!(bare.file(), Path::new(""));
    /// assert_eq!(bare.in_file("a.rs").file(), Path::new("a.rs"));
    /// ```
    #[must_use]
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// How much the finding matters.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_ast::{Diagnostic, Pos, Severity, Span};
    ///
    /// let found = Diagnostic::new("recovered", Span::at(Pos::new(0, 0, 0)));
    /// assert_eq!(found.severity(), Severity::Warning);
    /// ```
    #[must_use]
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// What to say, in one line.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_ast::{Diagnostic, Pos, Span};
    ///
    /// let found = Diagnostic::new("missing semicolon", Span::at(Pos::new(0, 0, 0)));
    /// assert_eq!(found.message(), "missing semicolon");
    /// ```
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Where in the file the finding is.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_ast::{Diagnostic, Pos, Span};
    ///
    /// let at = Span::at(Pos::new(1, 2, 12));
    /// let found = Diagnostic::new("here", at);
    /// assert_eq!(found.span(), &at);
    /// assert_eq!(found.span().start.line, 2);
    /// ```
    #[must_use]
    pub fn span(&self) -> &Span {
        &self.span
    }

    /// The diagnostic rendered the way `rustc` renders one, without a trailing
    /// newline: `path:line:column: severity: message`.
    ///
    /// Built from the fields on every call rather than stored, so a caller that
    /// adjusts a field before rendering does not get the pre-edit text back.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_ast::{Diagnostic, Pos, Span};
    ///
    /// let found = Diagnostic::new("unused import", Span::at(Pos::new(0, 0, 0))).in_file("src/lib.rs");
    /// assert_eq!(found.render(), "src/lib.rs:1:1: W: unused import");
    /// ```
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "{}:{}:{}: {}: {}",
            self.file.display(),
            self.span.start.line,
            self.span.start.column,
            self.severity.label(),
            self.message
        )
    }
}

/// A walked node's line/column, resolved against the source.
///
/// ast-grep reports a zero-based line with a zero-based **byte** column, and
/// its own `column()` accessor is an O(n) scan of the source. Neither is what a
/// report wants: a byte column on a line containing multi-byte text counts
/// bytes a person cannot see. This indexes every line start once, so converting
/// a span is two binary-search steps and a slice of one line regardless of how
/// many findings the file holds.
struct LineIndex<'src> {
    /// The text spans are resolved against. Borrowed, never copied: the caller
    /// already holds it and copying it per file would double peak memory on a
    /// file near [`MAX_SOURCE_BYTES`].
    source: &'src str,
    /// Byte offset of the start of each line, always beginning with 0.
    line_starts: Vec<usize>,
}

impl<'src> LineIndex<'src> {
    /// Index every line start in `source`.
    fn new(source: &'src str) -> Self {
        let mut line_starts = vec![0];
        // `+ 1` is a saturating step past the newline, not a wrapping one: the
        // offset of a character is always strictly less than the length of the
        // string it indexes, so this cannot reach `usize::MAX`.
        line_starts.extend(
            source
                .match_indices('\n')
                .map(|(at, _)| at.saturating_add(1)),
        );
        Self {
            source,
            line_starts,
        }
    }

    /// The 1-based line number holding `byte`.
    ///
    /// The partition point is the first line start strictly after `byte`, so
    /// `byte` belongs to the line before it. A trailing newline puts a final
    /// start equal to `source.len()`, and the strict comparison keeps the last
    /// real line from being counted twice.
    fn line_of(&self, byte: usize) -> usize {
        self.line_starts.partition_point(|start| *start <= byte)
    }

    /// The 1-based position of `byte` within the file, clamped to the source
    /// and floored to a character boundary.
    ///
    /// A span can come from another copy of the text than the one passed here,
    /// so an offset inside a multi-byte character is possible; it resolves to
    /// that character's first byte rather than panicking on the slice below.
    fn position(&self, byte: usize) -> Pos {
        let byte = self.source.floor_char_boundary(byte);
        let line = self.line_of(byte);
        // `line_starts` always holds at least one entry and `line_of` returns a
        // count in `1..=line_starts.len()`, so the index is in range. A byte
        // landing exactly on a line start resolves to that line, and `saturating_sub`
        // covers the first line, whose start is the 0th entry.
        let line_start = self.line_starts[line.saturating_sub(1)];
        let column = self.source[line_start..byte].chars().count();
        Pos::new(line.saturating_sub(1), column, byte)
    }

    /// The span covering `range`, with every end clamped into the source.
    fn span(&self, range: Range<usize>) -> Span {
        Span {
            start: self.position(range.start),
            end: self.position(range.end),
        }
    }
}

/// Collect one diagnostic per recovery node in `root`'s tree.
///
/// A recovery node is an `ERROR` node or a `MISSING` one: the parser reached
/// text it could not fit the grammar to, and guessed. Every such node becomes a
/// [`Severity::Warning`] carrying the exact span of the offending text, so a
/// caller can underline it. A clean tree yields an empty `Vec`.
///
/// `language` names the grammar in each message; `recovery_message` is why it
/// is a parameter rather than something read off the node.
///
/// The reported order is source order, whatever shape the tree is, because a
/// reader scanning a list of findings expects to walk down the file.
///
/// A source whose second function is malformed reports its recovery as a
/// warning on the second line, never as an error; the unit tests
/// `a_recovery_is_reported_on_the_line_that_carries_it` and
/// `a_recovery_is_never_reported_as_an_error` run that source.
///
/// # Bounds
///
/// This is a scan of an *already parsed* tree and is deliberately not a bound a
/// caller can rely on: it is `O(nodes)` and unbounded, exactly like
/// [`inspect_ast`](crate::inspect_ast) with no limit. Reach for
/// [`try_parse`](crate::try_parse) first, which refuses a tree carrying recovery
/// nodes outright, and use this on a tree you already hold — for instance one
/// built by [`parse`](crate::parse), whose entire purpose is to inspect
/// malformed trees.
///
/// `source` must be the text `root` was parsed from. Every span here is a byte
/// offset into one specific string, and resolving an offset against a different
/// string yields a position that is *silently* wrong rather than loudly broken:
/// an off-by-one that reads as a real finding at a plausible line. The safe
/// entry point is [`tree_diagnostics`](crate::tree_diagnostics), which takes
/// the source from the tree and cannot be handed the wrong one.
///
/// # Examples
///
/// ```
/// use lgwks_ast::{Language, Severity, diagnostic::diagnostics, parse};
///
/// let source = "fn main() {}\nfn broken( {\n";
/// let tree = parse(source, Language::Rust);
/// let found = diagnostics("src/lib.rs", &tree.root(), source, "Rust");
/// assert!(!found.is_empty());
/// assert!(found.iter().all(|d| d.severity() == Severity::Warning));
/// assert_eq!(found[0].span().start.line, 2);
/// ```
#[must_use]
#[cfg(feature = "parser")]
pub fn diagnostics<L: LanguageExt>(
    path: impl Into<PathBuf>,
    root: &AstNode<'_, L>,
    source: &str,
    language: &str,
) -> Vec<Diagnostic> {
    let file = labeled(path.into());
    let index = LineIndex::new(source);
    let mut found = Vec::new();
    visit_each_node(root, &mut |node| {
        if node.is_error || node.is_missing {
            found.push(
                Diagnostic::new(
                    recovery_message(node.is_missing, node.kind, language),
                    index.span(node.span.clone()),
                )
                .in_file(file.clone()),
            );
        }
    });
    // The walk already arrives in source order, and this sort keeps the
    // published order a property of `diagnostics` rather than of the traversal
    // that happens to feed it.
    found.sort_by_key(|diagnostic| (diagnostic.span.start.byte, diagnostic.span.end.byte));
    found
}

/// Whether a parsed tree carries recovery nodes, and where.
///
/// This is `diagnostics` for a caller that wants a count rather than a list,
/// and it is the honest form of the question `has_syntax_issues` answers: that
/// function reports a single sticky bit, which cannot say *how many* nodes the
/// parser gave up on or *where*. Use it to decide whether to keep going; use
/// this to tell a reader where the damage is.
///
/// # Examples
///
/// ```
/// use lgwks_ast::{Language, diagnostic::recovery_count, parse};
///
/// assert_eq!(recovery_count(&parse("fn main() {}", Language::Rust).root()), 0);
/// assert!(recovery_count(&parse("fn broken( {}", Language::Rust).root()) > 0);
/// ```
#[must_use]
#[cfg(feature = "parser")]
pub fn recovery_count<L: LanguageExt>(root: &AstNode<'_, L>) -> usize {
    let mut count = 0_usize;
    visit_each_node(root, &mut |node| {
        if node.is_error || node.is_missing {
            count = count.saturating_add(1);
        }
    });
    count
}

/// What one visited node looked like, detached from the cursor that reached it.
///
/// The visitor is handed facts rather than a node handle because a handle
/// borrows the cursor's position, and collecting handles would be the sibling
/// frontier this traversal exists not to build. Every field is read off the
/// node before the cursor moves on; `kind` borrows the tree rather than the
/// node, so it outlives the handle it came from.
#[cfg(feature = "parser")]
struct Found<'tree> {
    /// Whether the node is an `ERROR` recovery node.
    is_error: bool,
    /// Whether the node is a `MISSING` recovery node.
    is_missing: bool,
    /// The node's grammar symbol name.
    kind: &'tree str,
    /// Inclusive start and exclusive end byte offsets in the source.
    span: Range<usize>,
}

/// Visit every node at or below `root` exactly once, depth-first, in bounded
/// memory.
///
/// The walk drives a tree-sitter cursor, whose only retained state is the stack
/// of ancestors it has descended through, mirroring `inspect_ast_with_pending`
/// (#277): one entry per *active* ancestor, so retained memory follows depth
/// rather than the root's fan-out, and a level is popped at the end of its
/// branch rather than left pending. A file with N top-level items therefore
/// costs O(depth) resident memory, not O(N).
///
/// Children are visited in source order, which is the order
/// [`inspect_ast`] now reports. Callers that present results in source order
/// get them in source order; the sort at the end of [`diagnostics`] is retained
/// so the published order does not depend on it.
///
/// The root is visited first, then every descendant. `inspect_ast` charges and
/// inspects the root too, so the two walks agree on which nodes exist: a tree
/// whose root is itself a recovery node is counted here exactly as it is
/// refused by [`try_parse`].
#[cfg(feature = "parser")]
fn visit_each_node<L: LanguageExt>(root: &AstNode<'_, L>, visit: &mut dyn FnMut(&Found<'_>)) {
    visit_each_node_measuring(root, visit, None);
}

/// [`visit_each_node`], reporting the high-water mark of retained cursor depth.
///
/// The `peak` out-parameter is how INV-AST-1's resource claim is *tested*
/// against this walk rather than against a second copy of it. A test that
/// reimplemented the traversal would keep passing if this one regressed, which
/// is the failure mode a copied walk always has.
#[cfg(feature = "parser")]
fn visit_each_node_measuring<L: LanguageExt>(
    root: &AstNode<'_, L>,
    visit: &mut dyn FnMut(&Found<'_>),
    mut peak: Option<&mut usize>,
) {
    let mut cursor = root.get_inner_node().walk();
    let mut cursor_depth = 0_usize;
    loop {
        let node = cursor.node();
        let range = node.range();
        visit(&Found {
            is_error: node.is_error(),
            is_missing: node.is_missing(),
            kind: node.kind(),
            span: range.start_byte..range.end_byte,
        });
        if let Some(peak) = peak.as_deref_mut() {
            *peak = (*peak).max(cursor_depth.saturating_add(1));
        }
        if cursor.goto_first_child() {
            cursor_depth = cursor_depth.saturating_add(1);
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
            cursor_depth = cursor_depth.saturating_sub(1);
        }
    }
}

/// Peak depth [`visit_each_node`] descends to while walking `root`.
///
/// One walk, run for its side effect of measuring. Test-only by construction:
/// it exists so INV-AST-1's resource claim is asserted against the walk that
/// actually ships rather than against a copy of it.
#[cfg(all(test, feature = "lang-rust"))]
#[must_use]
fn peak_retained_frames<L: LanguageExt>(root: &AstNode<'_, L>) -> usize {
    let mut peak = 0_usize;
    visit_each_node_measuring(root, &mut |_| {}, Some(&mut peak));
    peak
}

/// What to say about one recovery node.
///
/// An `ERROR` node is text the grammar could not parse. A `MISSING` node is
/// text the grammar expected and did not find, so it spans no source of its own
/// and reads differently.
///
/// The language is named by `language`, which the caller supplies rather than
/// this module reading off the node. ast-grep's `Language` trait has no `name()`
/// — the display name belongs to this crate's own [`Language`] enum, not to the
/// generic grammar — so a message built from the node alone could not say which
/// grammar it meant. Taking it as a parameter keeps the two facts in one place:
/// the caller named the grammar it parsed with, and the message names that.
#[cfg(feature = "parser")]
fn recovery_message(node_kind_missing: bool, node_kind: &str, language: &str) -> String {
    if node_kind_missing {
        format!("{language} is missing {node_kind}")
    } else {
        format!("{language} could not parse this text")
    }
}

/// A file label that renders as something rather than nothing.
///
/// An empty path renders as `:1:1: E: …`, which reads as a truncated filename
/// instead of as "no file was given". The placeholder keeps the shape of the
/// line and says plainly that the file is unknown.
#[cfg(feature = "parser")]
fn labeled(path: PathBuf) -> PathBuf {
    if path.as_os_str().is_empty() {
        PathBuf::from("<source>")
    } else {
        path
    }
}

/// The offset just past the last byte of `source`, as a zero-width span.
///
/// The end of a file is where a whole-file refusal is reported, so this is what
/// the parser's `ParseError::to_diagnostic` anchors its
/// size refusals to.
///
/// # Examples
///
/// ```
/// use lgwks_ast::diagnostic::end_of;
///
/// let span = end_of("ab\ncd");
/// assert_eq!((span.start.line, span.start.column, span.start.byte), (2, 3, 5));
/// assert!(span.is_empty());
/// ```
#[must_use]
pub fn end_of(source: &str) -> Span {
    LineIndex::new(source).span(source.len()..source.len())
}

/// The span of `range` in `source`, each end clamped into the source.
///
/// How a checked parse's [`SyntaxDiagnostic`] byte
/// offsets become the line and column a renderer points at.
#[cfg(feature = "parser")]
pub(crate) fn span_of(source: &str, range: Range<usize>) -> Span {
    LineIndex::new(source).span(range)
}

// The fixtures here are Rust source, so the tests need the Rust grammar. The
// span arithmetic itself is language-independent and lives in `LineIndex`,
// which is exercised by the clamped-position test below without a grammar.
#[cfg(all(test, feature = "lang-rust"))]
mod tests;

/// The line arithmetic, with no grammar involved.
///
/// These hold at every feature set, so they are the tests that keep
/// [`LineIndex`] honest even in a build carrying no grammar at all — the case a
/// consumer of `--no-default-features` is in.
#[cfg(test)]
mod line_index_tests;
