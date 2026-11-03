//! TIFF container: headers, IFDs, strips and pages (TIFF 6.0 Part 1).
//!
//! Single- and multi-page bilevel images whose strips hold raw, G3 or G4
//! bytes. [`decode_tiff`] reads every page, reading only the tags a bilevel
//! page needs and passing over the rest (resolution, software, dates), with
//! FillOrder, `T4Options` and `T6Options` honoured. [`encode_tiff`] writes
//! one WhiteIsZero strip per page with word-aligned, chained IFDs.

use super::super::limits::Limits;
use super::{Compression, Strip, TiffError, TiffImage, t4, t6};
use crate::legacy::{get_u16_be, get_u16_le, get_u32_be, get_u32_le, push_u16_le, push_u32_le};

/// TIFF little-endian magic `II`.
const TIFF_LE: u16 = 0x4949;

/// TIFF big-endian magic `MM`.
const TIFF_BE: u16 = 0x4d4d;

/// The TIFF version number every header carries (TIFF 6.0 §2).
const TIFF_VERSION: u16 = 42;

/// Bytes of one IFD entry (TIFF 6.0 §2).
const IFD_ENTRY_LEN: usize = 12;

/// The SHORT field type (TIFF 6.0 §2).
const TYPE_SHORT: u16 = 3;

/// The LONG field type (TIFF 6.0 §2).
const TYPE_LONG: u16 = 4;

/// The ImageWidth tag (TIFF 6.0 §2).
const TAG_WIDTH: u16 = 256;

/// The ImageLength tag.
const TAG_HEIGHT: u16 = 257;

/// The BitsPerSample tag: must be `[1]` for bilevel.
const TAG_BPS: u16 = 258;

/// The Compression tag: 1 raw, 3 G3, 4 G4.
const TAG_COMPRESSION: u16 = 259;

/// The PhotometricInterpretation tag: 0 WhiteIsZero, 1 BlackIsZero.
const TAG_PHOTOMETRIC: u16 = 262;

/// The FillOrder tag: 1 most significant bit first, 2 least.
const TAG_FILL_ORDER: u16 = 266;

/// The StripOffsets tag.
const TAG_STRIP_OFFSETS: u16 = 273;

/// The RowsPerStrip tag.
const TAG_ROWS_PER_STRIP: u16 = 278;

/// The StripByteCounts tag.
const TAG_STRIP_COUNTS: u16 = 279;

/// The T4Options tag (TIFF 6.0 §11): bit 0 2D rows, bit 1 uncompressed
/// mode, bit 2 fill bits before EOLs.
const TAG_T4_OPTIONS: u16 = 292;

/// The T6Options tag (TIFF 6.0 §11): bit 1 uncompressed mode.
const TAG_T6_OPTIONS: u16 = 293;

/// The tags a bilevel page reads; every other tag is passed over.
const READ_TAGS: [u16; 11] = [
    TAG_WIDTH,
    TAG_HEIGHT,
    TAG_BPS,
    TAG_COMPRESSION,
    TAG_PHOTOMETRIC,
    TAG_FILL_ORDER,
    TAG_STRIP_OFFSETS,
    TAG_ROWS_PER_STRIP,
    TAG_STRIP_COUNTS,
    TAG_T4_OPTIONS,
    TAG_T6_OPTIONS,
];

/// The `T4Options` bit for 2D rows.
const T4_TWO_D: u32 = 1;

/// The uncompressed-mode bit both option tags share.
const UNCOMPRESSED_MODE: u32 = 2;

/// Raw bit-packed compression (TIFF Compression = 1).
const RAW: u32 = 1;

/// CCITT T.4 G3 compression (TIFF Compression = 3).
const G3: u32 = 3;

/// CCITT T.6 G4 compression (TIFF Compression = 4).
const G4: u32 = 4;

/// Whether the image bytes read little-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    /// Intel order.
    Little,
    /// Motorola order.
    Big,
}

/// One decoded IFD: the fields a page needs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PageTags {
    /// Image width in pixels.
    width: u32,
    /// Image height in rows.
    height: u32,
    /// Bits per sample; must be 1.
    bits: u32,
    /// Compression: 1, 3 or 4.
    compression: u32,
    /// Photometric: 0 WhiteIsZero, 1 BlackIsZero.
    photometric: u32,
    /// Fill order: 1 most significant bit first, 2 least.
    fill_order: u32,
    /// Rows per strip.
    rows_per_strip: u32,
    /// `T4Options` bits.
    t4_options: u32,
    /// `T6Options` bits.
    t6_options: u32,
    /// Strip byte offsets.
    offsets: Vec<u32>,
    /// Strip byte counts.
    counts: Vec<u32>,
}

