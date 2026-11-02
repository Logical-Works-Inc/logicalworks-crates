//! `retire` owns schema 3's exit mortgage: what replaces every approved edge,
//! what still blocks the replacement, what the edge costs to carry, and when
//! the mortgage was last reviewed (#788).
//!
//! The approval says what the edge is; this module says how it ends. Each
//! `[[approved]]` block carries one `[approved.retire]` table with a
//! `replacement` module path, a one-sentence `blocker`, a measured `cost`
//! inline table and a `review` date. `check` refuses a register that still
//! reads schema 2, an entry with no table, a replacement no workspace member
//! or `std` owns, an expired review and a cost that is not positive — each
//! with the entry's name and the offending field. `retire --report` renders
//! the same mortgages as text and as one JSON object, with the total distance
//! to zero beside every edge. The reader half lives in [`crate::retire_read`].

use std::path::Path;

use crate::{Refusal, contract::Contract};

// ── Mortgage ────────────────────────────────────────────────────────────────

/// The measured cost of carrying one approved edge, in the units the estate
/// budgets: seconds of cold build, crates in the closure, bytes of built
/// artifacts, the date the numbers were taken, and the exact command that
/// took them. Measured, never estimated (#788).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub(crate) struct RetireCost {
    /// Seconds of one cold build with the edge, minus the build without it.
    pub(crate) cold_build_s: f64,
    /// Distinct packages the edge adds to the resolved closure.
    pub(crate) crates: u64,
    /// Built artifact bytes the edge adds, summed over the edge's `.rlib`s.
    pub(crate) binary_bytes: u64,
    /// ISO date the numbers were measured.
    pub(crate) measured: String,
    /// The exact command that measured them, reproducible from a clean tree.
    pub(crate) command: String,
}

impl RetireCost {
    /// Seconds of one cold build with the edge, minus the build without it.
    #[must_use]
    pub(crate) fn cold_build_s(&self) -> f64 {
        self.cold_build_s
    }

    /// Distinct packages the edge adds to the resolved closure.
    #[must_use]
    pub(crate) fn crates(&self) -> u64 {
        self.crates
    }

    /// Built artifact bytes the edge adds, summed over the edge's `.rlib`s.
    #[must_use]
    pub(crate) fn binary_bytes(&self) -> u64 {
        self.binary_bytes
    }

    /// ISO date the numbers were measured.
    #[must_use]
    pub(crate) fn measured(&self) -> &str {
        &self.measured
    }

    /// The exact command that measured them, reproducible from a clean tree.
    #[must_use]
    pub(crate) fn command(&self) -> &str {
        &self.command
    }
}

/// How one approved edge ends: the module path that will own the capability,
/// the one sentence naming what is missing today, the measured cost of
/// carrying it, and the date the mortgage was last reviewed.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub(crate) struct Retire {
    /// The `std` or workspace module path that will own this capability.
    pub(crate) replacement: String,
    /// One sentence naming what is missing today.
    pub(crate) blocker: String,
    /// The measured cost of carrying the edge.
    pub(crate) cost: RetireCost,
    /// ISO date the mortgage was last reviewed.
    pub(crate) review: String,
}

impl Retire {
    /// The `std` or workspace module path that will own this capability.
    #[must_use]
    pub(crate) fn replacement(&self) -> &str {
        &self.replacement
    }

    /// One sentence naming what is missing today.
    #[must_use]
    pub(crate) fn blocker(&self) -> &str {
        &self.blocker
    }

    /// The measured cost of carrying the edge.
    #[must_use]
    pub(crate) fn cost(&self) -> &RetireCost {
        &self.cost
    }

    /// ISO date the mortgage was last reviewed.
    #[must_use]
    pub(crate) fn review(&self) -> &str {
        &self.review
    }
}

// ── Report ──────────────────────────────────────────────────────────────────

/// Why a retire report could not be rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReportError {
    /// String rendering failed.
    Render {
        /// What the formatter reported.
        cause: String,
    },
}

impl std::fmt::Display for ReportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Render { ref cause } => write!(
                formatter,
                "the retire report could not be rendered: {cause}"
            ),
        }
    }
}

impl std::error::Error for ReportError {}

/// One rendered line of the report.
fn line(out: &mut String, arguments: std::fmt::Arguments<'_>) -> Result<(), ReportError> {
    use std::fmt::Write as _;
    writeln!(out, "{arguments}").map_err(|cause| ReportError::Render {
        cause: cause.to_string(),
    })
}

/// Appends rendered arguments to the report without a trailing newline.
fn push(out: &mut String, arguments: std::fmt::Arguments<'_>) -> Result<(), ReportError> {
    use std::fmt::Write as _;
    out.write_fmt(arguments)
        .map_err(|cause| ReportError::Render {
            cause: cause.to_string(),
        })
}

