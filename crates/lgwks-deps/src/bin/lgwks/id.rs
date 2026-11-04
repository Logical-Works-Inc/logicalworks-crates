//! `lgwks id <action>`: time-ordered identities and the FoundationDB key bytes
//! they pack to, from the shell (#851, ADR 0010).
//!
//! `new` issues version 7 identifiers from `Uuid::now_v7`, which never
//! refuses. `key` packs each identifier as the tuple-layer UUID element, `0x30`
//! then its 16 bytes, so an operator can find a row by its key. `read` takes a
//! key back to the identifier at its front and the bytes after it, and refuses
//! another type code or a short key by name.

use std::io::Write;
use std::process::ExitCode;

use lgwks_std::id::{TupleError, Uuid};
use lgwks_std::json::{Value, json};

use super::common::{self, Refusal};

/// The command's own usage.
const ID_USAGE: &str = "\
lgwks id <action> [args] [--json]

Time-ordered identities and the FoundationDB key bytes they pack to.

  new [--count=N]     N version 7 ids (default 1, at most 10000), in issue order
  key <uuid>...       each id's tuple-layer element as hex: 30 then its 16 bytes
  read <hex>          the id at the front of a packed key, and the bytes after it
";

/// The most identifiers one `new` issues.
const MAX_COUNT: usize = 10_000;

/// What a finished action reports: the facts for `--json`, the text for a
/// person.
struct Fact {
    /// The facts, one JSON object; `ok` is added when it prints.
    value: Value,
    /// The same facts as lines for a person, newline-terminated.
    human: String,
}

/// `count` fresh identifiers, in issue order.
fn new_ids(options: &[&str]) -> Result<Fact, Refusal> {
    let mut count = 1_usize;
    for option in options {
        let Some(spelled) = option.strip_prefix("count=") else {
            return common::refuse(Refusal::whole(format!(
                "lgwks id new takes no option --{option}"
            )));
        };
        count = match spelled.parse::<usize>() {
            Ok(parsed) if (1..=MAX_COUNT).contains(&parsed) => parsed,
            Ok(parsed) => {
                return common::refuse(Refusal::bound(format!(
                    "--count={parsed} is outside 1..={MAX_COUNT}"
                )));
            }
            Err(error) => {
                return common::refuse(Refusal::whole(format!(
                    "--count={spelled} is not a whole number: {error}"
                )));
            }
        };
    }
    let ids: Vec<String> = (0..count).map(|_| Uuid::now_v7().to_string()).collect();
    Ok(Fact {
        human: format!("{}\n", ids.join("\n")),
        value: json!({ "ids": ids }),
    })
}

/// The tuple-layer key bytes of each named identifier, as hex.
fn keys(words: &[&str]) -> Result<Fact, Refusal> {
    if words.is_empty() {
        return common::refuse(Refusal::whole(String::from(
            "lgwks id key needs at least one id",
        )));
    }
    let mut rows = Vec::with_capacity(words.len());
    let mut lines = Vec::with_capacity(words.len());
    for word in words {
        let id = Uuid::parse(word)
            .map_err(|error| Refusal::whole(format!("{word} is not a UUID: {error}")))?;
        let mut key = Vec::with_capacity(17);
        id.pack_tuple(&mut key);
        let hex = lgwks_std::hex::encode(&key);
        lines.push(format!("{id} {hex}"));
        rows.push(json!({ "id": id.to_string(), "key": hex }));
    }
    Ok(Fact {
        human: format!("{}\n", lines.join("\n")),
        value: json!({ "keys": rows }),
    })
}

/// The identifier at the front of the packed key `text` spells in hex.
fn read_key(words: &[&str]) -> Result<Fact, Refusal> {
    let &[text] = words else {
        return common::refuse(Refusal::whole(String::from(
            "lgwks id read needs exactly one hex key",
        )));
    };
    let key = lgwks_std::hex::decode(text)
        .map_err(|error| Refusal::whole(format!("{text} is not hex: {error}")))?;
    let (id, rest) = Uuid::unpack_tuple(&key).map_err(|error| tuple_refusal(error, text))?;
    let rest = lgwks_std::hex::encode(rest);
    Ok(Fact {
        human: format!("{id}\nrest {rest}\n"),
        value: json!({ "id": id.to_string(), "rest": rest }),
    })
}

/// The refusal a key that does not lead with a UUID element earns.
fn tuple_refusal(error: TupleError, text: &str) -> Refusal {
    Refusal::whole(format!("{text} does not lead with a UUID element: {error}"))
}

/// Runs the action the arguments name.
fn route(args: &[String]) -> Result<Fact, Refusal> {
    let mut words: Vec<&str> = Vec::new();
    let mut options: Vec<&str> = Vec::new();
    for argument in args {
        match argument.strip_prefix("--") {
            Some(option) => options.push(option),
            None => words.push(argument.as_str()),
        }
    }
    let Some((&action, rest)) = words.split_first() else {
        return common::refuse(Refusal::whole(String::from(
            "lgwks id needs an action: new, key or read",
        )));
    };
    if action != "new" && !options.is_empty() {
        return common::refuse(Refusal::whole(format!(
            "lgwks id {action} takes no options"
        )));
    }
    match action {
        "new" if rest.is_empty() => new_ids(&options),
        "new" => common::refuse(Refusal::whole(String::from(
            "lgwks id new takes no positional arguments",
        ))),
        "key" => keys(rest),
        "read" => read_key(rest),
        other => common::refuse(Refusal::whole(format!(
            "lgwks id has no action {other}: one of new, key or read"
        ))),
    }
}

/// Run `lgwks id` and report its verdict.
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
        return write!(out, "{ID_USAGE}").map(|()| ExitCode::SUCCESS);
    }
    match route(args) {
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