/// Decodes every page of a TIFF image.
///
/// # Errors
///
/// [`TiffError`] for a bad header, truncation, tag, shape or corrupt strip,
/// or [`TiffError::Limit`] past a ceiling.
pub(crate) fn decode_tiff(bytes: &[u8], limits: &Limits) -> Result<Vec<TiffImage>, TiffError> {
    limits.check_bytes(bytes.len())?;
    let order = decode_order(bytes)?;
    let Some(version) = read_u16(bytes, 2, order) else {
        return crate::legacy::refuse(TiffError::Truncated { offset: 2, need: 4 });
    };
    if version != TIFF_VERSION {
        return crate::legacy::refuse(TiffError::BadHeader);
    }
    let Some(mut next) = read_u32(bytes, 4, order) else {
        return crate::legacy::refuse(TiffError::Truncated { offset: 4, need: 8 });
    };
    let mut images: Vec<TiffImage> = Vec::new();
    while next != 0 {
        limits.check_pages(images.len().saturating_add(1))?;
        let page = images.len();
        let (tags, following) = read_ifd(bytes, next, order, page, limits)?;
        images.push(decode_page(bytes, &tags, page, limits)?);
        next = following;
    }
    Ok(images)
}

/// Reads the byte order from the header magic.
fn decode_order(bytes: &[u8]) -> Result<Order, TiffError> {
    let Some(magic) = get_u16_le(bytes, 0) else {
        return crate::legacy::refuse(TiffError::Truncated { offset: 0, need: 2 });
    };
    if magic == TIFF_LE {
        Ok(Order::Little)
    } else if magic == TIFF_BE {
        Ok(Order::Big)
    } else {
        Err(TiffError::BadHeader)
    }
}

/// Reads a `u16` in the file order.
fn read_u16(bytes: &[u8], at: usize, order: Order) -> Option<u16> {
    match order {
        Order::Little => get_u16_le(bytes, at),
        Order::Big => get_u16_be(bytes, at),
    }
}

/// Reads a `u32` in the file order.
fn read_u32(bytes: &[u8], at: usize, order: Order) -> Option<u32> {
    match order {
        Order::Little => get_u32_le(bytes, at),
        Order::Big => get_u32_be(bytes, at),
    }
}

/// The truncation refusal for `need` bytes at `offset`.
const fn truncated(offset: usize, need: usize) -> TiffError {
    TiffError::Truncated {
        offset,
        need: offset.saturating_add(need),
    }
}

/// Reads one IFD: the page tags and the next IFD offset.
fn read_ifd(
    bytes: &[u8],
    at: u32,
    order: Order,
    page: usize,
    limits: &Limits,
) -> Result<(PageTags, u32), TiffError> {
    let Ok(offset) = usize::try_from(at) else {
        return crate::legacy::refuse(truncated(0, 2));
    };
    let Some(count) = read_u16(bytes, offset, order) else {
        return crate::legacy::refuse(truncated(offset, 2));
    };
    limits.check_records(usize::from(count))?;
    let mut tags = PageTags {
        width: 0,
        height: 0,
        bits: 1,
        compression: RAW,
        photometric: 0,
        fill_order: 1,
        rows_per_strip: u32::MAX,
        t4_options: 0,
        t6_options: 0,
        offsets: Vec::new(),
        counts: Vec::new(),
    };
    for index in 0..count {
        let entry_at = offset
            .saturating_add(2)
            .saturating_add(usize::from(index).saturating_mul(IFD_ENTRY_LEN));
        let (Some(tag), Some(kind), Some(total)) = (
            read_u16(bytes, entry_at, order),
            read_u16(bytes, entry_at.saturating_add(2), order),
            read_u32(bytes, entry_at.saturating_add(4), order),
        ) else {
            return crate::legacy::refuse(truncated(entry_at, IFD_ENTRY_LEN));
        };
        if !READ_TAGS.contains(&tag) {
            continue;
        }
        let values = read_entry_values(
            bytes,
            entry_at.saturating_add(8),
            (tag, kind, total),
            order,
            page,
        )?;
        store_tag(&mut tags, tag, values, page)?;
    }
    check_tags(&tags, page)?;
    let next_at = offset
        .saturating_add(2)
        .saturating_add(usize::from(count).saturating_mul(IFD_ENTRY_LEN));
    let Some(next) = read_u32(bytes, next_at, order) else {
        return crate::legacy::refuse(truncated(next_at, 4));
    };
    Ok((tags, next))
}

