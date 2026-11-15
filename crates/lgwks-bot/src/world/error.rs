//! The world's own error layer: every refusal is a variant, and none is a string.
//!
//! The world is a layer of its own (#990 splits `BotError`), so its refusals are not variants of `BotError`.
//! A refusal leaves the world as it was: a write refuses before it buffers, and `commit`, `replay` and
//! `restore` check everything before they change anything.

use std::fmt;

use lgwks_std::id::Uuid;
use lgwks_std::wire::WireError;

/// Which ceiling a [`WorldError::Ceiling`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Limit {
    /// Columns registered at build.
    Columns,
    /// Live entities.
    Entities,
    /// Live value bytes.
    Bytes,
    /// Bytes of one value.
    ValueBytes,
    /// Operations in one transaction.
    TxOps,
    /// Payload bytes in one transaction.
    ChangeBytes,
    /// Bytes of one change set on the wire.
    WireBytes,
    /// Bytes of one snapshot on the wire.
    SnapshotBytes,
}

impl Limit {
    /// The ceiling in words a log line reads.
    fn name(self) -> &'static str {
        match self {
            Self::Columns => "columns",
            Self::Entities => "entities",
            Self::Bytes => "world bytes",
            Self::ValueBytes => "value bytes",
            Self::TxOps => "transaction operations",
            Self::ChangeBytes => "transaction payload bytes",
            Self::WireBytes => "change set wire bytes",
            Self::SnapshotBytes => "snapshot wire bytes",
        }
    }
}

/// What was wrong with bytes that decoded but cannot be a change set or a snapshot of this world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Defect {
    /// Rows are not in strictly increasing slot order.
    RowOrder,
    /// A row names a slot past the snapshot's table.
    RowIndex,
    /// Cells of one row are not in strictly increasing column order.
    CellOrder,
    /// A cell names a column the world does not have.
    CellColumn,
    /// A dead row carries cells.
    DeadRowCells,
    /// The free list names a live slot, a slot twice, or a slot past the table.
    FreeList,
    /// A tick lies outside the span the snapshot covers.
    TickRange,
    /// A value is not the canonical bytes of its column.
    NotCanonical,
    /// The slot table is shorter than the world's or past the entity ceiling.
    SlotTable,
    /// A change set holds more operations than a transaction may.
    TooManyOps,
    /// A row's generation is behind the slot it replaces.
    Generation,
}

impl Defect {
    /// The defect in words a log line reads.
    fn name(self) -> &'static str {
        match self {
            Self::RowOrder => "rows out of order",
            Self::RowIndex => "row past the slot table",
            Self::CellOrder => "cells out of order",
            Self::CellColumn => "cell names an unknown column",
            Self::DeadRowCells => "dead row holds cells",
            Self::FreeList => "free list disagrees with the slots",
            Self::TickRange => "tick outside the covered span",
            Self::NotCanonical => "value is not canonical",
            Self::SlotTable => "slot table shrank or passed its ceiling",
            Self::TooManyOps => "too many operations",
            Self::Generation => "generation behind the slot",
        }
    }
}

/// Which state a checked operation found that the check had ruled out. Each is a defect of this module, not of
/// the caller: `Tx` and the snapshot reader check first, so none is reachable by a correct caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Broken {
    /// A spawn named an id that is alive.
    SpawnOfLiveId,
    /// A despawn named an id that is not alive.
    DespawnOfAbsentId,
    /// A write named an id that is not alive.
    WriteToAbsentId,
    /// An operation named a slot the table does not hold.
    SlotMissing,
    /// An operation named a column the world does not hold.
    ColumnMissing,
    /// A column holds no cell for a slot of the table.
    CellMissing,
}

impl Broken {
    /// The state in words a log line reads.
    fn name(self) -> &'static str {
        match self {
            Self::SpawnOfLiveId => "spawn of a live id",
            Self::DespawnOfAbsentId => "despawn of an id that is not alive",
            Self::WriteToAbsentId => "write to an id that is not alive",
            Self::SlotMissing => "slot missing from the table",
            Self::ColumnMissing => "column missing from the world",
            Self::CellMissing => "cell missing from its column",
        }
    }
}

