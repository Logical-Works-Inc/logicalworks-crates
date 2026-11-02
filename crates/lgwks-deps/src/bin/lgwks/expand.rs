//! `lgwks expand [file]`: print the Rust the file's `script!` compiles to.
//!
//! The expansion calls the one emitter, `lgwks_ast::script::emit::script`,
//! on the tree the one parser answers, then formats the tokens. What it
//! prints is byte-for-byte what the macro compiles: expanded and macro
//! output are never two code paths.

use std::io::Write;
use std::process::ExitCode;

use super::common::{self, Refusal, RefusalKind};

/// The command's own usage.
const EXPAND_USAGE: &str = "\
lgwks expand [file] [--json]

Print the formatted Rust the app file's script! block compiles to: the one
emitter's answer, formatted. [file] is the app file, its directory, or
app.rs when omitted.
";

/// Expand the app file `named` names, and print the formatted Rust.
pub(crate) fn run(
    args: &[String],
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<ExitCode> {
    let source = match common::open_app("expand", EXPAND_USAGE, args, json, out, err)? {
        Ok(source) => source,
        Err(code) => return Ok(code),
    };
    expand_source(source.text(), json, out, err)
}

/// Parse, emit and format `text`, through the one emitter.
fn expand_source(
    text: &str,
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<ExitCode> {
    let script = match common::load_script(text) {
        Ok(script) => script,
        Err(refusal) => return common::report_one(refusal, json, out, err),
    };
    let expanded = match lgwks_ast::script::emit::script(&script) {
        Ok(expanded) => expanded,
        Err(refusal) => {
            let rendered = Refusal::at(
                format!("{}:{}", refusal.line(), refusal.column().saturating_add(1)),
                RefusalKind::Malformed,
                refusal.message().to_owned(),
                None,
            );
            return common::report_one(rendered, json, out, err);
        }
    };
    // The token type is never named: it is inferred from what the one
    // emitter answers, so this file reaches no token edge of its own.
    let file: syn::File = match syn::parse2(expanded) {
        Ok(file) => file,
        Err(error) => {
            lgwks_std::trace::debug!(error = %error, "lgwks expand: the emitted tokens did not parse");
            let why = format!("the emitted Rust did not parse: {error}");
            return common::fail(why, json, out, err);
        }
    };
    let rendered = prettyplease::unparse(&file);
    if json {
        let value = lgwks_std::json::json!({
            "ok": true,
            "expanded": rendered,
        });
        common::json_line(&value, out).map(|()| ExitCode::SUCCESS)
    } else {
        writeln!(out, "{rendered}").map(|()| ExitCode::SUCCESS)
    }
}
