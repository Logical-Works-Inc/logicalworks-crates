//! `legacy` reads and writes the formats COBOL-era estates run on.
//!
//! EBCDIC text, packed and zoned decimal, COBOL copybooks, OLE compound
//! files with VBA sources, Jet/ACE databases and bilevel TIFF fax images:
//! each codec decodes and encodes, so `decode` then `encode` returns the
//! same bytes, every reader takes a `limits::Limits` and refuses past it
//! before allocating, and every failure is a typed error naming its offset.
//! Pure Rust, no third-party dependency; the only crate edge is the optional
//! `trace` debug log, compiled out when that feature is off.
//!
//! The supported one-call path is through `lgwks_bot::script!`: a flow line
//! names a codec function directly, as the `lgwks_bot` legacy-script test
//! shows.

/// Jet (`.mdb`) and ACE (`.accdb`) database pages, catalogues and rows.
pub mod access;
/// OLE compound files (MS-CFB) with VBA module decompression (MS-OVBA).
pub mod cfb;
/// COBOL copybook record layouts with fixed-width decode and encode.
pub mod copybook;
/// COBOL packed (`COMP-3`) and zoned decimal as exact decimals.
pub mod decimal;
/// EBCDIC code pages 037, 500, 1047 and 273.
pub mod ebcdic;
/// TIFF bilevel images with CCITT T.4 (G3) and T.6 (G4) compression.
pub mod fax;
/// Shared bounds every legacy reader enforces before allocating.
pub mod limits;

/// Refuses with `error`: logs it at debug level under `trace`, so every
/// codec refusal is a signal as well as a value, then returns it.
///
/// Cold and never inlined: a refusal ends a decode, so its logging is kept
/// out of the per-code loops that call this on every corrupt branch.
#[cold]
#[inline(never)]
pub(crate) fn refuse<T, E: core::fmt::Display>(error: E) -> Result<T, E> {
    #[cfg(feature = "trace")]
    crate::trace::debug!(%error, "legacy codec: refusing the input");
    Err(error)
}

/// Reads a little-endian `u16` at `offset`, or `None` past the end.
///
/// Bounds-checked through [`slice::get`], so a short input is `None` rather
/// than a panic; every legacy header reader builds on this.
pub(crate) fn get_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    let window = bytes.get(offset..offset.saturating_add(2))?;
    let (&low, rest) = window.split_first()?;
    let (&high, _) = rest.split_first()?;
    Some(u16::from(low).saturating_add(u16::from(high).saturating_mul(256)))
}

/// Reads a little-endian `u32` at `offset`, or `None` past the end.
///
/// Four byte reads through [`slice::get`], folded without a shift that can
/// overflow: each place value multiplies before it adds.
pub(crate) fn get_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let window = bytes.get(offset..offset.saturating_add(4))?;
    let mut value: u32 = 0;
    let mut place: u32 = 1;
    for byte in window.iter() {
        value = value.saturating_add(u32::from(*byte).saturating_mul(place));
        place = place.saturating_mul(256);
    }
    Some(value)
}

/// Reads a big-endian `u16` at `offset`, or `None` past the end.
///
/// The TIFF and binary-numeric twin of [`get_u16_le`].
pub(crate) fn get_u16_be(bytes: &[u8], offset: usize) -> Option<u16> {
    let window = bytes.get(offset..offset.saturating_add(2))?;
    let (&high, rest) = window.split_first()?;
    let (&low, _) = rest.split_first()?;
    Some(
        u16::from(high)
            .saturating_mul(256)
            .saturating_add(u16::from(low)),
    )
}

/// Reads a big-endian `u32` at `offset`, or `None` past the end.
///
/// The TIFF and binary-numeric twin of [`get_u32_le`].
pub(crate) fn get_u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
    let window = bytes.get(offset..offset.saturating_add(4))?;
    let mut value: u32 = 0;
    for byte in window.iter() {
        value = value.saturating_mul(256).saturating_add(u32::from(*byte));
    }
    Some(value)
}

/// Appends a little-endian `u16` to `out`.
///
/// The writer twin of [`get_u16_le`], used by the encoders that lay headers
/// down byte by byte. The conversion is total: the value's own bytes.
pub(crate) fn push_u16_le(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Appends a little-endian `u32` to `out`.
///
/// The writer twin of [`get_u32_le`]. The conversion is total: the value's
/// own bytes.
pub(crate) fn push_u32_le(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
