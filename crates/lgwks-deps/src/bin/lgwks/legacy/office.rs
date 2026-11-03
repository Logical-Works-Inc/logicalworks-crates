//! The Office codecs: compound files, VBA modules and Jet/ACE databases.
//!
//! `access pack` builds a database from a schema and a tab-separated rows
//! file, the way a migration hands rows back to an estate still reading
//! `.mdb`; a currency cell is written at the engine's four decimal places,
//! so `12.5` is stored as `12.5000` and never rounded.

use lgwks_std::json::Value;
use lgwks_std::legacy::access::{
    AccessDb, AccessError, CellValue, ColumnDef, ColumnTag, DbVersion, rows::TableSpec,
};
use lgwks_std::legacy::cfb::{self, CfbError, CfbFile, vba};
use lgwks_std::legacy::decimal::{Decimal, DecimalError};

use super::super::common::{self, Refusal};
use super::{Fact, Invocation, codec_refusal, human, unknown_action, write_file, wrote};

/// The decimal places a Jet currency cell holds.
const CURRENCY_PLACES: usize = 4;

/// The length a text column takes when its schema names none: Jet's ceiling.
const TEXT_LEN: u16 = 255;

/// `cfb list <in>`, `cfb read <in> <stream> <out>` and `cfb pack <out> ...`.
pub(super) fn cfb(call: &Invocation<'_>) -> Result<Fact, Refusal> {
    call.allow(&[])?;
    match call.word(0, "an action: list, read or pack")? {
        "list" => {
            let file = open_cfb(call, call.word(1, "a compound file")?)?;
            let names = file.stream_names();
            Ok(Fact {
                value: lgwks_std::json::json!({ "streams": names }),
                human: human(&names),
            })
        }
        "read" => {
            let file = open_cfb(call, call.word(1, "a compound file")?)?;
            let stream = call.word(2, "a stream name")?;
            let output = call.word(3, "an output file")?;
            let bytes = file
                .read_stream(stream, &call.limits)
                .map_err(|error: CfbError| codec_refusal(&error))?;
            write_file(output, &bytes)?;
            Ok(wrote(output, bytes.len()))
        }
        "pack" => pack_cfb(call),
        other => unknown_action(call, other, "list, read or pack"),
    }
}

/// Opens the compound file at `path` under the call's ceilings.
fn open_cfb(call: &Invocation<'_>, path: &str) -> Result<CfbFile, Refusal> {
    CfbFile::open(&call.read(path)?, &call.limits).map_err(|error: CfbError| codec_refusal(&error))
}

/// `cfb pack <out> <name=path>...`: one stream per pair, in the order given.
fn pack_cfb(call: &Invocation<'_>) -> Result<Fact, Refusal> {
    let output = call.word(1, "an output file")?;
    let mut streams: Vec<(&str, Vec<u8>)> = Vec::new();
    for pair in call.positional.iter().skip(2) {
        let Some((name, path)) = pair.split_once('=') else {
            return common::refuse(Refusal::whole(format!(
                "{pair} is not name=path: each stream is named, then its file"
            )));
        };
        streams.push((name, call.read(path)?));
    }
    if streams.is_empty() {
        return common::refuse(Refusal::whole(String::from(
            "lgwks legacy cfb pack needs at least one name=path stream",
        )));
    }
    let views: Vec<(&str, &[u8])> = streams
        .iter()
        .map(|&(name, ref bytes)| (name, bytes.as_slice()))
        .collect();
    let image =
        cfb::build_image(&views, &call.limits).map_err(|error: CfbError| codec_refusal(&error))?;
    write_file(output, &image)?;
    Ok(wrote(output, image.len()))
}

/// `vba decompress <in> <out>` and `vba compress <in> <out>` (MS-OVBA).
pub(super) fn vba(call: &Invocation<'_>) -> Result<Fact, Refusal> {
    call.allow(&[])?;
    let action = call.word(0, "an action: decompress or compress")?;
    let input = call.read(call.word(1, "an input file")?)?;
    let output = call.word(2, "an output file")?;
    let converted = match action {
        "decompress" => vba::decompress(&input, &call.limits),
        "compress" => vba::compress(&input, &call.limits),
        other => return unknown_action(call, other, "decompress or compress"),
    }
    .map_err(|error| codec_refusal(&error))?;
    write_file(output, &converted)?;
    Ok(wrote(output, converted.len()))
}

