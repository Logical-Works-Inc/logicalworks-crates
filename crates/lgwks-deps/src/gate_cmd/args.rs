//! The gate's argument parsing: one consuming pass, refusing rather than guessing.

use std::path::PathBuf;

/// What the gate was asked to do, after one consuming pass over its arguments.
pub(super) struct GateArgs {
    /// The repository to gate; the working directory when absent.
    pub(super) target: PathBuf,
    /// Emit one JSON document instead of the human transcript.
    pub(super) json: bool,
    /// The `lgwks-ast` binary.
    pub(super) ast: String,
    /// The architecture diff's base: a revision or a directory.
    pub(super) base: String,
}

/// Why the gate's arguments were refused.
pub(super) struct GateArgError(pub(super) String);

/// Logs `reason` where the gate refused its arguments, then hands it back:
/// a refusal the operator cannot diagnose is a refusal the operator cannot
/// repair, so every one carries the token that caused it.
fn refuse(reason: String) -> Result<GateArgs, GateArgError> {
    lgwks_std::trace::debug!(reason = %reason, "gate: refusing its arguments");
    Err(GateArgError(reason))
}

/// Parses the gate's arguments: at most one positional path, `--json` as a
/// mode that may repeat, `--ast` and `--base` each at most once. A refusal
/// names the token, the way `check`'s refusals do.
pub(super) fn parse_gate_args(args: &[String]) -> Result<GateArgs, GateArgError> {
    let mut target: Option<PathBuf> = None;
    let mut json = false;
    let mut ast: Option<String> = None;
    let mut base: Option<String> = None;
    let mut cursor = args.iter();
    while let Some(argument) = cursor.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                return refuse("help".to_owned());
            }
            "--json" => json = true,
            "--ast" | "--base" => {
                let flag = argument.clone();
                let Some(value) = cursor.next() else {
                    return refuse(format!("{flag} needs a value: {flag} VALUE"));
                };
                if value.starts_with("--") {
                    return refuse(format!(
                        "{flag} was given {value:?}, which is another option, not a value"
                    ));
                }
                let taken = if flag == "--ast" {
                    ast.is_some()
                } else {
                    base.is_some()
                };
                if taken {
                    return refuse(format!(
                        "{flag} was given more than once; the gate runs one tool against one base"
                    ));
                }
                if flag == "--ast" {
                    ast = Some(value.clone());
                } else {
                    base = Some(value.clone());
                }
            }
            flag if flag.starts_with("--") => {
                return refuse(format!("unknown option for `gate`: {flag}"));
            }
            path => {
                if target.is_some() {
                    return refuse(format!(
                        "`gate` audits one repository, and {path:?} is a second path"
                    ));
                }
                target = Some(PathBuf::from(path));
            }
        }
    }
    // Each option left unsaid takes its documented default: the current
    // directory, the installed `lgwks-ast`, and `origin/main` as the base.
    let target = match target {
        Some(target) => target,
        None => PathBuf::from("."),
    };
    let ast = match ast {
        Some(ast) => ast,
        None => "lgwks-ast".to_owned(),
    };
    let base = match base {
        Some(base) => base,
        None => "origin/main".to_owned(),
    };
    Ok(GateArgs {
        target,
        json,
        ast,
        base,
    })
}
