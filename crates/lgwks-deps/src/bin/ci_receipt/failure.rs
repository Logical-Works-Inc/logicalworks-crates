//! The failure a receipt holds, and the bounded, ordered set of them.
//!
//! A receipt is the first `max` failures in one total order (job, file, line,
//! test, then the rest), so the same logs give the same receipt whatever order
//! the files were read in, and a log's failures can be added one log at a time
//! without ever holding more than `max` plus one log's worth.

use std::collections::BTreeSet;

/// The most failures one reader takes from one log. More are dropped and the
/// log is shed.
pub(crate) const MAX_HITS: usize = 2_048;

/// The keys one reader of one log kept: each key once, at most [`MAX_HITS`].
#[derive(Debug)]
pub(crate) struct Kept<K> {
    /// The keys taken.
    seen: BTreeSet<K>,
    /// Whether a new key came past the bound and was dropped.
    pub(crate) shed: bool,
}

impl<K> Default for Kept<K> {
    fn default() -> Self {
        Self {
            seen: BTreeSet::new(),
            shed: false,
        }
    }
}

impl<K: Ord> Kept<K> {
    /// True when `key` is new and fewer than [`MAX_HITS`] were taken; a new key
    /// past the bound sets `shed` and warns, naming the `reader` that dropped it.
    pub(crate) fn admit(&mut self, key: K, reader: &'static str) -> bool {
        if self.seen.contains(&key) {
            return false;
        }
        if self.seen.len() >= MAX_HITS {
            self.shed = true;
            lgwks_std::trace::warn!(
                limit = MAX_HITS,
                reader,
                "a failure was dropped past the per-log bound"
            );
            return false;
        }
        self.seen.insert(key)
    }
}

/// The most failures `--max` may ask for. A PR comment holds 65,536 characters,
/// and a table row is at most about 650 of them.
pub(crate) const MAX_CEILING: usize = 100;

/// The failures a receipt keeps when `--max` is not given.
pub(crate) const DEFAULT_MAX: usize = 50;

/// The most characters a `message` keeps.
pub(crate) const MESSAGE_CHARS: usize = 400;

/// The most characters a `step` keeps.
pub(crate) const STEP_CHARS: usize = 200;

/// The most characters a `repro` keeps.
pub(crate) const REPRO_CHARS: usize = 1_000;

/// What sort of failure a log held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    /// A test nextest reported as failed.
    Test,
    /// A diagnostic rustc or clippy printed with a source location.
    Compile,
    /// A step that failed with nothing more specific found in its log.
    Step,
}

impl Kind {
    /// The word the receipt writes for this kind.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Compile => "compile",
            Self::Step => "step",
        }
    }
}

/// One failure, with the command that runs it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    /// The log's file stem.
    pub(crate) job: String,
    /// What sort of failure it is.
    pub(crate) kind: Kind,
    /// The test's name, for a test.
    pub(crate) test: Option<String>,
    /// The nextest binary id, for a test.
    pub(crate) binary: Option<String>,
    /// The file the failure names.
    pub(crate) file: Option<String>,
    /// The line of that file.
    pub(crate) line: Option<u32>,
    /// A rustc error code or a lint name.
    pub(crate) code: Option<String>,
    /// The seed the test printed, in digits.
    pub(crate) seed: Option<String>,
    /// What the failure said, at most [`MESSAGE_CHARS`] characters.
    pub(crate) message: String,
    /// The command that runs it again; empty when no safe one can be written.
    pub(crate) repro: String,
    /// The failing step's command, at most [`STEP_CHARS`] characters.
    pub(crate) step: String,
}

/// The sort key of a failure: job, file, line, test, then enough more to make
/// the order total.
type Order<'failure> = (
    &'failure str,
    Option<&'failure str>,
    Option<u32>,
    Option<&'failure str>,
    Kind,
    Option<&'failure str>,
    &'failure str,
);

impl Failure {
    /// This failure's place in the receipt.
    fn order(&self) -> Order<'_> {
        (
            &self.job,
            self.file.as_deref(),
            self.line,
            self.test.as_deref(),
            self.kind,
            self.code.as_deref(),
            &self.message,
        )
    }
}

/// The receipt: the first `max` failures, and what did not fit.
#[derive(Debug)]
pub(crate) struct Receipt {
    /// The most failures to keep.
    max: usize,
    /// The failures kept, in order, without repeats.
    pub(crate) failures: Vec<Failure>,
    /// How many failures came after the first `max` and were dropped.
    pub(crate) cut: usize,
    /// Whether some log text lay past a read bound and was not read.
    pub(crate) shed: bool,
}

impl Receipt {
    /// An empty receipt that keeps at most `max` failures.
    pub(crate) const fn new(max: usize) -> Self {
        Self {
            max,
            failures: Vec::new(),
            cut: 0,
            shed: false,
        }
    }

    /// Adds `more`, keeping the first `max` of everything seen. A failure that is
    /// dropped now sorts after `max` others, and more failures only push it
    /// further, so it can never come back: each is counted once.
    pub(crate) fn absorb(&mut self, more: Vec<Failure>) {
        self.failures.extend(more);
        self.failures
            .sort_by(|left, right| left.order().cmp(&right.order()));
        self.failures.dedup();
        let over = self.failures.len().saturating_sub(self.max);
        self.cut = self.cut.saturating_add(over);
        self.failures.truncate(self.max);
    }

    /// Whether the receipt is missing something the logs held.
    pub(crate) const fn truncated(&self) -> bool {
        self.cut > 0 || self.shed
    }
}