/// Quotes `value` as one JSON string through the estate facade.
fn quoted(value: &str) -> Result<String, ReportError> {
    lgwks_std::json::to_string(&value).map_err(|cause| ReportError::Render {
        cause: cause.to_string(),
    })
}

/// Renders an optional string field: the quoted value, or null.
fn field(value: Option<&str>) -> Result<String, ReportError> {
    match value {
        Some(text) => quoted(text),
        None => Ok("null".to_owned()),
    }
}

/// What the totals are: each edge was measured alone, so a crate two edges
/// share is counted once per edge and the sums bound the saving from above.
const TOTALS_BASIS: &str = "sum of per-edge measurements; a crate shared by two edges counts once per edge, so crates and binary_bytes are an upper bound on what removing every edge saves";

/// The summed distance to zero over every mortgaged edge.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Totals {
    /// Mortgaged edges counted.
    pub(crate) edges: usize,
    /// Summed cold-build seconds.
    pub(crate) cold_build_s: f64,
    /// Summed closure crates.
    pub(crate) crates: u64,
    /// Summed artifact bytes.
    pub(crate) binary_bytes: u64,
}

/// Sums the measured cost over every entry that mortgages one.
pub(crate) fn totals(register: &Contract) -> Totals {
    let mut summed = Totals::default();
    for entry in register.approvals() {
        let Some(mortgage) = entry.retire() else {
            continue;
        };
        summed.edges = summed.edges.saturating_add(1);
        summed.cold_build_s += mortgage.cost().cold_build_s();
        summed.crates = summed.crates.saturating_add(mortgage.cost().crates());
        summed.binary_bytes = summed
            .binary_bytes
            .saturating_add(mortgage.cost().binary_bytes());
    }
    summed
}

/// Renders the report as human text: one line per edge with its replacement,
/// its measured cost and its review date, then the summed distance to zero.
pub fn render_text(register: &Contract) -> Result<String, ReportError> {
    let mut out = String::new();
    line(
        &mut out,
        format_args!("retire edges {}", register.entry_count()),
    )?;
    for entry in register.approvals() {
        let mortgage = entry.retire();
        let (cold_build_s, crates, binary_bytes) = match mortgage {
            Some(row) => (
                row.cost().cold_build_s(),
                row.cost().crates(),
                row.cost().binary_bytes(),
            ),
            None => (0.0, 0, 0),
        };
        line(
            &mut out,
            format_args!(
                "edge {} owner {} replacement {} blocker {} cold_build_s {:.2} crates {} binary_bytes {} measured {} review {}",
                entry.krate(),
                entry.owner(),
                field(mortgage.map(Retire::replacement))?,
                field(mortgage.map(Retire::blocker))?,
                cold_build_s,
                crates,
                binary_bytes,
                field(mortgage.map(|row| row.cost().measured()))?,
                field(mortgage.map(Retire::review))?,
            ),
        )?;
    }
    let summed = totals(register);
    line(
        &mut out,
        format_args!(
            "totals edges {} cold_build_s {:.2} crates {} binary_bytes {}",
            summed.edges, summed.cold_build_s, summed.crates, summed.binary_bytes,
        ),
    )?;
    line(&mut out, format_args!("basis {TOTALS_BASIS}"))?;
    Ok(out)
}

/// Renders the report as one JSON object under stable keys.
pub fn render_json(register: &Contract, today: &str) -> Result<String, ReportError> {
    let mut out = String::from("{");
    push(&mut out, format_args!("\"today\":{}", quoted(today)?))?;
    out.push_str(",\"edges\":[");
    let mut first = true;
    for entry in register.approvals() {
        if !first {
            out.push(',');
        }
        first = false;
        let mortgage = entry.retire();
        let cost = match mortgage.map(Retire::cost) {
            Some(cost) => format!(
                "{{\"cold_build_s\":{:.2},\"crates\":{},\"binary_bytes\":{},\"measured\":{},\"command\":{}}}",
                cost.cold_build_s(),
                cost.crates(),
                cost.binary_bytes(),
                quoted(cost.measured())?,
                quoted(cost.command())?,
            ),
            None => "null".to_owned(),
        };
        push(
            &mut out,
            format_args!(
                "{{\"crate\":{},\"owner\":{},\"capability\":{},\"replacement\":{},\"blocker\":{},\"cost\":{cost},\"review\":{}}}",
                quoted(entry.krate())?,
                quoted(entry.owner())?,
                quoted(&entry.capability)?,
                field(mortgage.map(Retire::replacement))?,
                field(mortgage.map(Retire::blocker))?,
                field(mortgage.map(Retire::review))?,
            ),
        )?;
    }
    let summed = totals(register);
    push(
        &mut out,
        format_args!(
            "],\"totals\":{{\"edges\":{},\"cold_build_s\":{:.2},\"crates\":{},\"binary_bytes\":{},\"basis\":{}}}",
            summed.edges,
            summed.cold_build_s,
            summed.crates,
            summed.binary_bytes,
            quoted(TOTALS_BASIS)?,
        ),
    )?;
    out.push('}');
    Ok(out)
}