/// Stores one read tag's values into the page tags.
fn store_tag(
    tags: &mut PageTags,
    tag: u16,
    values: Vec<u32>,
    page: usize,
) -> Result<(), TiffError> {
    match tag {
        TAG_STRIP_OFFSETS => tags.offsets = values,
        TAG_STRIP_COUNTS => tags.counts = values,
        _ => {
            let Some(&first) = values.first() else {
                return crate::legacy::refuse(TiffError::BadTag { page, tag });
            };
            let slot = match tag {
                TAG_WIDTH => &mut tags.width,
                TAG_HEIGHT => &mut tags.height,
                TAG_BPS => &mut tags.bits,
                TAG_COMPRESSION => &mut tags.compression,
                TAG_PHOTOMETRIC => &mut tags.photometric,
                TAG_FILL_ORDER => &mut tags.fill_order,
                TAG_ROWS_PER_STRIP => &mut tags.rows_per_strip,
                TAG_T4_OPTIONS => &mut tags.t4_options,
                _ => &mut tags.t6_options,
            };
            *slot = first;
        }
    }
    Ok(())
}

/// Refuses a page whose tags a bilevel reader cannot honour, naming why.
fn check_tags(tags: &PageTags, page: usize) -> Result<(), TiffError> {
    let unsupported = |what: &str| {
        Err(TiffError::Unsupported {
            page,
            what: String::from(what),
        })
    };
    if tags.width == 0 || tags.height == 0 {
        return crate::legacy::refuse(TiffError::BadTag {
            page,
            tag: TAG_WIDTH,
        });
    }
    if tags.offsets.is_empty() || tags.offsets.len() != tags.counts.len() {
        return crate::legacy::refuse(TiffError::BadTag {
            page,
            tag: TAG_STRIP_COUNTS,
        });
    }
    if tags.bits != 1 {
        return unsupported("bits per sample past 1");
    }
    if tags.photometric > 1 {
        return unsupported("photometric past WhiteIsZero and BlackIsZero");
    }
    if tags.fill_order != 1 && tags.fill_order != 2 {
        return unsupported("fill order past 1 and 2");
    }
    if (tags.t4_options | tags.t6_options) & UNCOMPRESSED_MODE != 0 {
        return unsupported("CCITT uncompressed mode");
    }
    Ok(())
}

/// Reads the values of one IFD entry, given as `(tag, kind, count)`: inline
/// when they fit in four bytes, else at the offset the entry holds.
fn read_entry_values(
    bytes: &[u8],
    at: usize,
    entry: (u16, u16, u32),
    order: Order,
    page: usize,
) -> Result<Vec<u32>, TiffError> {
    let (tag, kind, total) = entry;
    let width = match kind {
        TYPE_SHORT => 2_usize,
        TYPE_LONG => 4_usize,
        _ => return crate::legacy::refuse(TiffError::BadTag { page, tag }),
    };
    let Ok(count) = usize::try_from(total) else {
        return crate::legacy::refuse(TiffError::BadTag { page, tag });
    };
    let span = count.saturating_mul(width);
    let start = if span <= 4 {
        at
    } else {
        let Some(offset) = read_u32(bytes, at, order) else {
            return crate::legacy::refuse(truncated(at, 4));
        };
        let Ok(start) = usize::try_from(offset) else {
            return crate::legacy::refuse(truncated(at, 4));
        };
        start
    };
    let Some(window) = bytes.get(start..start.saturating_add(span)) else {
        return crate::legacy::refuse(truncated(start, span));
    };
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let value_at = index.saturating_mul(width);
        let value = if kind == TYPE_SHORT {
            read_u16(window, value_at, order).map(u32::from)
        } else {
            read_u32(window, value_at, order)
        };
        let Some(value) = value else {
            return crate::legacy::refuse(truncated(start.saturating_add(value_at), width));
        };
        out.push(value);
    }
    Ok(out)
}

