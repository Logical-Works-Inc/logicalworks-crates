//! `advisories`: a `Cargo.lock` against a local RustSec advisory database (#799).
//!
//! Offline by construction: the database is a directory the operator names, so
//! the verdict binds to the digest printed with it rather than to whatever a
//! network answered that day.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use lgwks_std::json::Value;

use super::{AdvisoryMatch, AdvisoryReport, audit_lockfile};

/// The exit code of a refusal: bad arguments, or a lock or database unread.
const EXIT_REFUSED: u8 = 2;

/// One JSON string, escaped by the estate's one JSON writer.
fn field(text: &str) -> String {
    Value::String(text.to_owned().into()).to_string()
}

/// What `advisories` was asked to audit.
struct AdvisoriesArgs {
    /// The lockfile to audit.
    lock: PathBuf,
    /// The advisory database directory.
    database: PathBuf,
    /// True when the report is one JSON object on stdout.
    json: bool,
}

/// What one parse of `advisories`' arguments resolved to.
enum AdvisoriesRequest {
    /// Audit a lockfile.
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
    /// Not exactly a lockfile and a database.
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
                "`advisories` takes a lockfile and a database directory, and was given {given} paths"
            ),
        }
    }
}

/// Parses `advisories`' arguments in one consuming pass.
fn parse_advisories_args(args: &[String]) -> Result<AdvisoriesRequest, AdvisoriesArgError> {
    let mut paths: Vec<&String> = Vec::new();
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
            _ => paths.push(argument),
        }
    }
    let &[lock, database] = paths.as_slice() else {
        let refusal = Err(AdvisoriesArgError::WrongPaths { given: paths.len() });
        lgwks_std::trace::debug!(given = paths.len(), "advisories: wrong number of paths");
        return refusal;
    };
    Ok(AdvisoriesRequest::Audit(AdvisoriesArgs {
        lock: PathBuf::from(lock.as_str()),
        database: PathBuf::from(database.as_str()),
        json,
    }))
}

/// Whether this match is a vulnerability, which is what fails the audit.
fn is_vulnerable(found: &AdvisoryMatch) -> bool {
    found.match_kind().is_vulnerability()
}

/// The label a match carries: `vulnerability`, or its informational kind.
fn kind_label(found: &AdvisoryMatch) -> &str {
    match found.match_kind().informational_name() {
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
            let patched: Vec<String> = found
                .match_patched()
                .iter()
                .map(|item| field(item))
                .collect();
            format!(
                "{{\"id\":{},\"package\":{},\"version\":{},\"kind\":{},\"patched\":[{}]}}",
                field(found.advisory_id()),
                field(found.package()),
                field(found.installed_version()),
                field(kind_label(found)),
                patched.join(","),
            )
        })
        .collect();
    format!(
        "{{\"db_digest\":{},\"advisories_loaded\":{},\"packages_audited\":{},\"matches\":[{}]}}",
        field(report.db_digest()),
        report.advisories_loaded(),
        report.packages_audited(),
        rows.join(","),
    )
}

/// One line per match, then the counts and the digest the verdict binds to.
fn render_text(report: &AdvisoryReport, out: &mut impl io::Write) -> io::Result<()> {
    for found in report.matches() {
        writeln!(
            out,
            "{} {} {} {}; patched: {}",
            found.advisory_id(),
            found.package(),
            found.installed_version(),
            kind_label(found),
            found.match_patched().join(" | "),
        )?;
    }
    writeln!(
        out,
        "{} packages audited against {} advisories ({}); {} matched",
        report.packages_audited(),
        report.advisories_loaded(),
        report.db_digest(),
        report.matches().len(),
    )
}

/// Runs `advisories`. Exit 0 is clean, 1 names a vulnerability, 2 is a refusal:
/// an unreadable database or lock is never reported as a clean audit.
///
/// # Errors
///
/// An [`io::Error`] when `out` or `err` refuses a write.
#[doc(hidden)]
pub fn run(
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
    let report = match audit_lockfile(&parsed.lock, &parsed.database) {
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
    if report.matches().iter().any(is_vulnerable) {
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}
