//! `explain`: what a COBOL program or a JCL job does, for people and agents.
//!
//! One file in, one summary out: the program or job name, its dialect, and
//! one sentence per fact, each pinned to the source lines that prove it.
//! `--json` prints the same facts with the graphs beside them for an agent.
//! COPY bodies resolve through `--copy-dir DIR`: each COPY name reads
//! `DIR/NAME` then `DIR/NAME.cpy`, and a name no file supplies stays an
//! unresolved COPY the summary says it cannot tell about. Names holding a
//! slash or a parent reference never leave the directory.
//!
//! Every read runs under the legacy ceilings: source bytes ride
//! `--max-bytes`, source lines and datasets ride `--max-records`, copy files
//! ride `--max-pages`, and the node and edge bounds are eight per admitted
//! line. Past a bound is `bound-exceeded`; anything else is `malformed`.

use lgwks_ast::legacy::cobol::parse_program;
use lgwks_ast::legacy::jcl::parse_job;
use lgwks_ast::legacy::json::{job_json, program_json};
use lgwks_ast::legacy::{LegacyError, LegacyLimits};
use lgwks_std::json::Value;

use super::super::common::{self, Refusal};
use super::{Fact, Invocation, human};

/// The per-line fan-out the node and edge bounds admit: at most eight tree
/// nodes and eight graph edges per admitted source line.
const FAN_OUT: usize = 8;

/// Runs `lgwks legacy explain <file> [--copy-dir DIR]`.
pub(super) fn explain(call: &Invocation<'_>) -> Result<Fact, Refusal> {
    call.allow(&["copy-dir"])?;
    let path = call.word(0, "a COBOL or JCL file")?;
    if call.positional.len() > 1 {
        return common::refuse(Refusal::whole(String::from(
            "lgwks legacy explain takes one file",
        )));
    }
    let bytes = call.read(path)?;
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            return common::refuse(Refusal::whole(format!("{path} is not UTF-8 text: {error}")));
        }
    };
    let limits = legacy_limits(call)?;
    match file_kind(path, text.as_str()) {
        Some(true) => explain_cobol(call, path, text.as_str(), &limits),
        Some(false) => explain_jcl(path, text.as_str(), &limits),
        None => common::refuse(Refusal::whole(format!(
            "{path} is neither COBOL nor JCL: name it .cbl or .jcl"
        ))),
    }
}

/// Whether `path` names COBOL (`true`) or JCL (`false`): the extension first,
/// the content when the extension names neither.
fn file_kind(path: &str, text: &str) -> Option<bool> {
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());
    match extension.as_deref() {
        Some("cbl" | "cob" | "cpy" | "cobol") => Some(true),
        Some("jcl" | "job" | "proc") => Some(false),
        Some(_) | None => sniff_kind(text),
    }
}

/// Whether `text` reads as COBOL (`true`) or JCL (`false`), or neither.
fn sniff_kind(text: &str) -> Option<bool> {
    let upper = text.to_uppercase();
    if upper.contains("IDENTIFICATION DIVISION") {
        return Some(true);
    }
    let trimmed = text.trim_start();
    if trimmed.starts_with("//") || text.contains("\n//") {
        return Some(false);
    }
    None
}

/// The legacy bounds for this invocation: source bytes ride `--max-bytes`,
/// lines and datasets ride `--max-records`, copy files ride `--max-pages`,
/// and nodes and edges admit eight per line.
fn legacy_limits(call: &Invocation<'_>) -> Result<LegacyLimits, Refusal> {
    let lines = call.number("max-records", super::DEFAULT_MAX_RECORDS)?;
    let pages = call.number("max-pages", super::DEFAULT_MAX_PAGES)?;
    let fan = lines.saturating_mul(FAN_OUT);
    Ok(LegacyLimits::new(
        call.max_bytes,
        lines,
        fan,
        fan,
        pages,
        super::MAX_NESTING,
        lines,
    ))
}

/// The refusal a legacy parse error earns: past a ceiling is a bound the
/// caller can raise, anything else is the input's own shape at its line.
fn explain_refusal(error: &LegacyError) -> Refusal {
    match *error {
        LegacyError::SourceTooLarge { .. }
        | LegacyError::TooManyLines { .. }
        | LegacyError::TooManyNodes { .. }
        | LegacyError::TooManyEdges { .. }
        | LegacyError::CopyTooDeep { .. }
        | LegacyError::TooManyCopies { .. }
        | LegacyError::TooManyDatasets { .. } => Refusal::bound(format!(
            "{error}; raise it with --max-bytes, --max-records or --max-pages"
        )),
        _ => Refusal::whole(error.to_string()),
    }
}