/// Decodes one page from its strips.
fn decode_page(
    bytes: &[u8],
    tags: &PageTags,
    page: usize,
    limits: &Limits,
) -> Result<TiffImage, TiffError> {
    let compression = match tags.compression {
        RAW => Compression::Raw,
        G3 => Compression::G3,
        G4 => Compression::G4,
        _ => {
            return crate::legacy::refuse(TiffError::Unsupported {
                page,
                what: String::from("compression past raw, G3 and G4"),
            });
        }
    };
    let (Ok(width), Ok(height)) = (usize::try_from(tags.width), usize::try_from(tags.height))
    else {
        return crate::legacy::refuse(TiffError::BadTag {
            page,
            tag: TAG_WIDTH,
        });
    };
    limits.check_records(height)?;
    let row_bytes = width.div_ceil(8);
    limits.check_bytes(height.saturating_mul(row_bytes))?;
    let mut bitmap = vec![0_u8; height.saturating_mul(row_bytes)];
    let rows_per_strip = match usize::try_from(tags.rows_per_strip) {
        Ok(drawn) => drawn.max(1),
        Err(_) => height,
    };
    let mut row: usize = 0;
    for (&offset, &count) in tags.offsets.iter().zip(tags.counts.iter()) {
        let (Ok(start), Ok(len)) = (usize::try_from(offset), usize::try_from(count)) else {
            return crate::legacy::refuse(truncated(0, 1));
        };
        let Some(stored) = bytes.get(start..start.saturating_add(len)) else {
            return crate::legacy::refuse(truncated(start, len));
        };
        let reversed: Vec<u8>;
        let data = if tags.fill_order == 2 {
            reversed = stored.iter().map(|byte| byte.reverse_bits()).collect();
            reversed.as_slice()
        } else {
            stored
        };
        let strip_rows = rows_per_strip.min(height.saturating_sub(row));
        let span = row..row.saturating_add(strip_rows);
        if compression == Compression::Raw {
            if copy_raw(data, &mut bitmap, row_bytes, row, strip_rows).is_none() {
                return crate::legacy::refuse(TiffError::CorruptStrip { page, row });
            }
        } else {
            let mut strip = Strip::new(&mut bitmap, (row_bytes, width), span, page);
            if compression == Compression::G3 {
                t4::decode_g3_strip(data, &mut strip, tags.t4_options & T4_TWO_D != 0, limits)?;
            } else {
                t6::decode_g4_strip(data, &mut strip, limits)?;
            }
        }
        row = row.saturating_add(strip_rows);
    }
    if row != height {
        return crate::legacy::refuse(TiffError::CorruptStrip { page, row });
    }
    if tags.photometric == 1 {
        for byte in bitmap.iter_mut() {
            *byte = !*byte;
        }
    }
    clear_padding(&mut bitmap, row_bytes, width);
    TiffImage::from_bitmap(tags.width, tags.height, &bitmap)
}

/// The bits a row's last byte keeps, by how many pixels it holds (zero
/// meaning a full byte).
const KEEP_MASK: [u8; 8] = [0xff, 0x80, 0xc0, 0xe0, 0xf0, 0xf8, 0xfc, 0xfe];

/// Zeroes the bits past `width` in each row's last byte, so the bitmap is
/// canonical whatever the file stored there.
pub(super) fn clear_padding(bitmap: &mut [u8], row_bytes: usize, width: usize) {
    let Some(&keep) = KEEP_MASK.get(width.rem_euclid(8)) else {
        return;
    };
    if row_bytes == 0 {
        return;
    }
    for chunk in bitmap.chunks_mut(row_bytes) {
        if let Some(last) = chunk.last_mut() {
            *last &= keep;
        }
    }
}

/// Copies a raw strip's rows into the bitmap, or `None` when the strip is
/// short or the rows run past the page.
fn copy_raw(
    data: &[u8],
    bitmap: &mut [u8],
    row_bytes: usize,
    row: usize,
    rows: usize,
) -> Option<()> {
    let need = rows.saturating_mul(row_bytes);
    let window = data.get(..need)?;
    let at = row.saturating_mul(row_bytes);
    bitmap
        .get_mut(at..at.saturating_add(need))?
        .copy_from_slice(window);
    Some(())
}

