//! `lgwks legacy <codec> ...`: the mainframe and Office formats a migration
//! reads and writes, from the shell.
//!
//! Each codec of `lgwks_std::legacy` is one word: `ebcdic`, `packed`,
//! `copybook`, `cfb`, `vba`, `access` and `fax`. Every read runs under the
//! codec ceilings, raised with `--max-bytes=N`, `--max-output=N`,
//! `--max-records=N` and `--max-pages=N`. A refusal past a ceiling is
//! `bound-exceeded`, any other is `malformed`, and both name the byte that
//! earned them. Files are read no further than one byte past the input
//! ceiling, so an oversized input is refused without being held whole.

mod explain;
mod image;
mod office;
mod text;

use std::collections::BTreeMap;
use std::error::Error;
use std::io::{Read, Write};
use std::process::ExitCode;

use lgwks_std::json::Value;
use lgwks_std::legacy::limits::{LimitError, Limits};

use super::common::{self, Refusal};

/// The command's own usage.
const LEGACY_USAGE: &str = "\
lgwks legacy <codec> <action> [args] [--json]

Read and write the formats legacy estates run on, exactly and under bounds.

  ebcdic decode <in> [--page=037|500|1047|273]   EBCDIC bytes to text
  ebcdic encode <in> <out> [--page=...]          text to EBCDIC bytes
  packed decode <hex> --digits=N [--scale=S] [--unsigned]
  packed encode <value> --digits=N [--unsigned]  COMP-3 and exact decimals
  copybook <layout> [records] [--page=...]       the layout, or each record
  explain <program.cbl|job.jcl> [--copy-dir=DIR] what a program or job does
  cfb list <in>                                  a compound file's streams
  cfb read <in> <stream> <out>                   one stream's bytes
  cfb pack <out> <name=path>...                  a compound file from files
  vba decompress <in> <out>                      a VBA module to its source
  vba compress <in> <out>                        source to a VBA module
  access list <in>                               the engine and its tables
  access rows <in> <table>                       one table's rows
  access pack <out> <table> <schema> <rows.tsv> [--engine=jet3|jet4|ace]
  fax info <in>                                  each TIFF page's size
  fax decode <in> <out.pbm> [--image=N]          one page as a PBM bitmap
  fax encode <in.pbm> <out> [--compression=g3|g4|raw]

Every read takes --max-bytes=N (8388608), --max-output=N (33554432),
--max-records=N (100000) and --max-pages=N (4096). A schema is
name:kind[:len] joined by commas; kind is text, integer, currency, date or
memo. A rows file holds one row per line, cells split by tabs.
";

/// The input ceiling when `--max-bytes` is not given: 8 MiB.
const DEFAULT_MAX_BYTES: usize = 8_388_608;

/// The output ceiling when `--max-output` is not given: 32 MiB.
const DEFAULT_MAX_OUTPUT: usize = 33_554_432;

/// The record ceiling when `--max-records` is not given.
const DEFAULT_MAX_RECORDS: usize = 100_000;

/// The page ceiling when `--max-pages` is not given.
const DEFAULT_MAX_PAGES: usize = 4_096;

/// The deepest copybook nesting admitted; COBOL levels stop at 49.
const MAX_NESTING: usize = 64;

/// The ceiling options every codec takes.
const CEILINGS: [&str; 4] = ["max-bytes", "max-output", "max-records", "max-pages"];

/// One `legacy` invocation: the codec, its positionals, its options.
struct Invocation<'args> {
    /// The codec word: `ebcdic`, `packed` and the rest.
    codec: &'args str,
    /// The positional arguments after the codec word, in order.
    positional: Vec<&'args str>,
    /// Each `--key=value` option by key; a bare `--flag` maps to "".
    options: BTreeMap<&'args str, &'args str>,
    /// The input ceiling, also the read cap on every file.
    max_bytes: usize,
    /// The ceilings every codec call runs under.
    limits: Limits,
}

impl<'args> Invocation<'args> {
    /// The positional at `at`, or a refusal naming the `what` it should be.
    fn word(&self, at: usize, what: &str) -> Result<&'args str, Refusal> {
        match self.positional.get(at) {
            Some(word) => Ok(*word),
            None => common::refuse(Refusal::whole(format!(
                "lgwks legacy {} needs {what}; see `lgwks legacy --help`",
                self.codec
            ))),
        }
    }

    /// The value of `--key=value`, when given.
    fn option(&self, key: &str) -> Option<&'args str> {
        self.options.get(key).copied()
    }

    /// Whether the bare flag `--key` was given.
    fn flag(&self, key: &str) -> bool {
        self.options.contains_key(key)
    }

    /// The whole number `--key=N` names, or `default` when it is absent.
    fn number(&self, key: &str, default: usize) -> Result<usize, Refusal> {
        whole_number(self.option(key), key, default)
    }

    /// Refuses any option outside `allowed` and the ceilings.
    fn allow(&self, allowed: &[&str]) -> Result<(), Refusal> {
        for key in self.options.keys() {
            if !allowed.contains(key) && !CEILINGS.contains(key) {
                return common::refuse(Refusal::whole(format!(
                    "lgwks legacy {} takes no option --{key}",
                    self.codec
                )));
            }
        }
        Ok(())
    }

    /// The bytes at `path`, read no further than one byte past the input
    /// ceiling, so the codec refuses an oversized file by its length.
    fn read(&self, path: &str) -> Result<Vec<u8>, Refusal> {
        let file = std::fs::File::open(path)
            .map_err(|error| Refusal::whole(format!("{path} could not be opened: {error}")))?;
        let reach = match u64::try_from(self.max_bytes.saturating_add(1)) {
            Ok(reach) => reach,
            Err(error) => {
                return common::refuse(Refusal::bound(format!(
                    "--max-bytes={} is past this platform's file offsets: {error}",
                    self.max_bytes
                )));
            }
        };
        let mut bytes = Vec::new();
        file.take(reach)
            .read_to_end(&mut bytes)
            .map_err(|error| Refusal::whole(format!("{path} could not be read: {error}")))?;
        Ok(bytes)
    }
}

