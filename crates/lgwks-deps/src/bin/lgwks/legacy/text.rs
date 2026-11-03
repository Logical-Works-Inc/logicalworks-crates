//! The text codecs: EBCDIC pages, packed decimals and copybook records.
//!
//! A copybook with a records file decodes every record and encodes it back,
//! so each record reports whether its bytes replay: a migration learns which
//! records hold bytes the layout does not account for before it trusts them.

use lgwks_std::json::Value;
use lgwks_std::legacy::copybook::{
    CopybookError, DateValue, FieldValue, LayoutField, RecordLayout,
};
use lgwks_std::legacy::decimal::{self, Decimal, DecimalError};
use lgwks_std::legacy::ebcdic::{self, CodePage};

use super::super::common::{self, Refusal};
use super::{Fact, Invocation, codec_refusal, human, unknown_action, write_file, wrote};

/// The EBCDIC page `--page` names, CP037 when it is absent.
fn code_page(call: &Invocation<'_>) -> Result<CodePage, Refusal> {
    match call.option("page") {
        None | Some("037" | "37") => Ok(CodePage::Cp037),
        Some("500") => Ok(CodePage::Cp500),
        Some("1047") => Ok(CodePage::Cp1047),
        Some("273") => Ok(CodePage::Cp273),
        Some(other) => common::refuse(Refusal::whole(format!(
            "--page={other} is not one of 037, 500, 1047 or 273"
        ))),
    }
}

/// The UTF-8 text of `bytes` read from `path`.
fn utf8(bytes: Vec<u8>, path: &str) -> Result<String, Refusal> {
    String::from_utf8(bytes)
        .map_err(|error| Refusal::whole(format!("{path} is not UTF-8 text: {error}")))
}

/// `ebcdic decode <in>` and `ebcdic encode <in> <out>`.
pub(super) fn ebcdic(call: &Invocation<'_>) -> Result<Fact, Refusal> {
    call.allow(&["page"])?;
    let page = code_page(call)?;
    let action = call.word(0, "an action: decode or encode")?;
    let input = call.word(1, "an input file")?;
    match action {
        "decode" => {
            let bytes = call.read(input)?;
            let decoded = ebcdic::decode(page, &bytes, &call.limits)
                .map_err(|error| codec_refusal(&error))?;
            Ok(Fact {
                value: lgwks_std::json::json!({ "text": decoded.as_str(), "bytes": bytes.len() }),
                human: decoded,
            })
        }
        "encode" => {
            let output = call.word(2, "an output file")?;
            let text = utf8(call.read(input)?, input)?;
            let encoded =
                ebcdic::encode(page, &text, &call.limits).map_err(|error| codec_refusal(&error))?;
            write_file(output, &encoded)?;
            Ok(wrote(output, encoded.len()))
        }
        other => unknown_action(call, other, "decode or encode"),
    }
}

/// `packed decode <hex>` and `packed encode <value>`, both with `--digits`.
///
/// An encoded value's own decimal places are the picture's scale: `12.50`
/// under `PIC S9(3)V99` is the five digits `01250`.
pub(super) fn packed(call: &Invocation<'_>) -> Result<Fact, Refusal> {
    call.allow(&["digits", "scale", "unsigned"])?;
    let digits = call.number("digits", 0)?;
    if digits == 0 {
        return common::refuse(Refusal::whole(String::from(
            "lgwks legacy packed needs --digits=N, the picture's digit count",
        )));
    }
    let signed = !call.flag("unsigned");
    let action = call.word(0, "an action: decode or encode")?;
    let operand = call.word(1, "a value")?;
    match action {
        "decode" => {
            let bytes = lgwks_std::hex::decode(operand)
                .map_err(|error| Refusal::whole(format!("{operand} is not hex bytes: {error}")))?;
            let scale = u32::try_from(call.number("scale", 0)?)
                .map_err(|error| Refusal::whole(format!("--scale is past 32 bits: {error}")))?;
            let value: Decimal =
                decimal::decode_packed(&bytes, digits, scale, signed, &call.limits)
                    .map_err(|error: DecimalError| codec_refusal(&error))?;
            let spelled = value.to_string();
            Ok(Fact {
                value: lgwks_std::json::json!({ "value": spelled.as_str(), "digits": digits, "scale": scale }),
                human: format!("{spelled}\n"),
            })
        }
        "encode" => {
            let value = operand
                .parse::<Decimal>()
                .map_err(|error: DecimalError| codec_refusal(&error))?;
            let bytes = decimal::encode_packed(value, digits, signed, &call.limits)
                .map_err(|error: DecimalError| codec_refusal(&error))?;
            let spelled = lgwks_std::hex::encode(&bytes);
            Ok(Fact {
                value: lgwks_std::json::json!({ "hex": spelled.as_str(), "bytes": bytes.len() }),
                human: format!("{spelled}\n"),
            })
        }
        other => unknown_action(call, other, "decode or encode"),
    }
}

