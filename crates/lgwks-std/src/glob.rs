//! Pure matching of Unicode text against the `lgwks_std` glob dialect.
//!
//! `?` and classes consume one Unicode scalar value; `*` consumes zero or more
//! scalar values except `/`; `**` may consume `/`. Matching is case-sensitive,
//! does not normalize Unicode, and treats a backslash as an ordinary literal.
//! Leading dots are ordinary characters. These are text semantics, not
//! grapheme-cluster or native `OsStr` path semantics.
//!
//! The checked
//! [GlobPattern::compile](crate::glob::GlobPattern::compile) entry accepts the
//! strict dialect:
//! unclosed or empty classes, descending ranges, and `**` embedded inside a
//! path component are errors.
//! [GlobDialect::Legacy](crate::glob::GlobDialect::Legacy) preserves the
//! earlier boolean matcher's literal unclosed brackets, descending-range no-match, and
//! `**` anywhere behavior. The one-call [`matches`] function continues to use
//! that named legacy dialect for source compatibility.
//!
//! This module replaces only the matching portion of the upstream `glob`
//! crate. It does not walk directories or implement POSIX shell expansion.
//! The upstream crate rejects unclosed classes and constrains `**`; this
//! dialect deliberately keeps the documented `a/**/b`, `**/b`, `a/**`, and
//! legacy `a**b` forms. `globset` targets compiled sets of filesystem patterns,
//! which is a different job and adds an external dependency. A compiled NFA
//! with two rolling rows keeps this crate's zero-dependency single-pattern
//! contract while bounding each token transition to one path scan.

use core::fmt;

/// The rolling DP rows the matcher retains: the previous token's row and the
/// one being written. A third would buy nothing, since only these two are ever
/// read.
const ROLLING_ROWS: usize = 2;

/// The longest path, in Unicode scalars, that [`GlobPattern::is_match`] matches
/// in stack buffers. A longer path takes a fresh heap scratch for that one call.
const STACK_SCALARS: usize = 256;

/// The length of one stack rolling row: one entry per scalar plus the
/// terminating position.
const STACK_ROW: usize = 257;

/// Selects the syntax accepted while compiling a glob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GlobDialect {
    /// Reject malformed classes, descending ranges, and component-embedded `**`.
    Strict,
    /// Preserve the historical `matches` behavior for migration.
    Legacy,
}

/// A reason a strict glob pattern could not be compiled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PatternErrorKind {
    /// A `[` had no matching `]`.
    UnclosedClass,
    /// A range's first scalar sorts after its last scalar.
    DescendingRange,
    /// A `**` sequence was not a complete path component.
    DoubleStarPlacement,
}

/// A pattern compilation failure with a byte offset into the pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PatternError {
    /// The syntax error category.
    pub kind: PatternErrorKind,
    /// The UTF-8 byte offset where the invalid syntax begins.
    pub byte_offset: usize,
}

impl fmt::Display for PatternError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self.kind {
            PatternErrorKind::UnclosedClass => "character class is not closed",
            PatternErrorKind::DescendingRange => "character class range is descending",
            PatternErrorKind::DoubleStarPlacement => "** must occupy a complete path component",
        };
        write!(formatter, "{reason} at byte {}", self.byte_offset)
    }
}

impl std::error::Error for PatternError {}

/// Builds the typed refusal for `kind` at `byte_offset`, reporting it through
/// `trace` when that feature is on.
fn refusal(kind: PatternErrorKind, byte_offset: usize) -> PatternError {
    let error = PatternError { kind, byte_offset };
    #[cfg(feature = "trace")]
    crate::trace::debug!(error = ?error, "glob: returning an error to the caller");
    error
}

/// One inclusive Unicode scalar interval compiled from a class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScalarRange {
    /// Lowest included Unicode scalar.
    start: char,
    /// Highest included Unicode scalar.
    end: char,
}