/// Why the world refused.
#[derive(Debug)]
pub(crate) enum WorldError {
    /// The tenant name is empty or longer than the bound.
    Tenant {
        /// Its length in bytes.
        len: usize,
    },
    /// A ceiling was configured outside `1..=hard`.
    CeilingConfig {
        /// Which ceiling.
        what: Limit,
        /// The value asked for.
        asked: usize,
        /// The hard maximum.
        hard: usize,
    },
    /// A write, a build or a read of bytes went past a ceiling. Nothing changed.
    Ceiling {
        /// Which ceiling.
        what: Limit,
        /// Its value.
        limit: usize,
        /// What the refused step needed.
        asked: usize,
    },
    /// Two columns share a name.
    DuplicateColumn {
        /// The shared name.
        name: &'static str,
    },
    /// A component's canonical bytes are not a fixed point of its codec.
    CodecDrift {
        /// The column's name.
        column: &'static str,
    },
    /// A column handle names no column of this world.
    UnknownColumn {
        /// The index the handle carried.
        index: u16,
    },
    /// A column handle names a column registered for another component.
    ColumnType {
        /// The index the handle carried.
        index: u16,
        /// The component name the handle is for.
        expected: &'static str,
    },
    /// An entity id is not a version 7 `Uuid`.
    NotV7 {
        /// The refused id.
        id: Uuid,
    },
    /// An entity id is already alive.
    DuplicateEntity {
        /// The id.
        id: Uuid,
    },
    /// An entity id is not alive.
    NoSuchEntity {
        /// The id.
        id: Uuid,
    },
    /// Stored bytes do not decode as their component.
    Codec {
        /// The column's name.
        column: &'static str,
    },
    /// The wire codec refused to archive or to read bytes.
    Wire(WireError),
    /// The change set or snapshot belongs to another tenant.
    TenantMismatch,
    /// The change set or snapshot was made for another column layout.
    LayoutMismatch,
    /// A change set was made on another tick than the world stands on.
    Stale {
        /// The tick the change set was built on.
        base: u64,
        /// The tick the world stands on.
        now: u64,
    },
    /// A snapshot does not continue from the world's tick.
    ChainBroken {
        /// The tick the world stands on.
        expected: u64,
        /// The tick the snapshot starts after.
        found: u64,
    },
    /// A snapshot was asked for from a tick the world has not reached.
    FutureTick {
        /// The tick asked for.
        asked: u64,
        /// The tick the world stands on.
        now: u64,
    },
    /// The world's tick cannot advance.
    TickExhausted,
    /// Bytes decoded and break a rule of the format.
    Malformed(Defect),
    /// A count does not fit the width the format gives it.
    Width,
    /// Applying a checked operation found state the check did not expect.
    Inconsistent(Broken),
    /// The bytes were written in a format this reader does not know.
    UnsupportedFormat {
        /// The format the bytes carry.
        found: u16,
    },
    /// A change set's digest is not the digest of its content.
    DigestMismatch,
    /// A change set continues another history than this world's.
    Diverged,
    /// A snapshot applied cleanly and the world does not hash as the snapshot says it must.
    HashMismatch {
        /// The hash the world has after the snapshot.
        found: [u8; 32],
    },
}