/// `access list <in>`, `access rows <in> <table>` and `access pack ...`.
pub(super) fn access(call: &Invocation<'_>) -> Result<Fact, Refusal> {
    call.allow(&["engine"])?;
    match call.word(0, "an action: list, rows or pack")? {
        "list" => {
            let db = open_access(call)?;
            let engine: DbVersion = db.version();
            let mut lines = vec![format!("engine {engine:?}")];
            let mut tables = Vec::new();
            for name in db.tables() {
                let count = db
                    .rows(&name)
                    .map_err(|error: AccessError| codec_refusal(&error))?
                    .len();
                lines.push(format!("{name}: {count} rows"));
                tables.push(lgwks_std::json::json!({ "name": name, "rows": count }));
            }
            Ok(Fact {
                value: lgwks_std::json::json!({ "engine": format!("{engine:?}"), "tables": tables }),
                human: human(&lines),
            })
        }
        "rows" => {
            let db = open_access(call)?;
            let table = call.word(2, "a table name")?;
            let rows = db
                .rows(table)
                .map_err(|error: AccessError| codec_refusal(&error))?;
            let mut lines = Vec::new();
            let mut values = Vec::new();
            for row in rows {
                let cells: Vec<Value> = row.cells().iter().map(cell_json).collect();
                let spelled: Vec<String> = cells.iter().map(Value::to_string).collect();
                lines.push(spelled.join("\t"));
                values.push(Value::Array(cells));
            }
            Ok(Fact {
                value: lgwks_std::json::json!({ "table": table, "rows": values }),
                human: human(&lines),
            })
        }
        "pack" => pack_access(call),
        other => unknown_action(call, other, "list, rows or pack"),
    }
}

/// Opens the database the call's second word names.
fn open_access(call: &Invocation<'_>) -> Result<AccessDb, Refusal> {
    let path = call.word(1, "a database file")?;
    AccessDb::open(&call.read(path)?, &call.limits)
        .map_err(|error: AccessError| codec_refusal(&error))
}

/// One cell as JSON: currency in exact plain notation, a date as its days
/// and milliseconds.
fn cell_json(cell: &CellValue) -> Value {
    match *cell {
        CellValue::Text(ref text) | CellValue::Memo(ref text) => Value::String(text.clone()),
        CellValue::Integer(number) => Value::from(number),
        CellValue::Currency(amount) => Value::String(amount.to_string()),
        CellValue::Date { days, millis } => {
            lgwks_std::json::json!({ "days": days, "millis": millis })
        }
        CellValue::Null => Value::Null,
        ref other => Value::String(format!("{other:?}")),
    }
}

/// `access pack <out> <table> <schema> <rows.tsv>`: one table, written
/// under `--engine` (Jet4 when absent).
fn pack_access(call: &Invocation<'_>) -> Result<Fact, Refusal> {
    let output = call.word(1, "an output file")?;
    let table = call.word(2, "a table name")?;
    let (columns, tags) = schema(call.word(3, "a schema: name:kind[:len],...")?)?;
    let rows_path = call.word(4, "a rows file")?;
    let text = String::from_utf8(call.read(rows_path)?)
        .map_err(|error| Refusal::whole(format!("{rows_path} is not UTF-8 text: {error}")))?;
    let mut rows = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if !line.is_empty() {
            rows.push(row(&tags, line, index.saturating_add(1))?);
        }
    }
    let engine = match call.option("engine") {
        Some("jet3") => DbVersion::Jet3,
        None | Some("jet4") => DbVersion::Jet4,
        Some("ace") => DbVersion::Ace,
        Some(other) => {
            return common::refuse(Refusal::whole(format!(
                "--engine={other} is not one of jet3, jet4 or ace"
            )));
        }
    };
    let tables: [TableSpec<'_>; 1] = [(table, columns.as_slice(), rows.as_slice())];
    let image = lgwks_std::legacy::access::rows::build_image(engine, &tables, &call.limits)
        .map_err(|error: AccessError| codec_refusal(&error))?;
    write_file(output, &image)?;
    Ok(wrote(output, image.len()))
}