/// One compiled transition in the pattern automaton.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    /// One exact Unicode scalar.
    Literal(char),
    /// Exactly one non-separator scalar.
    Question,
    /// Zero or more non-separator scalars.
    Star,
    /// Zero or more scalars, including separators.
    DoubleStar,
    /// Zero or more scalars ending at a separator, or no directory prefix.
    DoubleStarSlash,
    /// A separator followed by zero or more scalars.
    SlashDoubleStar,
    /// A separator-delimited run with an endpoint after a separator.
    SlashDoubleStarSlash,
    /// One scalar in or outside the inclusive ranges.
    Class {
        /// Sorted scalar ranges accepted by this class.
        ranges: Vec<ScalarRange>,
        /// Whether a scalar outside the ranges is accepted.
        negated: bool,
    },
}

/// A reusable, validated glob pattern.
///
/// Pattern storage is O(M), including the token list and compiled class
/// intervals. A caller that matches many paths should retain this value and a
/// [`GlobScratch`] to amortize compilation and matching allocations.
///
/// # Sharing one pattern across threads
///
/// A `GlobPattern` is `Send + Sync` and holds no caller data, so one compiled
/// pattern serves any number of concurrent callers. The mutable half is
/// [`GlobScratch`], which [`is_match_with`](Self::is_match_with) takes by
/// `&mut`: the pattern is what is shared, and the scratch is what a caller owns.
/// Two callers sharing a scratch share one caller's path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobPattern {
    /// The compiled sequence of matching transitions.
    tokens: Vec<Token>,
}

/// The compile-time assertion that one pattern is shareable across threads: a
/// compile error if [`GlobPattern`] gains interior mutability or a non-`Send`
/// field. `tests/sim_shared_policy_tiers.rs` writes the same assertion against
/// the published type, and the test module below carries the runtime probe.
#[cfg(test)]
const fn assert_shared_across_threads() {
    const fn assert<T: Send + Sync>() {}
    assert::<GlobPattern>();
    assert::<GlobScratch>();
}

impl GlobPattern {
    /// Compiles the strict dialect and returns malformed syntax as a typed error.
    ///
    /// `src/**/[a-z]?.rs` matches `src/a1.rs` and `src/nested/a1.rs` and not
    /// `src/nested/ab1.rs` (the integration test
    /// `public_compile_reuse_and_legacy_call_share_text_semantics` runs those
    /// paths through a reused [`GlobScratch`]).
    pub fn compile(pattern: &str) -> Result<Self, PatternError> {
        Self::compile_with_dialect(pattern, GlobDialect::Strict)
    }

    /// Compiles a pattern under an explicitly selected syntax policy.
    ///
    /// Use [`GlobDialect::Legacy`] when migrating callers that rely on the old
    /// permissive forms. New consumers should prefer [`Self::compile`].
    ///
    /// Under [`GlobDialect::Legacy`] the embedded `**` of `a**b` matches
    /// `a/x/b`, the same test as above.
    pub fn compile_with_dialect(pattern: &str, dialect: GlobDialect) -> Result<Self, PatternError> {
        let mut work = Work::default();
        compile_pattern(pattern, dialect, &mut work)
    }

    /// Matches a path in stack buffers: no allocation and no state shared
    /// between calls or callers.
    ///
    /// A path of up to 256 Unicode scalars is matched in two stack rows and a
    /// stack scalar index; a longer one takes a fresh [`GlobScratch`] for that
    /// call. A caller that owns a [`GlobScratch`] should call
    /// [`Self::is_match_with`]; the two produce the same answer.
    #[must_use]
    pub fn is_match(&self, path: &str) -> bool {
        let count = path.chars().count();
        let mut scalars = ['\0'; STACK_SCALARS];
        let mut previous = [0_u8; STACK_ROW];
        let mut next = [0_u8; STACK_ROW];
        if let (Some(live), Some(previous_row), Some(next_row)) = (
            scalars.get_mut(..count),
            previous.get_mut(..=count),
            next.get_mut(..=count),
        ) {
            for (slot, scalar) in live.iter_mut().zip(path.chars()) {
                *slot = scalar;
            }
            return run_rows(self, live, previous_row, next_row, &mut Work::new());
        }
        GlobScratch::new().match_path(self, path)
    }

