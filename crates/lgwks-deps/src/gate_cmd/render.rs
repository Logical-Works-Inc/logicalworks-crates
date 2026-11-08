//! The gate's two renderings of its legs, and the one JSON string writer.

use std::io;
use std::path::Path;

use super::{Leg, Verdict};

/// One JSON string field: quoted, with every character RFC 8259 forbids raw
/// escaped, so a tool's transcript or an advisory's text cannot break the document.
fn field(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len().saturating_add(2));
    quoted.push('"');
    for ch in text.chars() {
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push(' '),
            control if u32::from(control) < 0x20 => {
                let escape = format!("\\u{:04x}", u32::from(control));
                quoted.push_str(&escape);
            }
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

/// The machine rendering: one document, one leg per row.
pub(super) fn render_json(root: &Path, base: &str, legs: &[Leg]) -> String {
    let rows: Vec<String> = legs
        .iter()
        .map(|leg| {
            format!(
                "{{\"name\":{},\"verdict\":{},\"detail\":{}}}",
                field(leg.name),
                field(leg.verdict.word()),
                field(&leg.detail),
            )
        })
        .collect();
    format!(
        "{{\"tool\":\"lgwks-deps-gate\",\"path\":{},\"base\":{},\"legs\":[\n{}\n]}}\n",
        field(&root.to_string_lossy()),
        field(base),
        rows.join(",\n"),
    )
}

/// The human rendering: one summary line per leg, the transcripts of the
/// legs that did not pass, then the verdict.
pub(super) fn render_human(root: &Path, legs: &[Leg], out: &mut impl io::Write) -> io::Result<()> {
    for leg in legs {
        writeln!(
            out,
            "{:13} {:5}  {}",
            leg.name,
            leg.verdict.word(),
            leg.detail
        )?;
    }
    for leg in legs.iter().filter(|leg| leg.verdict != Verdict::Pass) {
        writeln!(
            out,
            "\n--- {} ({}) ---\n{}",
            leg.name,
            leg.verdict.word(),
            leg.transcript
        )?;
    }
    let failures = legs
        .iter()
        .filter(|leg| leg.verdict == Verdict::Fail)
        .count();
    let errors = legs
        .iter()
        .filter(|leg| leg.verdict == Verdict::Error)
        .count();
    let skips = legs
        .iter()
        .filter(|leg| leg.verdict == Verdict::Skip)
        .count();
    if failures == 0 && errors == 0 {
        writeln!(
            out,
            "GATE PASS  {} — every applicable leg passed{}",
            root.display(),
            if skips == 0 {
                String::new()
            } else {
                format!(", {skips} skipped (named above, not evidence)")
            }
        )
    } else {
        writeln!(
            out,
            "GATE REFUSED  {} — {failures} leg(s) failed, {errors} errored",
            root.display()
        )
    }
}
