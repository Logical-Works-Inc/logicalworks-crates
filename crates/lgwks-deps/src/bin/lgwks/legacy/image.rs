//! The fax codec: bilevel TIFF pages to and from PBM bitmaps.
//!
//! PBM `P4` packs rows exactly as a TIFF page does here, most significant
//! bit first, a set bit black, each row padded to a whole byte, so a page
//! crosses between the two formats without one bit moved.

use lgwks_std::legacy::fax::{self, Compression, TiffError, TiffImage};

use super::super::common::{self, Refusal};
use super::{Fact, Invocation, codec_refusal, human, unknown_action, write_file, wrote};

/// `fax info <in>`, `fax decode <in> <out.pbm>` and `fax encode <in.pbm> <out>`.
pub(super) fn fax(call: &Invocation<'_>) -> Result<Fact, Refusal> {
    call.allow(&["image", "compression"])?;
    let action = call.word(0, "an action: info, decode or encode")?;
    let input = call.read(call.word(1, "an input file")?)?;
    match action {
        "info" => {
            let pages = decode(&input, call)?;
            let mut lines = Vec::new();
            let mut facts = Vec::new();
            for (index, page) in pages.iter().enumerate() {
                let black: u32 = page.bitmap().iter().map(|byte| byte.count_ones()).sum();
                lines.push(format!(
                    "image {index}: {} x {}, {} bytes a row, {black} black",
                    page.width(),
                    page.height(),
                    page.row_bytes()
                ));
                facts.push(lgwks_std::json::json!({
                    "width": page.width(),
                    "height": page.height(),
                    "row_bytes": page.row_bytes(),
                    "black": black,
                }));
            }
            Ok(Fact {
                value: lgwks_std::json::json!({ "images": facts }),
                human: human(&lines),
            })
        }
        "decode" => {
            let output = call.word(2, "an output PBM file")?;
            let pages = decode(&input, call)?;
            let wanted = call.number("image", 0)?;
            let Some(page) = pages.get(wanted) else {
                return common::refuse(Refusal::whole(format!(
                    "--image={wanted} is past the file's {} images",
                    pages.len()
                )));
            };
            let pbm = to_pbm(page);
            write_file(output, &pbm)?;
            Ok(wrote(output, pbm.len()))
        }
        "encode" => {
            let output = call.word(2, "an output TIFF file")?;
            let compression = match call.option("compression") {
                None | Some("g4") => Compression::G4,
                Some("g3") => Compression::G3,
                Some("raw") => Compression::Raw,
                Some(other) => {
                    return common::refuse(Refusal::whole(format!(
                        "--compression={other} is not one of g3, g4 or raw"
                    )));
                }
            };
            let page = from_pbm(&input)?;
            let tiff = fax::encode_tiff(&[page], compression, &call.limits)
                .map_err(|error: TiffError| codec_refusal(&error))?;
            write_file(output, &tiff)?;
            Ok(wrote(output, tiff.len()))
        }
        other => unknown_action(call, other, "info, decode or encode"),
    }
}

/// Every page of the TIFF in `bytes`, under the call's ceilings.
fn decode(bytes: &[u8], call: &Invocation<'_>) -> Result<Vec<TiffImage>, Refusal> {
    fax::decode_tiff(bytes, &call.limits).map_err(|error: TiffError| codec_refusal(&error))
}

/// One page as a `P4` PBM: the header, then each packed row.
fn to_pbm(page: &TiffImage) -> Vec<u8> {
    let mut pbm = format!("P4\n{} {}\n", page.width(), page.height()).into_bytes();
    for at in 0..page.height() {
        if let Some(row) = page.row(at) {
            pbm.extend_from_slice(row);
        }
    }
    pbm
}

/// The next PBM header token at or after `at`, past whitespace and `#`
/// comments: its bytes and the offset just past it.
fn token(bytes: &[u8], from: usize) -> Result<(&[u8], usize), Refusal> {
    let mut at = from;
    loop {
        match bytes.get(at).copied() {
            None => {
                return common::refuse(Refusal::whole(String::from(
                    "the PBM header ends before its magic, width and height",
                )));
            }
            Some(b'#') => {
                while let Some(byte) = bytes.get(at).copied() {
                    if byte == b'\n' {
                        break;
                    }
                    at = at.saturating_add(1);
                }
            }
            Some(byte) if byte.is_ascii_whitespace() => at = at.saturating_add(1),
            Some(_) => break,
        }
    }
    let start = at;
    while let Some(byte) = bytes.get(at).copied() {
        if byte.is_ascii_whitespace() {
            break;
        }
        at = at.saturating_add(1);
    }
    match bytes.get(start..at) {
        Some(word) => Ok((word, at)),
        None => common::refuse(Refusal::whole(String::from("the PBM header is cut short"))),
    }
}

/// The page a `P4` PBM holds: magic, width and height, one whitespace
/// byte, then the packed rows.
fn from_pbm(bytes: &[u8]) -> Result<TiffImage, Refusal> {
    let (magic, at) = token(bytes, 0)?;
    let (width, at) = token(bytes, at)?;
    let (height, at) = token(bytes, at)?;
    if magic != b"P4" {
        return common::refuse(Refusal::whole(String::from(
            "only binary PBM (P4) is read; save the bitmap as raw PBM",
        )));
    }
    let width = dimension(width, "width")?;
    let height = dimension(height, "height")?;
    let Some(rows) = bytes.get(at.saturating_add(1)..) else {
        return common::refuse(Refusal::whole(String::from(
            "the PBM ends at its header, with no rows",
        )));
    };
    TiffImage::from_bitmap(width, height, rows).map_err(|error: TiffError| codec_refusal(&error))
}

/// One PBM header number, refused when it is not a decimal `u32`.
fn dimension(spelled: &[u8], what: &str) -> Result<u32, Refusal> {
    let text = String::from_utf8_lossy(spelled);
    text.parse::<u32>()
        .map_err(|error| Refusal::whole(format!("the PBM {what} {text} is not a number: {error}")))
}