    /// Matches a path while reusing scalar indexing and rolling-row storage.
    ///
    /// After the scratch capacities are sufficient for the path, matching
    /// performs no allocation per token. The caller owns the scratch lifetime;
    /// scratch contains only the last path and is overwritten on the next call.
    #[must_use]
    pub fn is_match_with(&self, path: &str, scratch: &mut GlobScratch) -> bool {
        scratch.match_path(self, path)
    }

    /// Returns the number of compiled tokens: the `O(M)` pattern storage,
    /// reported apart from the `O(N)` matching scratch.
    #[must_use]
    pub const fn token_count(&self) -> usize {
        self.tokens.len()
    }
}

/// Reusable matching memory owned by the caller.
///
/// Its buffers are O(N): Unicode scalar indexing plus two rolling DP rows.
/// Reusing it avoids allocating a row for each token or retaining path state in
/// the compiled pattern.
#[derive(Debug, Default)]
pub struct GlobScratch {
    /// Unicode scalar indexing for the current path.
    scalars: Vec<char>,
    /// Reachable offsets after the previous token.
    previous: Vec<u8>,
    /// Reachable offsets while processing the current token.
    next: Vec<u8>,
    /// Test-only measurements of parser and transition work.
    work: Work,
}

impl GlobScratch {
    /// Creates empty scratch buffers.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            scalars: Vec::new(),
            previous: Vec::new(),
            next: Vec::new(),
            work: Work::new(),
        }
    }

    /// Returns the retained capacity of the scalar index, in scalars: `O(N)` in
    /// the path's scalar count and the largest buffer the scratch owns.
    #[must_use]
    pub fn scalar_capacity(&self) -> usize {
        self.scalars.capacity()
    }

    /// Returns the retained byte capacity of each rolling row: one byte per
    /// scalar plus the terminating position.
    #[must_use]
    pub fn row_capacity(&self) -> usize {
        self.previous.capacity()
    }

    /// Returns the number of rolling rows the scratch retains: always two,
    /// reported so a caller accounting its own storage reads the type.
    #[must_use]
    pub const fn row_count(&self) -> usize {
        ROLLING_ROWS
    }

    /// Returns the total retained capacity across all three buffers, apart from
    /// the pattern's own `O(M)` storage ([`GlobPattern::token_count`]).
    #[must_use]
    pub fn storage_bytes(&self) -> usize {
        self.scalars
            .capacity()
            .saturating_mul(core::mem::size_of::<char>())
            .saturating_add(self.previous.capacity())
            .saturating_add(self.next.capacity())
    }

    /// Runs the two-row automaton over the retained buffers.
    fn match_path(&mut self, pattern: &GlobPattern, path: &str) -> bool {
        self.scalars.clear();
        self.scalars.extend(path.chars());
        let row_len = self.scalars.len().saturating_add(1);
        self.previous.resize(row_len, 0);
        self.next.resize(row_len, 0);
        run_rows(
            pattern,
            &self.scalars,
            &mut self.previous,
            &mut self.next,
            &mut self.work,
        )
    }
}

/// Runs the two-row automaton over `scalars`, counting each inspected state as
/// work. Both rows hold one entry per scalar plus the terminating position.
fn run_rows<'row>(
    pattern: &GlobPattern,
    scalars: &[char],
    mut previous: &'row mut [u8],
    mut next: &'row mut [u8],
    work: &mut Work,
) -> bool {
    // Only the empty prefix is reachable before the first token.
    previous.fill(0);
    if let Some(first) = previous.first_mut() {
        *first = 1;
    }
    work.reset();
    for token in &pattern.tokens {
        next.fill(0);
        step_token(token, scalars, previous, next, work);
        core::mem::swap(&mut previous, &mut next);
    }
    // The path is consumed when the final offset is reachable; a missing offset
    // is a non-match.
    previous
        .get(scalars.len())
        .is_some_and(|&reachable| reachable != 0)
}

