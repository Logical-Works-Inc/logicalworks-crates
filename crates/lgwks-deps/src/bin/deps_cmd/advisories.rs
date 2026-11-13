//! `advisories`: `Cargo.lock` files against a local RustSec advisory database
//! (#799, #942), through the same `lgwks_deps::advisories` entry logical_ci's
//! advisory gate calls.
//!
//! Offline by construction: the database is a directory the operator names, so
//! the verdict binds to the commit and digest printed with it rather than to
//! whatever a network answered that day.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use lgwks_deps::advisories::{
    AdvisoryKind, AdvisoryMatch, AdvisoryReport, DatabaseIdentity, SkippedPin, audit_files,
};
use lgwks_std::json::Value;

/// The exit code of a refusal: bad arguments, or a lock or database unread.
const EXIT_REFUSED: u8 = 2;

/// One JSON string, escaped by the estate's one JSON writer.
fn field(text: &str) -> String {
    Value::String(text.to_owned().into()).to_string()
}

/// A JSON array of strings.
fn list(items: &[String]) -> String {
    let fields: Vec<String> = items.iter().map(|item| field(item)).collect();
    format!("[{}]", fields.join(","))
}

/// A JSON string, or `null` for none.
fn optional(text: Option<&str>) -> String {
    match text {
        Some(text) => field(text),
        None => "null".to_owned(),
    }
}

/// What `advisories` was asked to audit.
struct AdvisoriesArgs {
    /// The lockfiles to audit, in the order given.
    locks: Vec<PathBuf>,
    /// The advisory database directory: the last path.
    database: PathBuf,
    /// True when the report is one JSON object on stdout.
    json: bool,
}

/// What one parse of `advisories`' arguments resolved to.
enum AdvisoriesRequest {
    /// Audit lockfiles.
    Audit(AdvisoriesArgs),
    /// Print the usage block and exit successfully.
    Help,
}

/// Why `advisories`' arguments were refused.
#[derive(Debug)]
enum AdvisoriesArgError {
    /// A `-` option this command does not define.
    UnknownFlag {
        /// The unrecognised token, verbatim.
        flag: String,
    },
    /// Fewer than a lockfile and a database.
    WrongPaths {
        /// How many paths were given.
        given: usize,
    },
}

impl fmt::Display for AdvisoriesArgError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::UnknownFlag { ref flag } => {
                write!(formatter, "unknown option for `advisories`: {flag}")
            }
            Self::WrongPaths { given } => write!(
                formatter,
                "`advisories` takes one or more lockfiles and a database directory, and was given {given} paths"
            ),
        }
    }
}

/// Parses `advisories`' arguments in one consuming pass.
fn parse_advisories_args(args: &[String]) -> Result<AdvisoriesRequest, AdvisoriesArgError> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut json = false;
    for argument in args {
        match argument.as_str() {
            "--help" | "-h" => return Ok(AdvisoriesRequest::Help),
            "--json" => json = true,
            flag if flag.starts_with('-') => {
                let refusal = Err(AdvisoriesArgError::UnknownFlag {
                    flag: flag.to_owned(),
                });
                lgwks_std::trace::debug!(
                    flag,
                    "advisories: an option this command does not define"
                );
                return refusal;
            }
            _ => paths.push(PathBuf::from(argument)),
        }
    }
    let given = paths.len();
    let (Some(database), false) = (paths.pop(), paths.is_empty()) else {
        let refusal = Err(AdvisoriesArgError::WrongPaths { given });
        lgwks_std::trace::debug!(given, "advisories: wrong number of paths");
        return refusal;
    };
    Ok(AdvisoriesRequest::Audit(AdvisoriesArgs {
        locks: paths,
        database,
        json,
    }))
}

/// The label a match carries: `vulnerability`, or its informational kind.
fn kind_label(found: &AdvisoryMatch) -> &str {
    let kind: &AdvisoryKind = found.match_kind();
    match kind.informational_name() {
        Some(name) => name,
        None => "vulnerability",
    }
}