/// `copybook <layout> [records]`: the layout, or every record decoded.
pub(super) fn copybook(call: &Invocation<'_>) -> Result<Fact, Refusal> {
    call.allow(&["page"])?;
    let layout_path = call.word(0, "a copybook layout file")?;
    let source = utf8(call.read(layout_path)?, layout_path)?;
    let layout = RecordLayout::parse(&source, &call.limits)
        .map_err(|error: CopybookError| codec_refusal(&error))?
        .with_code_page(code_page(call)?);
    match call.positional.get(1) {
        None => Ok(describe(&layout)),
        Some(records) => decode_records(call, &layout, records),
    }
}

/// The layout as facts: its width, page, fields and condition names.
fn describe(layout: &RecordLayout) -> Fact {
    let page = format!("{:?}", layout.code_page());
    let mut lines = vec![format!("record of {} bytes on {page}", layout.record_len())];
    let mut fields = Vec::new();
    for field in layout.fields() {
        fields.push(field_json(field, 0, &mut lines));
    }
    for name in layout.condition_names() {
        lines.push(format!("88 {name}"));
    }
    Fact {
        value: lgwks_std::json::json!({
            "record_len": layout.record_len(),
            "page": page,
            "fields": fields,
            "condition_names": layout.condition_names(),
        }),
        human: human(&lines),
    }
}

/// One field and its children as JSON, with a line each for a person,
/// indented by `depth`.
fn field_json(field: &LayoutField, depth: usize, lines: &mut Vec<String>) -> Value {
    let occurs = if field.is_dynamic() {
        format!(" occurs up to {} depending", field.max_occurs())
    } else {
        String::new()
    };
    lines.push(format!(
        "{:indent$}{:>6} {:>5} {:<8} {} scale {}{occurs}",
        "",
        field.offset(),
        field.len(),
        field.kind(),
        field.name(),
        field.scale(),
        indent = depth.saturating_mul(2),
    ));
    let mut children = Vec::new();
    for child in field.children() {
        children.push(field_json(child, depth.saturating_add(1), lines));
    }
    lgwks_std::json::json!({
        "name": field.name(),
        "offset": field.offset(),
        "len": field.len(),
        "kind": field.kind(),
        "scale": field.scale(),
        "dynamic": field.is_dynamic(),
        "max_occurs": field.max_occurs(),
        "children": children,
    })
}

/// Decodes every record of the file at `path`, and encodes each back to
/// report whether its bytes replay.
fn decode_records(
    call: &Invocation<'_>,
    layout: &RecordLayout,
    path: &str,
) -> Result<Fact, Refusal> {
    let width = layout.record_len();
    if width == 0 {
        return common::refuse(Refusal::whole(String::from(
            "the layout lays out no bytes, so it holds no records",
        )));
    }
    let bytes = call.read(path)?;
    let chunks = bytes.chunks_exact(width);
    let short = chunks.remainder().len();
    if short != 0 {
        return common::refuse(Refusal::whole(format!(
            "{path} ends {short} bytes into a {width}-byte record"
        )));
    }
    let mut records = Vec::new();
    let mut lines = Vec::new();
    let mut replayed = 0_usize;
    for (index, record) in chunks.enumerate() {
        let values = layout
            .decode(record, &call.limits)
            .map_err(|error: CopybookError| codec_refusal(&error))?;
        let back = layout
            .encode(&values, &call.limits)
            .map_err(|error: CopybookError| codec_refusal(&error))?;
        let replays = back.as_slice() == record;
        if replays {
            replayed = replayed.saturating_add(1);
        }
        lines.push(if replays {
            format!("record {index}")
        } else {
            format!("record {index} (encodes back to different bytes)")
        });
        let mut named = Vec::new();
        for (field, value) in layout.fields().iter().zip(values.iter()) {
            let shown = value_json(value);
            lines.push(format!("  {} = {shown}", field.name()));
            named.push(lgwks_std::json::json!({ "name": field.name(), "value": shown }));
        }
        records.push(lgwks_std::json::json!({
            "index": index,
            "fields": named,
            "replays": replays,
        }));
    }
    Ok(Fact {
        value: lgwks_std::json::json!({
            "record_len": width,
            "records": records,
            "replayed": replayed,
        }),
        human: human(&lines),
    })
}

/// One decoded value as JSON: text as a string, a number in exact plain
/// notation, a date with its raw bytes, a group as an array.
fn value_json(value: &FieldValue) -> Value {
    match *value {
        FieldValue::Text(ref text) => Value::from(text.as_str()),
        FieldValue::Number(number) => Value::from(number.to_string()),
        FieldValue::Date(ref date) => date_json(date),
        FieldValue::Group(ref items) => Value::Array(items.iter().map(value_json).collect()),
        ref other => Value::from(format!("{other:?}")),
    }
}

/// A date's parts beside the raw bytes it was read from.
fn date_json(date: &DateValue) -> Value {
    lgwks_std::json::json!({
        "year": date.year(),
        "month": date.month(),
        "day": date.day(),
        "raw": lgwks_std::hex::encode(date.raw()),
    })
}
