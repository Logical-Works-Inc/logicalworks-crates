//! The compound file header: magic, version and the first chain heads.
//!
//! [`CfbFile::open`] validates the 512-byte header per MS-CFB §2.2 (magic,
//! major version 3 or 4, sector shift, directory and FAT heads, mini cutoff
//! and DIFAT), sizes the FAT, then hands the chains to [`super::fat`], the entries
//! to [`super::directory`] and the small streams to [`super::stream`].

use super::super::limits::Limits;
use super::{CfbError, CfbFile, ENDOFCHAIN, FREESECT, HEADER_LEN, MAGIC};
use crate::legacy::{get_u16_le, get_u32_le};

/// Alias for the error this header reports, so the arms below read alike.
type HeaderResult = Result<(), CfbError>;

impl CfbFile {
    /// Opens and validates a compound file image.
    ///
    /// # Errors
    ///
    /// [`CfbError`] for a bad magic, version, truncation, cycle, bad link or
    /// encrypted file, or [`CfbError::Limit`] past a ceiling.
    pub fn open(bytes: &[u8], limits: &Limits) -> Result<Self, CfbError> {
        limits.check_bytes(bytes.len())?;
        if bytes.len() < HEADER_LEN {
            let refusal = Err(CfbError::Truncated {
                offset: bytes.len(),
                need: HEADER_LEN,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("cfb: refusing a file shorter than its header");
            return refusal;
        }
        let Some(head) = bytes.get(..HEADER_LEN) else {
            let refusal = Err(CfbError::Truncated {
                offset: bytes.len(),
                need: HEADER_LEN,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("cfb: refusing a file with no header window");
            return refusal;
        };
        check_magic(head)?;
        let version = header_u16(head, 26)?;
        if version != 3 && version != 4 {
            let refusal = Err(CfbError::UnknownVersion { version });
            #[cfg(feature = "trace")]
            crate::trace::debug!(version, "cfb: refusing an unknown version");
            return refusal;
        }
        let sector_shift = header_u16(head, 30)?;
        let sector_len = match sector_shift {
            9 => 512_usize,
            12 => 4096_usize,
            _ => {
                let refusal = Err(CfbError::UnknownVersion {
                    version: sector_shift,
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(sector_shift, "cfb: refusing an unknown sector shift");
                return refusal;
            }
        };
        let first_dir = header_u32(head, 48)?;
        let cutoff = header_u32(head, 56)?;
        let first_mini_fat = header_u32(head, 60)?;
        let mini_fat_sectors = header_u32(head, 64)?;
        let first_difat = header_u32(head, 68)?;
        let difat_sectors = header_u32(head, 72)?;
        let mut fat_runs: Vec<u32> = Vec::new();
        for index in 0..109_usize {
            let at = 76_usize.saturating_add(index.saturating_mul(4));
            let Some(sector) = get_u32_le(head, at) else {
                let refusal = Err(CfbError::Truncated {
                    offset: at,
                    need: at.saturating_add(4),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!("cfb: refusing a header with no DIFAT entry");
                return refusal;
            };
            if sector == FREESECT {
                break;
            }
            fat_runs.push(sector);
        }
        if difat_sectors > 0 {
            read_difat_chain(bytes, first_difat, difat_sectors, sector_len, &mut fat_runs)?;
        }
        let sector_count = bytes.len().saturating_sub(HEADER_LEN).div_ceil(sector_len);
        limits.check_pages(sector_count)?;
        let mut fat: Vec<u32> = vec![FREESECT; sector_count];
        for run in fat_runs.iter() {
            let bytes_at = super::fat::sector_offset(*run, sector_len, bytes.len())?;
            for index in 0..sector_len.saturating_div(4) {
                let at = bytes_at.saturating_add(index.saturating_mul(4));
                let Some(next) = get_u32_le(bytes, at) else {
                    break;
                };
                // The FAT slot this entry fills, from the offset `sector_offset`
                // already bounded to the file: one sector holds a quarter of
                // its length in entries.
                let Some(sector) = bytes_at.saturating_sub(HEADER_LEN).checked_div(sector_len)
                else {
                    return crate::legacy::refuse(CfbError::BadFat { sector: *run });
                };
                let position = sector
                    .saturating_mul(sector_len.saturating_div(4))
                    .saturating_add(index);
                if let Some(slot) = fat.get_mut(position) {
                    *slot = next;
                }
            }
        }
        let mut file = Self {
            sector_len,
            fat,
            entries: Vec::new(),
            image: bytes.to_vec(),
            cutoff,
            mini_fat: Vec::new(),
            mini_stream: Vec::new(),
        };
        file.read_directory(first_dir, limits)?;
        file.read_mini_structures(first_mini_fat, mini_fat_sectors, limits)?;
        if file
            .entries
            .iter()
            .any(|entry| entry.name == "EncryptionInfo")
        {
            let refusal = Err(CfbError::Encrypted);
            #[cfg(feature = "trace")]
            crate::trace::debug!("cfb: refusing an encrypted file");
            return refusal;
        }
        Ok(file)
    }
}

/// Refuses a header whose first eight bytes are not the magic.
fn check_magic(head: &[u8]) -> HeaderResult {
    for (index, byte) in MAGIC.iter().enumerate() {
        let Some(drawn) = head.get(index) else {
            let refusal = Err(CfbError::Truncated {
                offset: index,
                need: HEADER_LEN,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("cfb: refusing a header with no magic window");
            return refusal;
        };
        if drawn != byte {
            let refusal = Err(CfbError::BadMagic);
            #[cfg(feature = "trace")]
            crate::trace::debug!(index, "cfb: refusing a magic mismatch");
            return refusal;
        }
    }
    Ok(())
}

/// Reads one little-endian `u16` out of the header, naming its offset.
fn header_u16(head: &[u8], at: usize) -> Result<u16, CfbError> {
    let Some(value) = get_u16_le(head, at) else {
        let refusal = Err(CfbError::Truncated {
            offset: at,
            need: at.saturating_add(2),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!(at, "cfb: refusing a header past its field");
        return refusal;
    };
    Ok(value)
}

/// Reads one little-endian `u32` out of the header, naming its offset.
fn header_u32(head: &[u8], at: usize) -> Result<u32, CfbError> {
    let Some(value) = get_u32_le(head, at) else {
        let refusal = Err(CfbError::Truncated {
            offset: at,
            need: at.saturating_add(4),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!(at, "cfb: refusing a header past its field");
        return refusal;
    };
    Ok(value)
}

/// Follows the DIFAT chain past the header's 109 entries, collecting FAT runs.
fn read_difat_chain(
    bytes: &[u8],
    first: u32,
    sectors: u32,
    sector_len: usize,
    runs: &mut Vec<u32>,
) -> HeaderResult {
    let mut at_sector = first;
    for _ in 0..sectors {
        let bytes_at = super::fat::sector_offset(at_sector, sector_len, bytes.len())?;
        for index in 0..sector_len.saturating_div(4).saturating_sub(1) {
            let at = bytes_at.saturating_add(index.saturating_mul(4));
            let Some(sector) = get_u32_le(bytes, at) else {
                let refusal = Err(CfbError::Truncated {
                    offset: at,
                    need: at.saturating_add(4),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!("cfb: refusing a DIFAT past the file");
                return refusal;
            };
            if sector == FREESECT {
                break;
            }
            runs.push(sector);
        }
        let link_at = bytes_at.saturating_add(sector_len.saturating_sub(4));
        let Some(next) = get_u32_le(bytes, link_at) else {
            let refusal = Err(CfbError::Truncated {
                offset: link_at,
                need: link_at.saturating_add(4),
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("cfb: refusing a DIFAT chain past the file");
            return refusal;
        };
        if next == ENDOFCHAIN || next == FREESECT {
            break;
        }
        at_sector = next;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::CfbFile;
    use crate::legacy::limits::Limits;

    /// The bounds the unit tests run under.
    const BOUNDS: Limits = Limits::new(1_048_576, 1024, 4096, 8, 4_194_304);

    #[test]
    fn a_bad_magic_is_refused() {
        let image = vec![0_u8; 512];
        assert!(
            matches!(
                CfbFile::open(&image, &BOUNDS),
                Err(super::CfbError::BadMagic)
            ),
            "a zeroed header must be refused on its magic"
        );
    }

    #[test]
    fn an_unknown_version_names_itself() {
        let mut image = vec![0_u8; 512];
        for (index, byte) in super::super::MAGIC.iter().enumerate() {
            if let Some(slot) = image.get_mut(index) {
                *slot = *byte;
            }
        }
        assert!(
            CfbFile::open(&image, &BOUNDS).is_err(),
            "a zeroed version must be refused"
        );
    }
}
