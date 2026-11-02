//! Located refusals: why one plan step did not become work.

// ── Refusals ─────────────────────────────────────────────────────────────────

/// Why one plan step did not become work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefusalKind {
    /// The plan names an operation the surface never registered.
    UnknownOperation,
    /// The operation needs a capability this run does not hold.
    MissingCapability,
    /// The plan is past one of the ceilings.
    BoundExceeded,
    /// The body is not a plan at all.
    Malformed,
    /// The plan store itself is unavailable.
    StoreUnavailable,
}

impl RefusalKind {
    /// The name this refusal carries in a verdict, for a log or a counter.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match *self {
            Self::UnknownOperation => "unknown-operation",
            Self::MissingCapability => "missing-capability",
            Self::BoundExceeded => "bound-exceeded",
            Self::Malformed => "malformed",
            Self::StoreUnavailable => "store-unavailable",
        }
    }
}

/// One refusal, located where the plan earned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRefusal {
    /// `line:col` for text, a JSON pointer for JSON, `1:1` for the body.
    location: String,
    /// The ordering key: text sorts by line and column, JSON by pointer.
    sort: String,
    /// Which arm fired.
    kind: RefusalKind,
    /// What it names.
    message: String,
    /// Whether this refusal is the body ceiling, answered `413`.
    over_body: bool,
}

impl PlanRefusal {
    /// One refusal at `location` (with `sort` ordering it) of `kind`.
    pub(crate) fn at(location: String, sort: String, kind: RefusalKind, message: String) -> Self {
        Self {
            location,
            sort,
            kind,
            message,
            over_body: false,
        }
    }

    /// One refusal for a body past its ceiling.
    pub(crate) fn over_body(got: usize, limit: usize) -> Self {
        Self {
            location: String::from("1:1"),
            sort: order(1, 1),
            kind: RefusalKind::BoundExceeded,
            message: format!("the plan body holds {got} bytes, past the ceiling of {limit}"),
            over_body: true,
        }
    }

    /// One refusal for an unavailable store.
    pub(crate) fn unavailable() -> Self {
        Self {
            location: String::from("1:1"),
            sort: order(1, 1),
            kind: RefusalKind::StoreUnavailable,
            message: String::from("the plan store is unavailable"),
            over_body: false,
        }
    }

    /// Where the plan earned this refusal.
    #[must_use]
    pub fn location(&self) -> &str {
        self.location.as_str()
    }

    /// Which arm fired.
    #[must_use]
    pub const fn kind(&self) -> RefusalKind {
        self.kind
    }

    /// What it names.
    #[must_use]
    pub fn message(&self) -> &str {
        self.message.as_str()
    }

    /// Whether this refusal is the body ceiling, answered `413`.
    pub(crate) const fn is_over_body(&self) -> bool {
        self.over_body
    }

    /// The ordering key: location, kind, then message.
    pub(crate) fn sort_key(&self) -> (&str, &str, &str) {
        (self.sort.as_str(), self.kind.label(), self.message.as_str())
    }
}

/// Log `refusal` and hand it back as the error, so every refused plan leaves
/// a record naming its arm and location before the caller sees the value.
pub(crate) fn refuse<T>(refusal: PlanRefusal) -> Result<T, PlanRefusal> {
    lgwks_std::trace::debug!(
        kind = refusal.kind().label(),
        location = refusal.location(),
        message = refusal.message(),
        "apps: the plan is refused"
    );
    Err(refusal)
}

/// A text location.
pub(crate) fn point(line: usize, col: usize) -> String {
    format!("{line}:{col}")
}

/// The ordering key for a text location.
pub(crate) fn order(line: usize, col: usize) -> String {
    format!("t{line:010}:{col:010}")
}
