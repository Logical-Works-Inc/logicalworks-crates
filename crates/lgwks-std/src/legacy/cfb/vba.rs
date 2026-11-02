//! VBA module-source (de)compression, MS-OVBA §2.4.1.
//!
//! A container starts with the `0x01` signature byte, then holds chunks:
//! a 2-byte header whose high bit marks compression and whose low 12 bits
//! hold the data size minus three, then either 4 KiB of raw bytes or flag
//! bytes over literal bytes and 2-byte copy tokens (low 4 bits: length
//! minus 3; high 12 bits: offset back minus 1) per MS-OVBA §2.4.1.3.4-5.
//! [`decompress`] reads containers; [`compress`] writes them for
//! round-trip tests, capping each chunk so its size field always holds.

use std::fmt;

use crate::legacy::limits::{LimitError, Limits};

/// The container signature byte every compressed VBA module starts with.
const SIGNATURE: u8 = 0x01;

/// The flag bit marking a compressed chunk in its 2-byte header.
const COMPRESSED_FLAG: u16 = 0x8000;

/// The mask holding a compressed chunk's data size minus three.
const SIZE_MASK: u16 = 0x0fff;

/// The bias between a stored chunk size and its real data size.
const SIZE_BIAS: usize = 3;

/// The bias between a stored copy length and its real length.
const LENGTH_BIAS: usize = 3;

/// The bias between a stored copy offset and its real distance back.
const OFFSET_BIAS: usize = 1;

/// The widest offset a copy token reaches back.
const MAX_OFFSET: usize = 4096;

/// Decompressed bytes one chunk holds at most.
const CHUNK_OUTPUT: usize = 4096;

/// Coded bytes one chunk holds at most: the 12-bit size field plus its bias.
const CHUNK_CODED: usize = 4098;

/// The longest match a copy token spans.
const MAX_MATCH: usize = 18;

/// Why a VBA container could not be decompressed or compressed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum VbaError {
    /// The container does not start with the `0x01` signature.
    BadSignature {
        /// The observed first byte, when the container holds one.
        byte: Option<u8>,
    },
    /// The container ends inside a header, token or literal.
    Truncated {
        /// Byte offset where the container ended.
        offset: usize,
        /// Bytes the structure needed.
        need: usize,
    },
    /// A copy token reaches before the window start.
    BadOffset {
        /// Byte offset of the offending token in the output.
        at: usize,
        /// The requested distance back.
        offset: usize,
    },
    /// The input or output passed a [`Limits`] ceiling.
    Limit(LimitError),
}

impl fmt::Display for VbaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::BadSignature { byte } => match byte {
                Some(drawn) => write!(
                    formatter,
                    "VBA container signature {drawn:#04x} is not 0x01"
                ),
                None => formatter.write_str("VBA container is empty"),
            },
            Self::Truncated { offset, need } => write!(
                formatter,
                "VBA container ends at {offset} short of {need} bytes"
            ),
            Self::BadOffset { at, offset } => write!(
                formatter,
                "VBA copy at output {at} reaches {offset} back past the window"
            ),
            Self::Limit(ref cause) => {
                write!(formatter, "VBA limits refused the input: {cause}")
            }
        }
    }
}

crate::legacy::limits::limit_carrying_error!(VbaError);