// ── Audit ───────────────────────────────────────────────────────────────────

/// `std`'s top-level modules a replacement may name after the `std::` head.
///
/// The estate replaces capabilities module by module, so the check names the
/// first segment rather than the whole tree: `std::vec` and `std::sync` are
/// both answers this list admits, and anything outside it is a path no `std`
/// the gate knows owns.
const STD_TOP: [&str; 43] = [
    "array",
    "ascii",
    "borrow",
    "boxed",
    "cell",
    "char",
    "clone",
    "cmp",
    "collections",
    "convert",
    "env",
    "error",
    "ffi",
    "fmt",
    "fs",
    "future",
    "hash",
    "hint",
    "io",
    "iter",
    "marker",
    "mem",
    "net",
    "num",
    "ops",
    "option",
    "os",
    "panic",
    "path",
    "pin",
    "primitive",
    "process",
    "ptr",
    "rc",
    "result",
    "slice",
    "str",
    "string",
    "sync",
    "task",
    "thread",
    "time",
    "vec",
];

/// Whether `replacement` names a module path the estate could own.
///
/// Shape only: at least two `::` segments, every one an identifier. Which
/// crate owns the head is the audit's question, not the parser's.
pub(crate) fn is_replacement_shape(value: &str) -> bool {
    let parts: Vec<&str> = value.split("::").collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|part| crate::contract::is_identifier(part))
}

/// Whether `replacement` names a module its crate owns, at `root`.
///
/// The head must be `std`, `core`, `alloc` or one of `members`; a workspace
/// head must resolve to `crates/<head>/src/<rest>.rs` or `<rest>/mod.rs`
/// beside `root`, and a `std` head must name a top-level module this gate
/// knows. Anything else is a path nothing owns.
fn replacement_resolves(replacement: &str, members: &[String], root: &Path) -> bool {
    let parts: Vec<&str> = replacement.split("::").collect();
    if parts.len() < 2 {
        return false;
    }
    let head = parts[0];
    let rest = &parts[1..];
    if head == "core" || head == "alloc" {
        return true;
    }
    if head == "std" {
        let Some(first) = rest.first() else {
            return false;
        };
        return STD_TOP.contains(first);
    }
    if !members.iter().any(|member| member == head) {
        return false;
    }
    let base = root.join("crates").join(head.replace('_', "-")).join("src");
    let mut file = base.clone();
    for part in rest {
        file = file.join(part);
    }
    file.set_extension("rs");
    if file.is_file() {
        return true;
    }
    let mut dir = base;
    for part in rest {
        dir = dir.join(part);
    }
    dir.join("mod.rs").is_file()
}

/// Every retire refusal for `register`, in deterministic order.
///
/// `today` is an ISO date the caller supplies: production passes the clock's
/// date, tests pass a fixed one, so an expiry verdict never depends on the
/// machine the test runs on. A register below schema 3 is one policy refusal
/// naming the migration, not one refusal per entry it never mortgaged.
#[must_use]
pub fn refusals(register: &Contract, members: &[String], root: &Path, today: &str) -> Vec<Refusal> {
    if register.schema() < 3 {
        return vec![Refusal::SchemaMigration {
            schema: register.schema(),
        }];
    }
    let mut found: Vec<Refusal> = Vec::new();
    for entry in register.approvals() {
        let Some(mortgage) = entry.retire() else {
            found.push(Refusal::RetireMissing {
                krate: entry.krate().to_owned(),
                field: "retire".to_owned(),
            });
            continue;
        };
        if !replacement_resolves(mortgage.replacement(), members, root) {
            found.push(Refusal::RetireInvalid {
                krate: entry.krate().to_owned(),
                field: "retire.replacement".to_owned(),
                value: mortgage.replacement().to_owned(),
                expected: "expected std, core, alloc or a workspace member owning that module path"
                    .to_owned(),
            });
        }
        if mortgage.review().is_empty() || mortgage.review() < today {
            found.push(Refusal::RetireExpired {
                krate: entry.krate().to_owned(),
                field: "retire.review".to_owned(),
                review: mortgage.review().to_owned(),
                today: today.to_owned(),
            });
        }
        let cost = mortgage.cost();
        for (name, positive) in [
            ("retire.cost.cold_build_s", cost.cold_build_s() > 0.0),
            ("retire.cost.crates", cost.crates() > 0),
            ("retire.cost.binary_bytes", cost.binary_bytes() > 0),
        ] {
            if !positive {
                found.push(Refusal::RetireInvalid {
                    krate: entry.krate().to_owned(),
                    field: name.to_owned(),
                    value: "0".to_owned(),
                    expected: "expected a measured positive number, never an estimate or zero"
                        .to_owned(),
                });
            }
        }
    }
    found.sort_by_key(|refusal| refusal.to_string());
    found
}