/// Counts deterministic parser and transition operations in test builds only.
#[derive(Debug, Default)]
struct Work {
    /// Parser inspections, included only in unit-test builds.
    #[cfg(test)]
    parse: usize,
    /// DP and class lookup inspections, included only in unit-test builds.
    #[cfg(test)]
    transition: usize,
}

impl Work {
    /// Creates an empty operation counter.
    const fn new() -> Self {
        Self {
            #[cfg(test)]
            parse: 0,
            #[cfg(test)]
            transition: 0,
        }
    }

    /// Clears operation counts before one observable match.
    fn reset(&mut self) {
        #[cfg(test)]
        {
            self.transition = 0;
        }
    }

    /// Records one parser inspection in test builds.
    fn parser_step(&mut self) {
        #[cfg(test)]
        {
            self.parse = self.parse.saturating_add(1);
        }
    }

    /// Records one transition inspection in test builds.
    fn transition_step(&mut self) {
        #[cfg(test)]
        {
            self.transition = self.transition.saturating_add(1);
        }
    }
}

/// Compiles chars and class ranges once; tokens borrow no caller data.
fn compile_pattern(
    pattern: &str,
    dialect: GlobDialect,
    work: &mut Work,
) -> Result<GlobPattern, PatternError> {
    let chars: Vec<char> = pattern.chars().collect();
    let next_close = close_index(&chars, work);
    check_double_stars(&chars, &next_close, dialect, work)?;
    let tokens = compile_tokens(&chars, &next_close, dialect, work)?;
    Ok(GlobPattern { tokens })
}

/// Maps each scalar offset to the offset of the nearest `]` at or after it, in
/// one backward pass, with a terminal entry carrying the input's own length.
fn close_index(chars: &[char], work: &mut Work) -> Vec<usize> {
    let mut next_close = vec![chars.len(); chars.len().saturating_add(1)];
    let mut nearest = chars.len();
    for ((index, &scalar), slot) in chars.iter().enumerate().zip(next_close.iter_mut()).rev() {
        work.parser_step();
        if scalar == ']' {
            nearest = index;
        }
        *slot = nearest;
    }
    next_close
}

/// Refuses, under the strict dialect, a `**` run that is not a complete path
/// component. Classes are skipped whole, so a `**` inside one is not a run.
fn check_double_stars(
    chars: &[char],
    next_close: &[usize],
    dialect: GlobDialect,
    work: &mut Work,
) -> Result<(), PatternError> {
    let mut cursor = 0;
    while let Some(&current) = chars.get(cursor) {
        work.parser_step();
        if current == '[' {
            let (_, _, close) = class_bounds(chars, next_close, cursor);
            cursor = if close < chars.len() {
                close.saturating_add(1)
            } else if dialect == GlobDialect::Strict {
                chars.len()
            } else {
                cursor.saturating_add(1)
            };
        } else if current == '*' && chars.get(cursor.saturating_add(1)) == Some(&'*') {
            let run_start = cursor;
            while chars.get(cursor) == Some(&'*') {
                work.parser_step();
                cursor = cursor.saturating_add(1);
            }
            let left = run_start == 0 || chars.get(run_start.saturating_sub(1)) == Some(&'/');
            let right = cursor == chars.len() || chars.get(cursor) == Some(&'/');
            if dialect == GlobDialect::Strict
                && (cursor.saturating_sub(run_start) != 2 || !left || !right)
            {
                let at = byte_offset(chars, run_start);
                let refusal = Err(refusal(PatternErrorKind::DoubleStarPlacement, at));
                #[cfg(feature = "trace")]
                crate::trace::debug!(error = ?refusal.as_ref().err(), "check_double_stars: returning an error to the caller");
                return refusal;
            }
        } else {
            cursor = cursor.saturating_add(1);
        }
    }
    Ok(())
}

