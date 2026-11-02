//! `cfb` reads OLE compound files (MS-CFB) and decompresses VBA sources (MS-OVBA).
//!
//! The header, FAT, mini FAT, directory and streams live in their own files;
//! this root holds the shared error, the file handle and the constants. The
//! header, FAT, mini FAT and directory of a compound file (the format `.xls`
//! and VBA projects live in) are validated per MS-CFB §2.2-§2.6: magic,
//! major version 3 or 4, sector chains with cycle detection, and a linear
//! directory scan. Streams read through the FAT, or through the mini FAT
//! rooted at the root storage for small streams. An `EncryptionInfo` stream
//! marks an encrypted file, and any other major version is refused
//! (MS-CFB §2.2.1). `vba` decompresses module source from the
//! `_VBA_PROJECT` container per MS-OVBA §2.4.1: signature `0x01`, 4 KiB
//! chunks with a 12-bit size, literal bytes and 12-bit-offset copy tokens.

/// The directory table: entries, names, links and spans.
mod directory;
/// Sector chains: following, cycle detection and byte offsets.
mod fat;
/// The compound file header: magic, version and the first chain heads.
mod header;
/// Streams: FAT and mini-FAT reads, plus the image builder.
mod stream;

pub use self::stream::build_image;
/// VBA module-source (de)compression, MS-OVBA §2.4.1.
pub mod vba;

use std::fmt;

use super::limits::LimitError;

/// The compound-file magic: `D0 CF 11 E0 A1 B1 1A E1` (MS-CFB §2.2).
const MAGIC: [u8; 8] = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];

/// Bytes of one file header (MS-CFB §2.2).
const HEADER_LEN: usize = 512;

/// Bytes of one directory entry (MS-CFB §2.6.1).
const DIR_ENTRY_LEN: usize = 128;

/// Bytes of one mini sector (MS-CFB §2.4).
const MINI_SECTOR_LEN: usize = 64;

/// The FAT marker for a free sector (MS-CFB §2.3.1).
const FREESECT: u32 = 0xffff_ffff;

/// The FAT marker for the end of a chain (MS-CFB §2.3.1).
const ENDOFCHAIN: u32 = 0xffff_fffe;

/// The FAT marker for a sector holding the FAT itself.
const FATSECT: u32 = 0xffff_fffd;

/// The FAT marker for a sector holding the DIFAT.
const DIFSECT: u32 = 0xffff_fffc;

/// The sentinel for no stream in directory links (MS-CFB §2.6.1).
const NOSTREAM: u32 = 0xffff_ffff;

/// The mini-stream cutoff this writer emits: zero, so every stream reads
/// through the FAT and no mini layout is needed to round-trip.
const WRITER_CUTOFF: u32 = 0;

/// Why a compound file could not be opened or read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CfbError {
    /// The first eight bytes are not the compound-file magic.
    BadMagic,
    /// A major version outside 3 and 4.
    UnknownVersion {
        /// The observed major version.
        version: u16,
    },
    /// The input ends before the structure does.
    Truncated {
        /// Byte offset where the input ended.
        offset: usize,
        /// Bytes the structure needed.
        need: usize,
    },
    /// A sector chain revisits a sector.
    FatCycle {
        /// The revisited sector.
        sector: u32,
    },
    /// A FAT entry points past the file.
    BadFat {
        /// The offending sector.
        sector: u32,
    },
    /// A directory link points past the entry table.
    BadDir {
        /// The offending link.
        link: u32,
    },
    /// No stream carries the name.
    StreamMissing {
        /// The requested name.
        name: String,
    },
    /// An `EncryptionInfo` stream marks an encrypted file.
    Encrypted,
    /// A stream name MS-CFB §2.6.1 forbids, or one a sibling already holds.
    BadName {
        /// The refused name, verbatim.
        name: String,
        /// The rule it breaks.
        why: String,
    },
    /// The input or output passed a [`Limits`](super::limits::Limits) ceiling.
    Limit(LimitError),
}

impl fmt::Display for CfbError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::FatCycle { sector } => write!(formatter, "FAT chain revisits sector {sector}"),
            Self::BadFat { sector } => {
                write!(
                    formatter,
                    "FAT entry points past the file at sector {sector}"
                )
            }
            Self::BadDir { link } => {
                write!(
                    formatter,
                    "directory link {link} points past the entry table"
                )
            }
            Self::StreamMissing { ref name } => {
                write!(formatter, "compound file holds no stream {name:?}")
            }
            Self::Truncated { offset, need } => {
                write!(
                    formatter,
                    "compound file ends at {offset} short of {need} bytes"
                )
            }
            Self::BadMagic => formatter.write_str("compound file magic mismatch"),
            Self::UnknownVersion { version } => {
                write!(formatter, "compound file version {version} is not 3 or 4")
            }
            Self::Encrypted => formatter.write_str("compound file is encrypted"),
            Self::BadName { ref name, ref why } => {
                write!(formatter, "stream name {name:?} refused: {why}")
            }
            Self::Limit(ref cause) => {
                write!(formatter, "compound limits refused the input: {cause}")
            }
        }
    }
}

crate::legacy::limits::limit_carrying_error!(CfbError);

/// One parsed directory entry: the name, kind and stream span.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DirEntry {
    /// The stream or storage name.
    name: String,
    /// The entry type: 0 empty, 1 storage, 2 stream, 5 root.
    kind: u8,
    /// The first sector of the stream, or `NOSTREAM`.
    start: u32,
    /// The stream size in bytes.
    size: u64,
    /// The left sibling link, or `NOSTREAM`.
    left: u32,
    /// The right sibling link, or `NOSTREAM`.
    right: u32,
    /// The child link, or `NOSTREAM`.
    child: u32,
}

/// An opened compound file: sectors, chains and the directory.
///
/// The header parses in `header`, chains resolve in `fat`, entries in
/// `directory`, and bytes in `stream`; this struct is the handle they
/// share.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CfbFile {
    /// Bytes of one sector: 512 for version 3, 4096 for version 4.
    sector_len: usize,
    /// The FAT: one next-sector per sector.
    fat: Vec<u32>,
    /// The directory entries in file order.
    entries: Vec<DirEntry>,
    /// The raw file bytes the sectors slice.
    image: Vec<u8>,
    /// The mini-stream cutoff from the header.
    cutoff: u32,
    /// The mini FAT: one next mini-sector per mini sector.
    mini_fat: Vec<u32>,
    /// The root storage's stream: the mini sectors' bytes.
    mini_stream: Vec<u8>,
}