/// Encodes images into one TIFF: headers, IFDs, strips and data.
///
/// # Errors
///
/// [`TiffError`] for an oversize shape, or [`TiffError::Limit`] past a ceiling.
pub(crate) fn encode_tiff(
    images: &[TiffImage],
    compression: Compression,
    limits: &Limits,
) -> Result<Vec<u8>, TiffError> {
    limits.check_pages(images.len())?;
    let mut out: Vec<u8> = Vec::new();
    push_u16_le(&mut out, TIFF_LE);
    push_u16_le(&mut out, TIFF_VERSION);
    let mut link_slot = out.len();
    push_u32_le(&mut out, 0);
    for (page, image) in images.iter().enumerate() {
        if !out.len().is_multiple_of(2) {
            out.push(0);
        }
        let ifd_at = narrow_offset(out.len(), page)?;
        write_back_u32(&mut out, link_slot, ifd_at)?;
        let (offset_slot, count_slot, next_slot) = write_ifd_shell(&mut out, image, compression);
        let data = encode_strip(image, compression, page, limits)?;
        let offset = narrow_offset(out.len(), page)?;
        let count = narrow_offset(data.len(), page)?;
        out.extend_from_slice(&data);
        write_back_u32(&mut out, offset_slot, offset)?;
        write_back_u32(&mut out, count_slot, count)?;
        link_slot = next_slot;
    }
    Ok(out)
}

/// Narrows a file offset or length into the `u32` an IFD entry holds.
fn narrow_offset(offset: usize, page: usize) -> Result<u32, TiffError> {
    match u32::try_from(offset) {
        Ok(drawn) => Ok(drawn),
        Err(_) => Err(TiffError::LengthMismatch {
            page,
            need: offset,
            have: usize::MAX,
        }),
    }
}

/// Writes one IFD shell, its entries in ascending tag order, returning the
/// strip offset, byte-count and next-IFD slots for the writer to fill in.
fn write_ifd_shell(
    out: &mut Vec<u8>,
    image: &TiffImage,
    compression: Compression,
) -> (usize, usize, usize) {
    push_u16_le(out, 8);
    let compression_value = match compression {
        Compression::Raw => RAW,
        Compression::G3 => G3,
        Compression::G4 => G4,
    };
    write_entry(out, (TAG_WIDTH, TYPE_LONG), image.width());
    write_entry(out, (TAG_HEIGHT, TYPE_LONG), image.height());
    write_entry(out, (TAG_BPS, TYPE_SHORT), 1);
    write_entry(out, (TAG_COMPRESSION, TYPE_SHORT), compression_value);
    write_entry(out, (TAG_PHOTOMETRIC, TYPE_SHORT), 0);
    let offset_slot = write_entry(out, (TAG_STRIP_OFFSETS, TYPE_LONG), 0);
    write_entry(out, (TAG_ROWS_PER_STRIP, TYPE_LONG), image.height());
    let count_slot = write_entry(out, (TAG_STRIP_COUNTS, TYPE_LONG), 0);
    let next_slot = out.len();
    push_u32_le(out, 0);
    (offset_slot, count_slot, next_slot)
}

/// Writes one single-value IFD entry, returning the offset of its value so
/// a placeholder can be filled in later. A SHORT sits in the low half of
/// the little-endian value field.
fn write_entry(out: &mut Vec<u8>, field: (u16, u16), value: u32) -> usize {
    let (tag, kind) = field;
    push_u16_le(out, tag);
    push_u16_le(out, kind);
    push_u32_le(out, 1);
    let slot = out.len();
    push_u32_le(out, value);
    slot
}

/// Writes one page's strip with the page compression: every row, as one
/// continuous coded stream for G3 and G4.
fn encode_strip(
    image: &TiffImage,
    compression: Compression,
    page: usize,
    limits: &Limits,
) -> Result<Vec<u8>, TiffError> {
    let (Ok(width), Ok(height)) = (
        usize::try_from(image.width()),
        usize::try_from(image.height()),
    ) else {
        return crate::legacy::refuse(TiffError::LengthMismatch {
            page,
            need: 1,
            have: 0,
        });
    };
    limits.check_bytes(image.bitmap().len())?;
    let mut rows: Vec<&[u8]> = Vec::with_capacity(height);
    for y in 0..image.height() {
        let Some(row) = image.row(y) else {
            return crate::legacy::refuse(TiffError::CorruptStrip {
                page,
                row: rows.len(),
            });
        };
        rows.push(row);
    }
    match compression {
        Compression::Raw => Ok(image.bitmap().to_vec()),
        Compression::G3 => t4::encode_g3(&rows, width, page),
        Compression::G4 => t6::encode_g4(&rows, width, page),
    }
}

/// Writes a final `u32` back into a placeholder slot.
fn write_back_u32(out: &mut [u8], at: usize, value: u32) -> Result<(), TiffError> {
    let Some(window) = out.get_mut(at..at.saturating_add(4)) else {
        return crate::legacy::refuse(truncated(at, 4));
    };
    window.copy_from_slice(&value.to_le_bytes());
    Ok(())
}
