//! `check`'s argument parsing: one consuming pass, refusing rather than guessing.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::{USAGE, handle_help, run_check};

/// What `check` was asked to do, after one consuming pass over its arguments.
///
/// The two path fields are separate and separately typed. An earlier revision
/// read every token that did not start with `--` as positional, which made the
/// *value* of `--contract` a candidate audit target as well as the register:
/// `check --contract other/contract/APPROVED.toml` then discovered its
/// repository root inside `other/` and reported a verdict for a tree nobody
/// asked about. The register and the subject are different things and are now
/// different fields, so no parse can conflate them.
struct CheckArgs {
    /// Repository to audit. `None` means the process working directory, which
    /// is the documented default for an omitted `PATH`.
    target: Option<PathBuf>,
    /// Register to read instead of the target's own `contract/APPROVED.toml`.
    /// Diagnosis only: a build always reads the register beside the code.
    contract: Option<PathBuf>,
    /// True when the verdict is rendered as one JSON object on stdout.
    json: bool,
}

/// What one parse of `check`'s arguments resolved to.
///
/// An enum rather than a `help` flag on [`CheckArgs`]: a request to print the
/// usage block has no audit target, and a bool would leave every consumer
/// deciding what an empty target means when the flag is set.
enum CheckRequest {
    /// Audit a repository.
    Audit(CheckArgs),
    /// Print the usage block and exit successfully.
    Help,
}

/// Why `check`'s arguments were refused.
///
/// Refusals rather than guesses. Every variant names the token that caused it,
/// because the operator's next action is to look at that token, and every one
/// of them used to be silently tolerated — which is how an option's value
/// became the audited repository.
#[derive(Debug)]
enum CheckArgError {
    /// An option that takes a value was the last argument.
    MissingValue {
        /// The option that needed a value.
        flag: &'static str,
    },
    /// An option that takes a value was handed another option.
    ///
    /// `--contract --json` is a missing value, not a path named `--json`:
    /// treating it as a path would silently redirect the register to a
    /// filename that does not exist and turn a diagnosis into a refusal about
    /// the wrong file. No usable path begins with `--`.
    ValueLooksLikeOption {
        /// The option that needed a value.
        flag: &'static str,
        /// The token it was handed instead.
        value: String,
    },
    /// A value-taking option appeared twice.
    DuplicateOverride {
        /// The option that was repeated.
        flag: &'static str,
    },
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

impl fmt::Display for CheckArgError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::MissingValue { flag } => {
                write!(formatter, "{flag} needs a value: {flag} FILE")
            }
            Self::ValueLooksLikeOption { flag, ref value } => write!(
                formatter,
                "{flag} was given {value:?}, which is another option, not a path"
            ),
            Self::DuplicateOverride { flag } => write!(
                formatter,
                "{flag} was given more than once; `check` reads one register"
            ),
            Self::UnknownFlag { ref flag } => {
                write!(formatter, "unknown option for `check`: {flag}")
            }
            Self::SurplusTarget { ref value } => write!(
                formatter,
                "`check` audits one repository, and {value:?} is a second path"
            ),
        }
    }
}

/// Parses `check`'s arguments in one consuming pass.
///
/// The pass consumes each option's value, so a value can never also be read as
/// the positional target. `PATH` may appear before or after the options and
/// means the same thing either way; omitting it leaves [`CheckArgs::target`]
/// as `None`, which [`run_check`] resolves to the working directory.
///
/// `--json` may repeat: it is a mode, and repeating a mode cannot change what
/// the command does. `--contract` may not: two registers are two policies, and
/// picking either one silently would make the verdict depend on argument order.
/// The `--contract=FILE` spelling is not accepted — it is reported as an
/// unknown option rather than parsed into a path, so a caller that guessed the
/// wrong spelling is told instead of audited against the wrong file.
fn parse_check_args(args: &[String]) -> Result<CheckRequest, CheckArgError> {
    let mut target: Option<PathBuf> = None;
    let mut contract: Option<PathBuf> = None;
    let mut json = false;
    let mut cursor = args.iter();
    while let Some(argument) = cursor.next() {
        match argument.as_str() {
            "--help" | "-h" => return Ok(CheckRequest::Help),
            "--json" => json = true,
            "--contract" => {
                if contract.is_some() {
                    let refusal = Err(CheckArgError::DuplicateOverride { flag: "--contract" });
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "parse_check_args: returning an error to the caller");
                    return refusal;
                }
                let Some(value) = cursor.next() else {
                    let refusal = Err(CheckArgError::MissingValue { flag: "--contract" });
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "parse_check_args: returning an error to the caller");
                    return refusal;
                };
                if value.starts_with("--") {
                    let refusal = Err(CheckArgError::ValueLooksLikeOption {
                        flag: "--contract",
                        value: value.clone(),
                    });
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "parse_check_args: returning an error to the caller");
                    return refusal;
                }
                contract = Some(PathBuf::from(value));
            }
            flag if flag.starts_with("--") => {
                let refusal = Err(CheckArgError::UnknownFlag {
                    flag: flag.to_owned(),
                });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "parse_check_args: returning an error to the caller");
                return refusal;
            }
            // A single `-` token is a path, not an option: the only short flag
            // this command defines is `-h`, which is matched above.
            path => {
                if target.is_some() {
                    let refusal = Err(CheckArgError::SurplusTarget {
                        value: path.to_owned(),
                    });
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "parse_check_args: returning an error to the caller");
                    return refusal;
                }
                target = Some(PathBuf::from(path));
            }
        }
    }
    Ok(CheckRequest::Audit(CheckArgs {
        target,
        contract,
        json,
    }))
}

/// Runs `check`, keeping the argument parsing out of the audit path.
///
/// A refused invocation prints the reason and the usage block and exits 2
/// without auditing anything: the one thing it must never do is fall back to
/// auditing something that parses.
pub(crate) fn handle_check(
    args: &[String],
    out: &mut impl io::Write,
    err: &mut impl io::Write,
) -> io::Result<ExitCode> {
    let request = match parse_check_args(args) {
        Ok(request) => request,
        Err(error) => {
            writeln!(err, "lgwks-deps: {error}\n")?;
            write!(err, "{USAGE}")?;
            return Ok(ExitCode::from(2));
        }
    };
    match request {
        CheckRequest::Help => handle_help(out),
        CheckRequest::Audit(parsed) => {
            run_check(parsed.target, parsed.contract, parsed.json, out, err)
        }
    }
}
