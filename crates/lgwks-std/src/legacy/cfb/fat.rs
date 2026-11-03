//! Sector chains: following, cycle detection and byte offsets.
//!
//! [`CfbFile::follow_chain`] walks one FAT chain per MS-CFB §2.3, refusing a
//! revisit, a jump past the file and a chain past the page ceiling; every
//! step is bounded by [`Limits`], so a cyclic image cannot loop forever.

use super::super::limits::Limits;
use super::{CfbError, CfbFile, DIFSECT, ENDOFCHAIN, FATSECT, FREESECT, HEADER_LEN};

impl CfbFile {
    /// Follows one FAT chain from a start sector, refusing cycles and jumps
    /// past the file.
    pub(crate) fn follow_chain(&self, start: u32, limits: &Limits) -> Result<Vec<u32>, CfbError> {
        walk_chain(&self.fat, start, limits)
    }

    /// Concatenates the sectors of the chain starting at `start`, in order:
    /// the bytes of one stream or of the directory, before any truncation to
    /// a declared size.
    pub(crate) fn read_chain(&self, start: u32, limits: &Limits) -> Result<Vec<u8>, CfbError> {
        let chain = self.follow_chain(start, limits)?;
        let mut out: Vec<u8> = Vec::new();
        for sector in chain.iter() {
            let at = sector_offset(*sector, self.sector_len, self.image.len())?;
            let Some(window) = self.image.get(at..at.saturating_add(self.sector_len)) else {
                let refusal = Err(CfbError::Truncated {
                    offset: at,
                    need: at.saturating_add(self.sector_len),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(sector, "cfb: refusing a chain sector past the file");
                return refusal;
            };
            out.extend_from_slice(window);
        }
        Ok(out)
    }
}

/// Walks one chain through `table` (the FAT or the mini FAT) from `start`,
/// refusing a revisit, an index past the table and a link into FAT or DIFAT
/// sectors; every step is bounded by [`Limits::check_pages`], so a cyclic
/// table cannot loop forever.
pub(crate) fn walk_chain(table: &[u32], start: u32, limits: &Limits) -> Result<Vec<u32>, CfbError> {
    if start == ENDOFCHAIN || start == FREESECT {
        return Ok(Vec::new());
    }
    let mut chain: Vec<u32> = Vec::new();
    let mut at = start;
    loop {
        limits.check_pages(chain.len().saturating_add(1))?;
        if chain.contains(&at) {
            let refusal = Err(CfbError::FatCycle { sector: at });
            #[cfg(feature = "trace")]
            crate::trace::debug!(at, "cfb: refusing a chain cycle");
            return refusal;
        }
        let next = usize::try_from(at)
            .ok()
            .and_then(|position| table.get(position).copied());
        let Some(next) = next else {
            let refusal = Err(CfbError::BadFat { sector: at });
            #[cfg(feature = "trace")]
            crate::trace::debug!(at, "cfb: refusing a chain sector past its table");
            return refusal;
        };
        chain.push(at);
        if next == ENDOFCHAIN || next == FREESECT {
            return Ok(chain);
        }
        if next == FATSECT || next == DIFSECT {
            let refusal = Err(CfbError::BadFat { sector: at });
            #[cfg(feature = "trace")]
            crate::trace::debug!(at, "cfb: refusing a chain through a FAT sector");
            return refusal;
        }
        at = next;
    }
}

/// The byte offset of a sector: the header, then sectors in order.
pub(crate) fn sector_offset(
    sector: u32,
    sector_len: usize,
    file_len: usize,
) -> Result<usize, CfbError> {
    let Ok(index) = usize::try_from(sector) else {
        let refusal = Err(CfbError::BadFat { sector });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a sector past the address width");
        return refusal;
    };
    let at = HEADER_LEN.saturating_add(index.saturating_mul(sector_len));
    if at.saturating_add(sector_len) > file_len {
        let refusal = Err(CfbError::Truncated {
            offset: at,
            need: at.saturating_add(sector_len),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!(sector, "cfb: refusing a sector past the file");
        return refusal;
    }
    Ok(at)
}
