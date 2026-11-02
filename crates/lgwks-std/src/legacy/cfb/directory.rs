//! The directory table: entries, names, links and spans.
//!
//! [`CfbFile::read_directory`] concatenates the directory chain and parses
//! each 128-byte entry per MS-CFB §2.6.1: the UTF-16LE name, the kind, the
//! sibling and child links (every link validated against the table) and the
//! start sector with the 64-bit size. Entries of kind 0 are padding and are
//! skipped.

use super::super::limits::Limits;
use super::{CfbError, CfbFile, DIR_ENTRY_LEN, DirEntry, NOSTREAM};
use crate::legacy::{get_u16_le, get_u32_le};

impl CfbFile {
    /// Reads the directory chain into entries, validating every link.
    pub(crate) fn read_directory(&mut self, first: u32, limits: &Limits) -> Result<(), CfbError> {
        let raw = self.read_chain(first, limits)?;
        let count = raw.len().saturating_div(DIR_ENTRY_LEN);
        limits.check_records(count)?;
        for index in 0..count {
            let at = index.saturating_mul(DIR_ENTRY_LEN);
            let Some(window) = raw.get(at..at.saturating_add(DIR_ENTRY_LEN)) else {
                break;
            };
            let entry = parse_dir_entry(window, index)?;
            if entry.kind == 0 {
                continue;
            }
            check_link(entry.left, count)?;
            check_link(entry.right, count)?;
            check_link(entry.child, count)?;
            self.entries.push(entry);
        }
        Ok(())
    }
}

/// Refuses a directory link past the entry table; `NOSTREAM` is always fine.
fn check_link(link: u32, count: usize) -> Result<(), CfbError> {
    if link == NOSTREAM {
        return Ok(());
    }
    let Ok(position) = usize::try_from(link) else {
        let refusal = Err(CfbError::BadDir { link });
        #[cfg(feature = "trace")]
        crate::trace::debug!(
            link,
            "cfb: refusing a directory link past the address width"
        );
        return refusal;
    };
    if position >= count {
        let refusal = Err(CfbError::BadDir { link });
        #[cfg(feature = "trace")]
        crate::trace::debug!(link, "cfb: refusing a directory link past the table");
        return refusal;
    }
    Ok(())
}

/// Parses one 128-byte directory entry: UTF-16LE name, kind, links, span.
fn parse_dir_entry(window: &[u8], index: usize) -> Result<DirEntry, CfbError> {
    if window.len() < DIR_ENTRY_LEN {
        let refusal = Err(CfbError::Truncated {
            offset: index.saturating_mul(DIR_ENTRY_LEN),
            need: DIR_ENTRY_LEN,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a short directory entry");
        return refusal;
    }
    let Some(name_window) = window.get(..64) else {
        let refusal = Err(CfbError::Truncated {
            offset: index,
            need: 64,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a directory entry with no name");
        return refusal;
    };
    let Some(name_len) = get_u16_le(window, 64) else {
        let refusal = Err(CfbError::Truncated {
            offset: 64,
            need: 66,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a directory entry with no name length");
        return refusal;
    };
    let name_chars = usize::from(name_len).saturating_div(2).saturating_sub(1);
    let mut name = String::new();
    for position in 0..name_chars.min(32) {
        let at = position.saturating_mul(2);
        let Some(low) = name_window.get(at).copied() else {
            break;
        };
        let Some(high) = name_window.get(at.saturating_add(1)).copied() else {
            break;
        };
        let unit = u16::from(low).saturating_add(u16::from(high).saturating_mul(256));
        if unit == 0 {
            break;
        }
        if let Some(drawn) = char::from_u32(u32::from(unit)) {
            name.push(drawn);
        }
    }
    let Some(kind) = window.get(66).copied() else {
        let refusal = Err(CfbError::Truncated {
            offset: 66,
            need: 67,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a directory entry with no kind");
        return refusal;
    };
    let Some(left) = get_u32_le(window, 68) else {
        let refusal = Err(CfbError::Truncated {
            offset: 68,
            need: 72,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a directory entry with no left link");
        return refusal;
    };
    let Some(right) = get_u32_le(window, 72) else {
        let refusal = Err(CfbError::Truncated {
            offset: 72,
            need: 76,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a directory entry with no right link");
        return refusal;
    };
    let Some(child) = get_u32_le(window, 76) else {
        let refusal = Err(CfbError::Truncated {
            offset: 76,
            need: 80,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a directory entry with no child link");
        return refusal;
    };
    let Some(start) = get_u32_le(window, 116) else {
        let refusal = Err(CfbError::Truncated {
            offset: 116,
            need: 120,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a directory entry with no start sector");
        return refusal;
    };
    let Some(low) = get_u32_le(window, 120) else {
        let refusal = Err(CfbError::Truncated {
            offset: 120,
            need: 124,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a directory entry with no size");
        return refusal;
    };
    let Some(high) = get_u32_le(window, 124) else {
        let refusal = Err(CfbError::Truncated {
            offset: 124,
            need: 128,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("cfb: refusing a directory entry with no high size");
        return refusal;
    };
    Ok(DirEntry {
        name,
        kind,
        start,
        size: u64::from(low).saturating_add(u64::from(high).saturating_mul(0x1_0000_0000)),
        left,
        right,
        child,
    })
}