/// The columns a `name:kind[:len]` list names, beside each one's kind.
fn schema(spelled: &str) -> Result<(Vec<ColumnDef>, Vec<ColumnTag>), Refusal> {
    let mut columns = Vec::new();
    let mut tags = Vec::new();
    for column in spelled.split(',') {
        let mut parts = column.split(':');
        let (Some(name), Some(kind)) = (parts.next(), parts.next()) else {
            return common::refuse(Refusal::whole(format!(
                "column {column} is not name:kind[:len]"
            )));
        };
        let tag = match kind {
            "text" => ColumnTag::Text,
            "integer" => ColumnTag::Integer,
            "currency" => ColumnTag::Currency,
            "date" => ColumnTag::Date,
            "memo" => ColumnTag::Memo,
            other => {
                return common::refuse(Refusal::whole(format!(
                    "column {name} has kind {other}: one of text, integer, currency, date or memo"
                )));
            }
        };
        let len = match parts.next() {
            None => TEXT_LEN,
            Some(width) => width.parse::<u16>().map_err(|error| {
                Refusal::whole(format!("column {name} has length {width}: {error}"))
            })?,
        };
        columns.push(ColumnDef::new(name, tag, len));
        tags.push(tag);
    }
    Ok((columns, tags))
}

/// One rows-file line as cells, each read as its column's kind.
fn row(tags: &[ColumnTag], line: &str, number: usize) -> Result<Vec<CellValue>, Refusal> {
    let cells: Vec<&str> = line.split('\t').collect();
    if cells.len() != tags.len() {
        return common::refuse(Refusal::whole(format!(
            "line {number} holds {} cells for {} columns",
            cells.len(),
            tags.len()
        )));
    }
    let mut row = Vec::with_capacity(tags.len());
    for (&tag, cell) in tags.iter().zip(cells) {
        row.push(
            cell_of(tag, cell)
                .map_err(|why| Refusal::whole(format!("line {number}: {}", why.message())))?,
        );
    }
    Ok(row)
}

/// One cell read as `tag`: an empty memo is null, a date is `days[:millis]`.
fn cell_of(tag: ColumnTag, cell: &str) -> Result<CellValue, Refusal> {
    match tag {
        ColumnTag::Text => Ok(CellValue::Text(String::from(cell))),
        ColumnTag::Memo if cell.is_empty() => Ok(CellValue::Null),
        ColumnTag::Memo => Ok(CellValue::Memo(String::from(cell))),
        ColumnTag::Integer => cell
            .parse::<i32>()
            .map(CellValue::Integer)
            .map_err(|error| Refusal::whole(format!("{cell} is not a 32-bit integer: {error}"))),
        ColumnTag::Currency => currency(cell).map(CellValue::Currency),
        ColumnTag::Date => {
            let (days, millis) = match cell.split_once(':') {
                Some(pair) => pair,
                None => (cell, "0"),
            };
            let days = days
                .parse::<i32>()
                .map_err(|error| Refusal::whole(format!("{cell} names no day: {error}")))?;
            let millis = millis
                .parse::<u32>()
                .map_err(|error| Refusal::whole(format!("{cell} names no millisecond: {error}")))?;
            Ok(CellValue::Date { days, millis })
        }
        other => common::refuse(Refusal::whole(format!(
            "a {other:?} column cannot be packed from text"
        ))),
    }
}

/// A currency cell at Jet's four places: `12.5` is coefficient 125000.
fn currency(cell: &str) -> Result<Decimal, Refusal> {
    let places = match cell.split_once('.') {
        Some((_, fraction)) => fraction.len(),
        None => 0,
    };
    if places > CURRENCY_PLACES {
        return common::refuse(Refusal::whole(format!(
            "{cell} has {places} decimal places; currency holds {CURRENCY_PLACES}"
        )));
    }
    let point = if cell.contains('.') { "" } else { "." };
    let zeros = "0".repeat(CURRENCY_PLACES.saturating_sub(places));
    format!("{cell}{point}{zeros}")
        .parse::<Decimal>()
        .map_err(|error: DecimalError| codec_refusal(&error))
}