/// Turns the scalars of a pattern into transitions, longest `**` form first.
fn compile_tokens(
    chars: &[char],
    next_close: &[usize],
    dialect: GlobDialect,
    work: &mut Work,
) -> Result<Vec<Token>, PatternError> {
    let mut tokens = Vec::with_capacity(chars.len());
    let mut index = 0;
    while let Some(&current) = chars.get(index) {
        work.parser_step();
        if starts_with(chars, index, &['/', '*', '*', '/']) {
            tokens.push(Token::SlashDoubleStarSlash);
            index = index.saturating_add(4);
        } else if starts_with(chars, index, &['/', '*', '*']) {
            tokens.push(Token::SlashDoubleStar);
            index = index.saturating_add(3);
        } else if starts_with(chars, index, &['*', '*', '/']) {
            tokens.push(Token::DoubleStarSlash);
            index = index.saturating_add(3);
        } else if starts_with(chars, index, &['*', '*']) {
            let previous_is_separator =
                index == 0 || chars.get(index.saturating_sub(1)) == Some(&'/');
            let after = index.saturating_add(2);
            let next_is_separator = after == chars.len() || chars.get(after) == Some(&'/');
            if dialect == GlobDialect::Strict && !(previous_is_separator && next_is_separator) {
                let at = byte_offset(chars, index);
                let refusal = Err(refusal(PatternErrorKind::DoubleStarPlacement, at));
                #[cfg(feature = "trace")]
                crate::trace::debug!(error = ?refusal.as_ref().err(), "compile_tokens: returning an error to the caller");
                return refusal;
            }
            tokens.push(Token::DoubleStar);
            index = after;
        } else if current == '*' {
            tokens.push(Token::Star);
            index = index.saturating_add(1);
        } else if current == '?' {
            tokens.push(Token::Question);
            index = index.saturating_add(1);
        } else if current == '[' {
            let (negated, content, close) = class_bounds(chars, next_close, index);
            if close == chars.len() {
                if dialect == GlobDialect::Strict {
                    let at = byte_offset(chars, index);
                    let refusal = Err(refusal(PatternErrorKind::UnclosedClass, at));
                    #[cfg(feature = "trace")]
                    crate::trace::debug!(error = ?refusal.as_ref().err(), "compile_tokens: returning an error to the caller");
                    return refusal;
                }
                tokens.push(Token::Literal('['));
                index = index.saturating_add(1);
            } else {
                let body = chars.get(content..close).ok_or_else(|| {
                    refusal(PatternErrorKind::UnclosedClass, byte_offset(chars, index))
                })?;
                let ranges = compile_class(body, dialect, byte_offset(chars, index), work)?;
                tokens.push(Token::Class { ranges, negated });
                index = close.saturating_add(1);
            }
        } else {
            tokens.push(Token::Literal(current));
            index = index.saturating_add(1);
        }
    }
    Ok(tokens)
}

/// Tests a short token prefix without allocating a suffix or rescanning input.
fn starts_with(chars: &[char], offset: usize, prefix: &[char]) -> bool {
    chars.get(offset..offset.saturating_add(prefix.len())) == Some(prefix)
}

/// Resolves one class opener using the precomputed linear-time close index.
fn class_bounds(chars: &[char], next_close: &[usize], opening: usize) -> (bool, usize, usize) {
    let mut content = opening.saturating_add(1);
    let negated = matches!(chars.get(content), Some('!' | '^'));
    if negated {
        content = content.saturating_add(1);
    }
    let mut closing_search = content;
    if chars.get(closing_search) == Some(&']') {
        closing_search = closing_search.saturating_add(1);
    }
    // The table's terminal entry carries the input's length, so an unclosed
    // class names the end of input rather than a missing entry.
    let close = match next_close.get(closing_search) {
        Some(&close) => close,
        None => chars.len(),
    };
    (negated, content, close)
}