/// Explains one COBOL program: its summary for a person, its JSON for agents.
fn explain_cobol(
    call: &Invocation<'_>,
    path: &str,
    text: &str,
    limits: &LegacyLimits,
) -> Result<Fact, Refusal> {
    let copy_dir = call.option("copy-dir").map(String::from);
    // A copybook file that exists but cannot be read is a refusal, kept here
    // because the resolver answers only Some(body) or None.
    let failure: std::cell::RefCell<Option<Refusal>> = std::cell::RefCell::new(None);
    let resolve = |name: &str| -> Option<String> {
        let dir = copy_dir.clone()?;
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return None;
        }
        for candidate in [format!("{dir}/{name}"), format!("{dir}/{name}.cpy")] {
            if !std::path::Path::new(candidate.as_str()).is_file() {
                continue;
            }
            let read = call.read(candidate.as_str()).and_then(|bytes| {
                String::from_utf8(bytes).map_err(|error| {
                    Refusal::whole(format!("copybook {candidate} is not UTF-8 text: {error}"))
                })
            });
            match read {
                Ok(body) => return Some(body),
                Err(refusal) => {
                    failure.replace(Some(refusal));
                    return None;
                }
            }
        }
        None
    };
    let parsed = parse_program(text, limits, &resolve);
    if let Some(refusal) = failure.take() {
        return common::refuse(refusal);
    }
    let program = parsed.map_err(|error| explain_refusal(&error))?;
    let verdict = program_json(&program, limits).map_err(|error| explain_refusal(&error))?;
    let mut lines = vec![format!(
        "program {} ({})",
        verdict_text(&verdict, "name")?,
        verdict_text(&verdict, "dialect")?
    )];
    lines.extend(sentence_lines(&verdict)?);
    let value = lgwks_std::json::json!({
        "kind": "cobol",
        "path": path,
        "program": verdict,
    });
    Ok(Fact {
        human: human(&lines),
        value,
    })
}

/// Explains one JCL job: its summary for a person, its JSON for agents.
fn explain_jcl(path: &str, text: &str, limits: &LegacyLimits) -> Result<Fact, Refusal> {
    let job = parse_job(text, limits).map_err(|error| explain_refusal(&error))?;
    let verdict = job_json(&job, limits).map_err(|error| explain_refusal(&error))?;
    let mut lines = vec![format!("job {}", verdict_text(&verdict, "name")?)];
    lines.extend(sentence_lines(&verdict)?);
    let value = lgwks_std::json::json!({
        "kind": "jcl",
        "path": path,
        "job": verdict,
    });
    Ok(Fact {
        human: human(&lines),
        value,
    })
}

/// The text the verdict holds at `key`: the person's view reads the same
/// JSON the agent gets, so the two can never disagree.
fn verdict_text<'verdict>(verdict: &'verdict Value, key: &str) -> Result<&'verdict str, Refusal> {
    verdict
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Refusal::whole(format!("the legacy verdict holds no {key}")))
}

/// One line per summary sentence, with the ranges a person reads:
/// `The run calls POST. (lines 5-5, lines 7-8)`.
fn sentence_lines(verdict: &Value) -> Result<Vec<String>, Refusal> {
    let missing = |what: &str| Refusal::whole(format!("the legacy verdict holds no {what}"));
    let sentences = verdict
        .get("sentences")
        .and_then(Value::as_array)
        .ok_or_else(|| missing("sentences"))?;
    let mut lines: Vec<String> = Vec::with_capacity(sentences.len());
    for sentence in sentences {
        let text = verdict_text(sentence, "text")?;
        let ranges = sentence
            .get("lines")
            .and_then(Value::as_array)
            .ok_or_else(|| missing("sentence lines"))?;
        let mut parts: Vec<String> = Vec::with_capacity(ranges.len());
        for range in ranges {
            let bound = |end: &str| {
                range
                    .get(end)
                    .and_then(Value::as_u64)
                    .ok_or_else(|| missing("line bound"))
            };
            parts.push(format!("lines {}-{}", bound("start")?, bound("end")?));
        }
        lines.push(format!("{text} ({})", parts.join(", ")));
    }
    Ok(lines)
}