/// The machine rendering: one document, one row per match.
fn render_json(report: &AdvisoryReport) -> String {
    let rows: Vec<String> = report
        .matches()
        .iter()
        .map(|found| {
            format!(
                "{{\"id\":{},\"package\":{},\"title\":{},\"version\":{},\"kind\":{},\"patched\":{},\"os\":{},\"arch\":{},\"lockfile\":{}}}",
                field(found.advisory_id()),
                field(found.package()),
                optional(found.title()),
                field(found.installed_version()),
                field(kind_label(found)),
                list(found.match_patched()),
                list(found.os()),
                list(found.arch()),
                field(found.lockfile()),
            )
        })
        .collect();
    let skipped: Vec<String> = report
        .skipped()
        .iter()
        .map(|pin: &SkippedPin| {
            format!(
                "{{\"name\":{},\"version\":{},\"lockfile\":{},\"source\":{}}}",
                field(pin.name()),
                field(pin.version()),
                field(pin.lockfile()),
                optional(pin.source()),
            )
        })
        .collect();
    let database: &DatabaseIdentity = report.database();
    format!(
        "{{\"db_digest\":{},\"db_commit\":{},\"db_date\":{},\"advisories_loaded\":{},\"lockfiles\":{},\"packages_audited\":{},\"packages_skipped\":{},\"skipped\":[{}],\"matches\":[{}]}}",
        field(database.digest()),
        optional(database.commit()),
        optional(database.date()),
        database.advisories(),
        report.lockfiles(),
        report.packages_audited(),
        report.packages_skipped(),
        skipped.join(","),
        rows.join(","),
    )
}

/// One line per match, then the counts and the database the verdict binds to.
fn render_text(report: &AdvisoryReport, out: &mut impl io::Write) -> io::Result<()> {
    for found in report.matches() {
        let platforms = if found.is_restricted() {
            format!(
                " (only on {} {})",
                found.os().join("/"),
                found.arch().join("/")
            )
        } else {
            String::new()
        };
        writeln!(
            out,
            "{} {} {} {}{platforms}; patched: {}; in {}",
            found.advisory_id(),
            found.package(),
            found.installed_version(),
            kind_label(found),
            found.match_patched().join(" | "),
            found.lockfile(),
        )?;
    }
    let database: &DatabaseIdentity = report.database();
    let checkout = match (database.commit(), database.date()) {
        (Some(commit), Some(date)) => format!("commit {commit} of {date}"),
        _ => "not a git checkout".to_owned(),
    };
    writeln!(
        out,
        "{} packages audited in {} lockfiles ({} not from crates.io) against {} advisories ({}, {checkout}); {} matched",
        report.packages_audited(),
        report.lockfiles(),
        report.packages_skipped(),
        database.advisories(),
        database.digest(),
        report.matches().len(),
    )
}

/// Runs `advisories`. Exit 0 is clean, 1 names a vulnerability, 2 is a refusal:
/// an unreadable database or lock is never reported as a clean audit.
///
/// # Errors
///
/// An [`io::Error`] when `out` or `err` refuses a write.
pub(crate) fn run(
    args: &[String],
    usage: &str,
    out: &mut impl io::Write,
    err: &mut impl io::Write,
) -> io::Result<ExitCode> {
    let parsed = match parse_advisories_args(args) {
        Ok(AdvisoriesRequest::Audit(parsed)) => parsed,
        Ok(AdvisoriesRequest::Help) => {
            write!(out, "{usage}")?;
            return Ok(ExitCode::SUCCESS);
        }
        Err(error) => {
            writeln!(err, "lgwks-deps: {error}\n")?;
            write!(err, "{usage}")?;
            return Ok(ExitCode::from(EXIT_REFUSED));
        }
    };
    let report = match audit_files(&parsed.locks, &parsed.database) {
        Ok(report) => report,
        Err(error) => {
            writeln!(err, "lgwks-deps: {error}")?;
            return Ok(ExitCode::from(EXIT_REFUSED));
        }
    };
    if parsed.json {
        writeln!(out, "{}", render_json(&report))?;
    } else {
        render_text(&report, out)?;
    }
    let vulnerable = report.matches().iter().any(|found| {
        let kind: &AdvisoryKind = found.match_kind();
        kind.is_vulnerability()
    });
    if vulnerable {
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}