/// Converts a scalar offset to the public UTF-8 byte-offset convention. An
/// offset past the end of the input measures the input's own length.
fn byte_offset(chars: &[char], scalar_offset: usize) -> usize {
    chars
        .iter()
        .take(scalar_offset)
        .map(|scalar| scalar.len_utf8())
        .sum()
}

/// Compiles the members of one class into intervals sorted for binary lookup.
fn compile_class(
    body: &[char],
    dialect: GlobDialect,
    class_offset: usize,
    work: &mut Work,
) -> Result<Vec<ScalarRange>, PatternError> {
    let mut ranges = Vec::with_capacity(body.len());
    let mut index = 0;
    while let Some(&start) = body.get(index) {
        work.parser_step();
        let dash = body.get(index.saturating_add(1));
        let last = body.get(index.saturating_add(2));
        if let (Some(&'-'), Some(&end)) = (dash, last) {
            if start > end && dialect == GlobDialect::Strict {
                let refusal = Err(refusal(PatternErrorKind::DescendingRange, class_offset));
                #[cfg(feature = "trace")]
                crate::trace::debug!(error = ?refusal.as_ref().err(), "compile_class: returning an error to the caller");
                return refusal;
            }
            if start <= end {
                ranges.push(ScalarRange { start, end });
            }
            index = index.saturating_add(3);
        } else {
            ranges.push(ScalarRange { start, end: start });
            index = index.saturating_add(1);
        }
    }
    for _range in &ranges {
        work.parser_step();
    }
    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    merge_overlapping_ranges(&mut ranges, work);
    Ok(ranges)
}

/// Merges overlapping intervals so binary-search membership remains monotone.
fn merge_overlapping_ranges(ranges: &mut Vec<ScalarRange>, work: &mut Work) {
    ranges.dedup_by(|current, kept| {
        work.parser_step();
        let overlaps = current.start <= kept.end;
        if overlaps && current.end > kept.end {
            kept.end = current.end;
        }
        overlaps
    });
}

/// Advances one compiled token over one path using the rolling previous row.
fn step_token(token: &Token, path: &[char], previous: &[u8], next: &mut [u8], work: &mut Work) {
    match *token {
        Token::Literal(literal) => {
            step_single(path, previous, next, work, |scalar, _| scalar == literal);
        }
        Token::Question => step_single(path, previous, next, work, |scalar, _| scalar != '/'),
        Token::Class {
            ref ranges,
            negated,
        } => step_single(path, previous, next, work, |scalar, counter| {
            let member = class_contains(ranges, scalar, counter);
            scalar != '/' && (member != negated)
        }),
        Token::Star => step_star(path, previous, next, work),
        Token::DoubleStar => step_double_star(previous, next, work),
        Token::DoubleStarSlash => step_double_star_slash(path, previous, next, work),
        Token::SlashDoubleStar => step_slash_double_star(path, previous, next, work),
        Token::SlashDoubleStarSlash => step_slash_double_star_slash(path, previous, next, work),
    }
}

/// A token that consumes one scalar: the next offset is reachable when this one
/// was and `accepts` takes the scalar between them.
fn step_single(
    path: &[char],
    previous: &[u8],
    next: &mut [u8],
    work: &mut Work,
    mut accepts: impl FnMut(char, &mut Work) -> bool,
) {
    for ((&reach, &scalar), slot) in previous.iter().zip(path).zip(next.iter_mut().skip(1)) {
        work.transition_step();
        let accepted = accepts(scalar, work);
        if reach != 0 && accepted {
            *slot = 1;
        }
    }
}

