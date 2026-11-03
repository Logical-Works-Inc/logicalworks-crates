//! Database pages: windows, detection and header writes.
//!
//! [`page_window`] slices one page out of the image; [`detect_version`]
//! reads the engine from the magic; [`write_u16_at`] and
//! [`write_header_counts`] lay the header fields the writer needs.

use super::super::limits::Limits;
use super::{AccessError, DbVersion};

/// Slices one page out of the image, refusing jumps past the file.
pub(crate) fn page_window(bytes: &[u8], page: u32, page_size: usize) -> Result<&[u8], AccessError> {
    let Ok(index) = usize::try_from(page) else {
        let refusal = Err(AccessError::BadPage {
            page,
            why: String::from("page past the address width"),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!(page, "access: refusing a page past the address width");
        return refusal;
    };
    let at = index.saturating_mul(page_size);
    let Some(window) = bytes.get(at..at.saturating_add(page_size)) else {
        let refusal = Err(AccessError::Truncated {
            offset: at,
            need: at.saturating_add(page_size),
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!(page, "access: refusing a page past the file");
        return refusal;
    };
    Ok(window)
}

/// Reads the engine from the magic, refusing anything else.
pub(crate) fn detect_version(bytes: &[u8]) -> Result<DbVersion, AccessError> {
    let Some(head) = bytes.get(..20) else {
        let refusal = Err(AccessError::Truncated {
            offset: 0,
            need: 20,
        });
        #[cfg(feature = "trace")]
        crate::trace::debug!("access: refusing a file with no magic window");
        return refusal;
    };
    if head == super::JET_MAGIC {
        let Some(version_byte) = bytes.get(21).copied() else {
            let refusal = Err(AccessError::Truncated {
                offset: 21,
                need: 22,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!("access: refusing a header with no version");
            return refusal;
        };
        return DbVersion::from_byte(version_byte).ok_or(AccessError::UnknownVersion {
            version: version_byte,
        });
    }
    if head == super::ACE_MAGIC {
        return Ok(DbVersion::Ace);
    }
    let refusal = Err(AccessError::BadMagic);
    #[cfg(feature = "trace")]
    crate::trace::debug!("access: refusing a magic mismatch");
    refusal
}

/// Writes a `u16` count into a page header at an absolute offset.
pub(crate) fn write_u16_at(page: &mut [u8], at: usize, value: usize) -> Result<(), AccessError> {
    let Ok(narrow) = u16::try_from(value) else {
        return crate::legacy::refuse(AccessError::BadPage {
            page: 0,
            why: String::from("count past 16 bits"),
        });
    };
    let bytes = narrow.to_le_bytes();
    let Some(window) = page.get_mut(at..at.saturating_add(2)) else {
        return crate::legacy::refuse(AccessError::BadPage {
            page: 0,
            why: String::from("count past the page"),
        });
    };
    window.copy_from_slice(&bytes);
    Ok(())
}

/// Writes the page size, version, page count and catalogue page into page 0.
///
/// Header integers are little-endian, matching what [`page_window`] readers
/// parse with the `get_u32_le` family.
pub(crate) fn write_header_counts(
    image: &mut [u8],
    version: DbVersion,
    page_total: usize,
    limits: &Limits,
) -> Result<(), AccessError> {
    if let Some(window) = image.get_mut(20..22) {
        let bytes = match u16::try_from(version.page_size()) {
            Ok(drawn) => drawn,
            Err(_) => {
                return crate::legacy::refuse(AccessError::BadPage {
                    page: 0,
                    why: String::from("page size past 16 bits"),
                });
            }
        }
        .to_le_bytes();
        window.copy_from_slice(&bytes);
    }
    if let Some(slot) = image.get_mut(21) {
        *slot = version.version_byte();
    }
    if image.get(22).is_none() {
        return crate::legacy::refuse(AccessError::Truncated {
            offset: 22,
            need: 23,
        });
    }
    let page_count = match u32::try_from(page_total) {
        Ok(drawn) => drawn,
        Err(_) => {
            return crate::legacy::refuse(AccessError::BadPage {
                page: 0,
                why: String::from("page total past 32 bits"),
            });
        }
    };
    let bytes = page_count.to_le_bytes();
    if let Some(window) = image.get_mut(24..28) {
        window.copy_from_slice(&bytes);
    }
    let catalog_bytes = 1_u32.to_le_bytes();
    if let Some(window) = image.get_mut(28..32) {
        window.copy_from_slice(&catalog_bytes);
    }
    limits.check_bytes(image.len())?;
    Ok(())
}