impl fmt::Display for WorldError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Tenant { len } => {
                write!(formatter, "a tenant name of {len} bytes is not allowed")
            }
            Self::CeilingConfig { what, asked, hard } => write!(
                formatter,
                "the {} ceiling {asked} is outside 1..={hard}",
                what.name()
            ),
            Self::Ceiling { what, limit, asked } => write!(
                formatter,
                "the {} ceiling is {limit} and the step needed {asked}",
                what.name()
            ),
            Self::DuplicateColumn { name } => {
                write!(formatter, "column {name:?} is registered twice")
            }
            Self::CodecDrift { column } => write!(
                formatter,
                "column {column:?}: the codec does not reproduce its own bytes"
            ),
            Self::UnknownColumn { index } => write!(formatter, "no column {index}"),
            Self::ColumnType { index, expected } => {
                write!(
                    formatter,
                    "column {index} is not the column of {expected:?}"
                )
            }
            Self::NotV7 { id } => write!(formatter, "entity id {id} is not a version 7 uuid"),
            Self::DuplicateEntity { id } => write!(formatter, "entity {id} is already alive"),
            Self::NoSuchEntity { id } => write!(formatter, "entity {id} is not alive"),
            Self::Codec { column } => {
                write!(formatter, "column {column:?}: stored bytes do not decode")
            }
            Self::Wire(ref cause) => write!(formatter, "the wire codec refused: {cause}"),
            Self::TenantMismatch => formatter.write_str("the bytes belong to another tenant"),
            Self::LayoutMismatch => formatter.write_str("the bytes were made for another layout"),
            Self::Stale { base, now } => write!(
                formatter,
                "the change set was built on tick {base} and the world stands on {now}"
            ),
            Self::ChainBroken { expected, found } => write!(
                formatter,
                "the snapshot starts after tick {found} and the world stands on {expected}"
            ),
            Self::FutureTick { asked, now } => {
                write!(formatter, "tick {asked} is ahead of the world at {now}")
            }
            Self::TickExhausted => formatter.write_str("the world's tick cannot advance"),
            Self::Malformed(defect) => write!(formatter, "malformed bytes: {}", defect.name()),
            Self::Width => formatter.write_str("a count does not fit its wire width"),
            Self::Inconsistent(broken) => {
                write!(
                    formatter,
                    "a checked operation met unexpected state: {}",
                    broken.name()
                )
            }
            Self::UnsupportedFormat { found } => {
                write!(
                    formatter,
                    "the bytes are in format {found}, which this reader does not know"
                )
            }
            Self::DigestMismatch => {
                formatter.write_str("the change set is not the one its digest names")
            }
            Self::Diverged => formatter.write_str("the change set continues another history"),
            Self::HashMismatch { found } => {
                formatter.write_str("the world after the snapshot hashes as ")?;
                for byte in found.iter().take(8) {
                    write!(formatter, "{byte:02x}")?;
                }
                formatter.write_str(", not as the snapshot says")
            }
        }
    }
}

impl std::error::Error for WorldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Wire(ref cause) => Some(cause),
            Self::Tenant { .. }
            | Self::CeilingConfig { .. }
            | Self::Ceiling { .. }
            | Self::DuplicateColumn { .. }
            | Self::CodecDrift { .. }
            | Self::UnknownColumn { .. }
            | Self::ColumnType { .. }
            | Self::NotV7 { .. }
            | Self::DuplicateEntity { .. }
            | Self::NoSuchEntity { .. }
            | Self::Codec { .. }
            | Self::TenantMismatch
            | Self::LayoutMismatch
            | Self::Stale { .. }
            | Self::ChainBroken { .. }
            | Self::FutureTick { .. }
            | Self::TickExhausted
            | Self::Malformed(_)
            | Self::Width
            | Self::Inconsistent(_)
            | Self::UnsupportedFormat { .. }
            | Self::DigestMismatch
            | Self::Diverged
            | Self::HashMismatch { .. } => None,
        }
    }
}

/// Log `error` at `warn` and hand it back, so every refusal leaves a record.
pub(crate) fn refused(error: WorldError) -> WorldError {
    lgwks_std::trace::warn!(%error, "world: refused");
    error
}

/// A refusal as a `Result`, logged.
pub(crate) fn refuse<T>(error: WorldError) -> Result<T, WorldError> {
    Err(refused(error))
}

/// A count that does not fit its width, logged with what the conversion said.
pub(super) fn width(cause: std::num::TryFromIntError) -> WorldError {
    lgwks_std::trace::warn!(%cause, "world: a count does not fit its width");
    WorldError::Width
}

/// A ceiling refusal, logged.
pub(crate) fn over<T>(what: Limit, limit: usize, asked: usize) -> Result<T, WorldError> {
    refuse(WorldError::Ceiling { what, limit, asked })
}