/// `*`: an offset is reachable when it was before, or when the offset behind it
/// is reachable now and the scalar between them is not `/`.
fn step_star(path: &[char], previous: &[u8], next: &mut [u8], work: &mut Work) {
    let mut scalars = path.iter();
    let mut extends = false;
    for (&reach, slot) in previous.iter().zip(next.iter_mut()) {
        work.transition_step();
        let value = u8::from(reach != 0 || extends);
        *slot = value;
        let consumed = scalars.next();
        extends = value != 0 && consumed.is_some_and(|&scalar| scalar != '/');
    }
}

/// `**`: every offset at or after the first reachable one.
fn step_double_star(previous: &[u8], next: &mut [u8], work: &mut Work) {
    let mut reachable = false;
    for (&reach, slot) in previous.iter().zip(next.iter_mut()) {
        work.transition_step();
        reachable |= reach != 0;
        *slot = u8::from(reachable);
    }
}

/// `**/`: the offsets after each separator reachable from any earlier offset,
/// plus every offset that was already reachable.
fn step_double_star_slash(path: &[char], previous: &[u8], next: &mut [u8], work: &mut Work) {
    let mut scalars = path.iter();
    let mut behind: Option<(u8, char)> = None;
    let mut reachable = false;
    for (&reach, slot) in previous.iter().zip(next.iter_mut()) {
        work.transition_step();
        if let Some((earlier, '/')) = behind {
            reachable |= earlier != 0;
            *slot = u8::from(reachable);
        }
        *slot |= reach;
        reachable |= reach != 0;
        behind = scalars.next().map(|&scalar| (reach, scalar));
    }
}

/// `/**`: a separator reached from a reachable offset opens every later offset,
/// and every offset that was already reachable stays.
fn step_slash_double_star(path: &[char], previous: &[u8], next: &mut [u8], work: &mut Work) {
    let mut scalars = path.iter();
    let mut behind: Option<(u8, char)> = None;
    let mut can_extend = false;
    for (&reach, slot) in previous.iter().zip(next.iter_mut()) {
        work.transition_step();
        if let Some((earlier, scalar)) = behind {
            can_extend |= earlier != 0 && scalar == '/';
        }
        *slot = u8::from(can_extend || reach != 0);
        behind = scalars.next().map(|&scalar| (reach, scalar));
    }
}

/// `/**/`: an offset is reachable when it follows a separator and an earlier
/// separator was reached from a reachable offset.
fn step_slash_double_star_slash(path: &[char], previous: &[u8], next: &mut [u8], work: &mut Work) {
    let mut started = false;
    for ((&earlier, &scalar), slot) in previous.iter().zip(path).zip(next.iter_mut().skip(1)) {
        work.transition_step();
        let separator = scalar == '/';
        started |= earlier != 0 && separator;
        *slot = u8::from(started && separator);
    }
}

/// Looks up one scalar in pre-sorted, disjointness-independent intervals.
fn class_contains(ranges: &[ScalarRange], value: char, work: &mut Work) -> bool {
    if ranges.len() <= 16 {
        ranges.iter().any(|range| {
            work.transition_step();
            range.start <= value && value <= range.end
        })
    } else {
        ranges
            .binary_search_by(|range| {
                work.transition_step();
                if value < range.start {
                    core::cmp::Ordering::Greater
                } else if value > range.end {
                    core::cmp::Ordering::Less
                } else {
                    core::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }
}

/// Reports whether `path` matches `pattern` using the historical permissive dialect.
///
/// This one-call convenience function retains the old behavior: `?` and
/// classes consume one Unicode scalar, `*` stops at `/`, `**` may cross `/`,
/// unmatched `[` is literal, and `**` may occur within a component. It compiles
/// O(M) pattern storage and allocates O(N) scratch. For repeated matches, use
/// [`GlobPattern`] and reuse [`GlobScratch`].
#[must_use]
pub fn matches(pattern: &str, path: &str) -> bool {
    let Ok(compiled) = GlobPattern::compile_with_dialect(pattern, GlobDialect::Legacy) else {
        return false;
    };
    compiled.is_match(path)
}

#[cfg(test)]
mod tests;