/// Reads one little-endian `u16` out of a chunk window that starts at
/// container offset `at`, which a truncation names.
fn chunk_u16(window: &[u8], at: usize) -> Result<u16, VbaError> {
    let (Some(low), Some(high)) = (window.first(), window.get(1)) else {
        let refusal = Err(VbaError::Truncated {
            offset: at,
            need: at.saturating_add(2),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("vba: refusing a two-byte read past a window");
        return refusal;
    };
    Ok(u16::from(*low).saturating_add(u16::from(*high).saturating_mul(256)))
}

/// Decompresses a VBA module container into its source bytes.
///
/// # Errors
///
/// [`VbaError`] for a bad signature, truncation or a copy past the
/// window, or [`VbaError::Limit`] past a ceiling.
pub fn decompress(data: &[u8], limits: &Limits) -> Result<Vec<u8>, VbaError> {
    limits.check_bytes(data.len())?;
    let Some((&signature, rest)) = data.split_first() else {
        let refusal = Err(VbaError::BadSignature { byte: None });
        #[cfg(feature = "trace")]
        crate::trace::debug!("vba: refusing an empty container");
        return refusal;
    };
    if signature != SIGNATURE {
        let refusal = Err(VbaError::BadSignature {
            byte: Some(signature),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!(signature, "vba: refusing a bad signature");
        return refusal;
    }
    let mut out: Vec<u8> = Vec::new();
    let mut cursor: usize = 0;
    while cursor < rest.len() {
        let Some(window) = rest.get(cursor..cursor.saturating_add(2)) else {
            let refusal = Err(VbaError::Truncated {
                offset: cursor,
                need: cursor.saturating_add(2),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!(cursor, "vba: refusing a chunk past the header");
            return refusal;
        };
        let header = chunk_u16(window, cursor)?;
        cursor = cursor.saturating_add(2);
        if header & COMPRESSED_FLAG == 0 {
            let Some(raw) = rest.get(cursor..cursor.saturating_add(CHUNK_OUTPUT)) else {
                let refusal = Err(VbaError::Truncated {
                    offset: cursor,
                    need: cursor.saturating_add(CHUNK_OUTPUT),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(cursor, "vba: refusing a raw chunk past the input");
                return refusal;
            };
            limits.check_decompressed(out.len().saturating_add(raw.len()))?;
            out.extend_from_slice(raw);
            cursor = cursor.saturating_add(CHUNK_OUTPUT);
        } else {
            let data_len = usize::from(header & SIZE_MASK).saturating_add(SIZE_BIAS);
            let Some(chunk) = rest.get(cursor..cursor.saturating_add(data_len)) else {
                let refusal = Err(VbaError::Truncated {
                    offset: cursor,
                    need: cursor.saturating_add(data_len),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(cursor, "vba: refusing a chunk past the input");
                return refusal;
            };
            decompress_chunk(chunk, &mut out, limits)?;
            cursor = cursor.saturating_add(data_len);
        }
    }
    Ok(out)
}

/// Decompresses one compressed chunk: flag bytes over literal bytes and
/// 12-bit-offset copy tokens, per MS-OVBA §2.4.1.3.4-5.
fn decompress_chunk(chunk: &[u8], out: &mut Vec<u8>, limits: &Limits) -> Result<(), VbaError> {
    let mut cursor: usize = 0;
    while cursor < chunk.len() {
        let Some(&flags) = chunk.get(cursor) else {
            break;
        };
        cursor = cursor.saturating_add(1);
        for bit in 0..8_u32 {
            if cursor >= chunk.len() {
                break;
            }
            if (flags & flag_bit(bit)) == 0 {
                let Some(&byte) = chunk.get(cursor) else {
                    let refusal = Err(VbaError::Truncated {
                        offset: cursor,
                        need: cursor.saturating_add(1),
                    });
                    #[cfg(feature = "trace")]
                    crate::trace::debug!(cursor, "vba: refusing a literal past the chunk");
                    return refusal;
                };
                cursor = cursor.saturating_add(1);
                limits.check_decompressed(out.len().saturating_add(1))?;
                out.push(byte);
            } else {
                let Some(window) = chunk.get(cursor..cursor.saturating_add(2)) else {
                    let refusal = Err(VbaError::Truncated {
                        offset: cursor,
                        need: cursor.saturating_add(2),
                    });
                    #[cfg(feature = "trace")]
                    crate::trace::debug!(cursor, "vba: refusing a token past the chunk");
                    return refusal;
                };
                cursor = cursor.saturating_add(2);
                let token = chunk_u16(window, cursor.saturating_sub(2))?;
                let length = usize::from(token.rem_euclid(16)).saturating_add(LENGTH_BIAS);
                let offset = usize::from(token.saturating_div(16)).saturating_add(OFFSET_BIAS);
                if offset == 0 || offset > out.len() || offset > MAX_OFFSET {
                    let refusal = Err(VbaError::BadOffset {
                        at: out.len(),
                        offset,
                    });
                    #[cfg(feature = "trace")]
                    crate::trace::debug!(offset, "vba: refusing a copy past the window");
                    return refusal;
                }
                limits.check_decompressed(out.len().saturating_add(length))?;
                for _ in 0..length {
                    let from = out.len().saturating_sub(offset);
                    let Some(&byte) = out.get(from) else {
                        let refusal = Err(VbaError::BadOffset {
                            at: out.len(),
                            offset,
                        });
                        #[cfg(feature = "trace")]
                        crate::trace::debug!("vba: refusing a copy past the output");
                        return refusal;
                    };
                    out.push(byte);
                }
            }
        }
    }
    Ok(())
}

/// One flag bit: bit `position` of a flag byte.
fn flag_bit(position: u32) -> u8 {
    1_u8 << position
}

/// Compresses source bytes into a VBA container, for round-trip tests.
///
/// Greedy matches of three or more bytes in the 4 KiB window become copy
/// tokens; every other byte is a literal. Chunks are capped so the 12-bit
/// size field always holds, and the output decompresses to the input under
/// [`decompress`].
///
/// # Errors
///
/// [`VbaError::Limit`] past a ceiling.
pub fn compress(data: &[u8], limits: &Limits) -> Result<Vec<u8>, VbaError> {
    limits.check_bytes(data.len())?;
    let mut out: Vec<u8> = Vec::new();
    out.push(SIGNATURE);
    let mut cursor: usize = 0;
    while cursor < data.len() {
        let consumed = code_chunk(data, cursor, &mut out, limits)?;
        cursor = cursor.saturating_add(consumed);
    }
    limits.check_bytes(out.len())?;
    Ok(out)
}

/// Codes one chunk from `start`: literals and copy tokens until the coded
/// size or the 4 KiB output cap would overflow, returning bytes consumed.
fn code_chunk(
    data: &[u8],
    start: usize,
    out: &mut Vec<u8>,
    limits: &Limits,
) -> Result<usize, VbaError> {
    let mut coded: Vec<u8> = Vec::new();
    let mut group: Vec<u8> = Vec::new();
    let mut flags: u8 = 0;
    let mut flag_count: u32 = 0;
    let mut decoded: usize = 0;
    let mut at = start;
    while at < data.len() && decoded < CHUNK_OUTPUT {
        let Some(window) = data.get(at..) else {
            break;
        };
        let (length, offset) = longest_match(data, at, start);
        let unit_coded = if length >= LENGTH_BIAS {
            2_usize
        } else {
            1_usize
        };
        if coded
            .len()
            .saturating_add(group.len())
            .saturating_add(unit_coded)
            .saturating_add(1)
            > CHUNK_CODED
            || decoded.saturating_add(length.max(1)) > CHUNK_OUTPUT
        {
            break;
        }
        if length >= LENGTH_BIAS {
            let token = offset
                .saturating_sub(OFFSET_BIAS)
                .saturating_mul(16)
                .saturating_add(length.saturating_sub(LENGTH_BIAS));
            let Ok(narrow) = u16::try_from(token) else {
                let refusal = Err(VbaError::Truncated {
                    offset: at,
                    need: 2,
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!("vba: refusing a token past 16 bits");
                return refusal;
            };
            flags |= flag_bit(flag_count);
            let bytes = narrow.to_le_bytes();
            let (Some(low), Some(high)) = (bytes.first(), bytes.get(1)) else {
                let refusal = Err(VbaError::Truncated {
                    offset: at,
                    need: 2,
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!("vba: refusing a token past its bytes");
                return refusal;
            };
            group.push(*low);
            group.push(*high);
            at = at.saturating_add(length);
            decoded = decoded.saturating_add(length);
        } else {
            let Some(&byte) = window.first() else {
                break;
            };
            group.push(byte);
            at = at.saturating_add(1);
            decoded = decoded.saturating_add(1);
        }
        flag_count = flag_count.saturating_add(1);
        if flag_count >= 8 {
            coded.push(flags);
            coded.extend_from_slice(&group);
            flags = 0;
            flag_count = 0;
            group.clear();
        }
    }
    if flag_count > 0 {
        coded.push(flags);
        coded.extend_from_slice(&group);
    }
    if coded.is_empty() || at == start {
        let refusal = Err(VbaError::Truncated {
            offset: at,
            need: 1,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("vba: refusing an empty chunk");
        return refusal;
    }
    let header = COMPRESSED_FLAG.saturating_add(header_size(coded.len())?);
    let bytes = header.to_le_bytes();
    let (Some(low), Some(high)) = (bytes.first(), bytes.get(1)) else {
        let refusal = Err(VbaError::Truncated {
            offset: at,
            need: 2,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("vba: refusing a header past its bytes");
        return refusal;
    };
    out.push(*low);
    out.push(*high);
    limits.check_bytes(out.len().saturating_add(coded.len()))?;
    out.extend_from_slice(&coded);
    Ok(at.saturating_sub(start))
}

/// The stored size field for `coded` bytes: length minus the bias, masked.
fn header_size(coded: usize) -> Result<u16, VbaError> {
    let Ok(drawn) = u16::try_from(coded.saturating_sub(SIZE_BIAS)) else {
        let refusal = Err(VbaError::Truncated {
            offset: coded,
            need: coded,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("vba: refusing a chunk size past 16 bits");
        return refusal;
    };
    Ok(drawn & SIZE_MASK)
}

/// The longest window match at `at`: length capped to fit a token, offset
/// back within the window, or zero length for a literal.
fn longest_match(data: &[u8], at: usize, start: usize) -> (usize, usize) {
    let mut best: (usize, usize) = (0, 0);
    let earliest = at.saturating_sub(MAX_OFFSET).max(start);
    let mut candidate = earliest;
    while candidate < at {
        let mut length: usize = 0;
        while length < MAX_MATCH {
            let left = data.get(candidate.saturating_add(length));
            let right = data.get(at.saturating_add(length));
            match (left, right) {
                (Some(first), Some(second)) if first == second => {
                    length = length.saturating_add(1);
                }
                _ => break,
            }
        }
        if length > best.0 {
            best = (length, at.saturating_sub(candidate));
        }
        candidate = candidate.saturating_add(1);
    }
    best
}

#[cfg(test)]
mod tests {
    use super::{compress, decompress};
    use crate::legacy::limits::Limits;

    /// The bounds the unit tests run under.
    const BOUNDS: Limits = Limits::new(1_048_576, 1024, 4096, 8, 4_194_304);

    /// What every fallible unit test returns.
    type UnitResult = Result<(), super::VbaError>;

    #[test]
    fn vba_source_round_trips() -> UnitResult {
        let source = b"Sub Hello()\r\n    MsgBox \"Hi\"\r\nEnd Sub\r\n";
        let coded = compress(source, &BOUNDS)?;
        assert_eq!(
            decompress(&coded, &BOUNDS)?,
            source,
            "compressed VBA source must decompress to itself"
        );
        Ok(())
    }

    #[test]
    fn vba_rejects_a_bad_signature() {
        assert!(
            matches!(
                decompress(&[0x02, 0x00, 0x00], &BOUNDS),
                Err(super::VbaError::BadSignature { .. })
            ),
            "a container without the 0x01 signature must be refused"
        );
    }

    #[test]
    fn incompressible_source_still_round_trips() -> UnitResult {
        use crate::seeded::Seeded;
        let mut source = vec![0_u8; 6000];
        let mut stream = Seeded::from_seed(0x5eed_0344);
        for slot in source.iter_mut() {
            let wide = stream.next_u64().to_le_bytes();
            let Some(&drawn) = wide.last() else {
                break;
            };
            *slot = drawn;
        }
        let coded = compress(&source, &BOUNDS)?;
        assert_eq!(
            decompress(&coded, &BOUNDS)?,
            source,
            "incompressible source must still round-trip"
        );
        Ok(())
    }
}