/// The whole number `text` spells for `--key`, or `default` when absent.
fn whole_number(text: Option<&str>, key: &str, default: usize) -> Result<usize, Refusal> {
    match text {
        None => Ok(default),
        Some(spelled) => spelled.parse::<usize>().map_err(|error| {
            Refusal::whole(format!("--{key}={spelled} is not a whole number: {error}"))
        }),
    }
}

/// Writes `bytes` to `path`, naming the path when the write fails.
fn write_file(path: &str, bytes: &[u8]) -> Result<(), Refusal> {
    std::fs::write(path, bytes)
        .map_err(|error| Refusal::whole(format!("{path} could not be written: {error}")))
}

/// The refusal a codec error earns: past a ceiling is a bound the caller can
/// raise, anything else is the input's own fault, named where it sits.
fn codec_refusal(error: &(dyn Error + 'static)) -> Refusal {
    match error
        .source()
        .and_then(|source| source.downcast_ref::<LimitError>())
    {
        Some(cause) => Refusal::bound(format!(
            "{cause}; raise it with --max-bytes, --max-output, --max-records or --max-pages"
        )),
        None => Refusal::whole(error.to_string()),
    }
}

/// What a finished action reports: the facts for `--json`, the text for a
/// person.
struct Fact {
    /// The facts, one JSON object; `ok` is added when it prints.
    value: Value,
    /// The same facts as lines for a person, newline-terminated.
    human: String,
}

/// The text a person reads: `lines`, each newline-terminated.
fn human(lines: &[String]) -> String {
    format!("{}\n", lines.join("\n"))
}

/// The fact of `count` bytes written to `path`.
fn wrote(path: &str, count: usize) -> Fact {
    Fact {
        value: lgwks_std::json::json!({ "out": path, "bytes": count }),
        human: format!("wrote {count} bytes to {path}\n"),
    }
}

/// Refuses an action word the codec does not hold, naming the ones it does.
fn unknown_action(call: &Invocation<'_>, other: &str, actions: &str) -> Result<Fact, Refusal> {
    common::refuse(Refusal::whole(format!(
        "lgwks legacy {} has no action {other}: one of {actions}",
        call.codec
    )))
}

/// Splits the arguments into the codec word, positionals and options, and
/// sets the ceilings the options name.
fn invocation(args: &[String]) -> Result<Invocation<'_>, Refusal> {
    let mut words: Vec<&str> = Vec::new();
    let mut options = BTreeMap::new();
    for argument in args {
        if let Some(option) = argument.strip_prefix("--") {
            let (key, value) = match option.split_once('=') {
                Some(pair) => pair,
                None => (option, ""),
            };
            options.insert(key, value);
        } else {
            words.push(argument.as_str());
        }
    }
    let Some((&codec, positional)) = words.split_first() else {
        return common::refuse(Refusal::whole(String::from(
            "lgwks legacy needs a codec: ebcdic, packed, copybook, explain, cfb, vba, access or fax",
        )));
    };
    let max_bytes = whole_number(
        options.get("max-bytes").copied(),
        "max-bytes",
        DEFAULT_MAX_BYTES,
    )?;
    let limits = Limits::new(
        max_bytes,
        whole_number(
            options.get("max-records").copied(),
            "max-records",
            DEFAULT_MAX_RECORDS,
        )?,
        whole_number(
            options.get("max-pages").copied(),
            "max-pages",
            DEFAULT_MAX_PAGES,
        )?,
        MAX_NESTING,
        whole_number(
            options.get("max-output").copied(),
            "max-output",
            DEFAULT_MAX_OUTPUT,
        )?,
    );
    Ok(Invocation {
        codec,
        positional: positional.to_vec(),
        options,
        max_bytes,
        limits,
    })
}

/// Runs the codec the invocation names.
fn route(call: &Invocation<'_>) -> Result<Fact, Refusal> {
    match call.codec {
        "ebcdic" => text::ebcdic(call),
        "packed" => text::packed(call),
        "copybook" => text::copybook(call),
        "explain" => explain::explain(call),
        "cfb" => office::cfb(call),
        "vba" => office::vba(call),
        "access" => office::access(call),
        "fax" => image::fax(call),
        other => common::refuse(Refusal::whole(format!(
            "unknown codec {other}: one of ebcdic, packed, copybook, explain, cfb, vba, access or fax"
        ))),
    }
}

/// Run `lgwks legacy` and report its verdict.
pub(crate) fn run(
    args: &[String],
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<ExitCode> {
    if args.is_empty()
        || args
            .iter()
            .any(|argument| argument == "--help" || argument == "-h")
    {
        return write!(out, "{LEGACY_USAGE}").map(|()| ExitCode::SUCCESS);
    }
    let verdict = invocation(args).and_then(|call| route(&call));
    match verdict {
        Ok(fact) if json => {
            let mut value = fact.value;
            if let Some(object) = value.as_object_mut() {
                object.insert(String::from("ok"), Value::Bool(true));
            }
            common::json_line(&value, out).map(|()| ExitCode::SUCCESS)
        }
        Ok(fact) => write!(out, "{}", fact.human).map(|()| ExitCode::SUCCESS),
        Err(refusal) => common::report_one(refusal, json, out, err),
    }
}
