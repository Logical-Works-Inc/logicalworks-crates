//! `lgwks-deps retire --report`: the exit-mortgage report (#788).
//!
//! `check` refuses; this command explains. It renders every approval's
//! `[approved.retire]` mortgage — replacement, blocker, measured cost and
//! review — beside the summed distance to zero, as human text and, with
//! `--json`, as one JSON object under stable keys.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use lgwks_deps::{contract::Contract, repository_root, retire};

use crate::{refuse, subject_path};

/// What `retire` was asked to do, after one consuming pass over its arguments.
enum RetireRequest {
    /// Render the mortgage report.
    Report(RetireArgs),
    /// Print the usage block and exit successfully.
    Help,
}

/// The `retire --report` invocation, in parsed form.
struct RetireArgs {
    /// Repository to report on. `None` means the process working directory.
    target: Option<PathBuf>,
    /// True when the report renders as one JSON object on stdout.
    json: bool,
}

/// Why `retire`'s arguments were refused.
#[derive(Debug)]
enum RetireArgError {
    /// `--report` was not given: the command reports and nothing else.
    MissingReport,
    /// A `--flag` this command does not define.
    UnknownFlag {
        /// The unrecognised token, verbatim.
        flag: String,
    },
    /// A second positional argument.
    SurplusTarget {
        /// The extra token, verbatim.
        value: String,
    },
}

impl fmt::Display for RetireArgError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::MissingReport => {
                write!(
                    formatter,
                    "`retire` renders a report: `retire --report [PATH] [--json]`"
                )
            }
            Self::UnknownFlag { ref flag } => {
                write!(formatter, "unknown option for `retire`: {flag}")
            }
            Self::SurplusTarget { ref value } => {
                write!(
                    formatter,
                    "`retire` reports one repository, and {value:?} is a second path"
                )
            }
        }
    }
}

/// Parses `retire`'s arguments in one consuming pass.
fn parse_retire_args(args: &[String]) -> Result<RetireRequest, RetireArgError> {
    let mut target: Option<PathBuf> = None;
    let mut report = false;
    let mut json = false;
    for argument in args {
        match argument.as_str() {
            "--help" | "-h" => return Ok(RetireRequest::Help),
            "--report" => report = true,
            "--json" => json = true,
            flag if flag.starts_with("--") => {
                let refusal = Err(RetireArgError::UnknownFlag {
                    flag: flag.to_owned(),
                });
                lgwks_std::trace::debug!(flag, "retire: an option this command does not define");
                return refusal;
            }
            path => {
                if target.is_some() {
                    let refusal = Err(RetireArgError::SurplusTarget {
                        value: path.to_owned(),
                    });
                    lgwks_std::trace::debug!(path, "retire: a second repository path");
                    return refusal;
                }
                target = Some(PathBuf::from(path));
            }
        }
    }
    if !report {
        let refusal = Err(RetireArgError::MissingReport);
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "retire: called without --report");
        return refusal;
    }
    Ok(RetireRequest::Report(RetireArgs { target, json }))
}

/// Renders one repository's exit mortgages and reports success.
fn run_report(
    args: RetireArgs,
    out: &mut impl io::Write,
    err: &mut impl io::Write,
) -> io::Result<ExitCode> {
    let start = subject_path(args.target.as_deref());
    let root = match repository_root(&start) {
        Ok(root) => root,
        Err(error) => return refuse(&error.to_string(), err),
    };
    let register_path = root.join(lgwks_deps::CONTRACT_PATH);
    let text = match std::fs::read_to_string(&register_path) {
        Ok(text) => text,
        Err(error) => {
            return refuse(
                &format!("cannot read {}: {error}", register_path.display()),
                err,
            );
        }
    };
    let register = match Contract::parse(&text) {
        Ok(register) => register,
        Err(error) => {
            return refuse(&format!("{}: {error}", register_path.display()), err);
        }
    };
    let today = lgwks_deps::patches::today_utc();
    let rendered = if args.json {
        retire::render_json(&register, today.as_str())
    } else {
        retire::render_text(&register)
    };
    match rendered {
        Ok(report) => {
            writeln!(out, "{report}")?;
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => refuse(&error.to_string(), err),
    }
}

/// Runs `retire`, keeping the argument parsing out of the report path.
pub(crate) fn handle_retire(
    args: &[String],
    out: &mut impl io::Write,
    err: &mut impl io::Write,
) -> io::Result<ExitCode> {
    let request = match parse_retire_args(args) {
        Ok(request) => request,
        Err(error) => {
            writeln!(err, "lgwks-deps: {error}\n")?;
            write!(err, "{}", crate::USAGE)?;
            return Ok(ExitCode::from(2));
        }
    };
    match request {
        RetireRequest::Help => {
            write!(out, "{}", crate::USAGE)?;
            Ok(ExitCode::SUCCESS)
        }
        RetireRequest::Report(parsed) => run_report(parsed, out, err),
    }
}
