//! The wire forms: a change set and a snapshot, archived with `lgwks_std::wire` and bounded on the way in.
//!
//! These bytes carry the log and the checkpoint. They are not the world's identity: `lgwks_std::wire` says
//! archive bytes are not a content identity, so [`super::digest`] hashes the canonical column form instead.
//! Both readers bound the input before they decode it, then validate what decoded.

use lgwks_std::wire::{
    AlignedVec, Archive, Deserialize, Serialize, WireError, from_bytes, to_bytes,
};

use super::error::{Limit, WorldError, over, refuse, refused};
use super::{FORMAT, MAX_WIRE_BYTES};

/// One buffered write, named by the stable id of its entity.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv)]
pub(crate) enum Op {
    /// A new entity.
    Spawn {
        /// The version 7 id.
        id: [u8; 16],
    },
    /// An entity and every cell it holds are gone.
    Despawn {
        /// The id.
        id: [u8; 16],
    },
    /// A cell takes a value.
    Set {
        /// The column's position.
        column: u16,
        /// The entity's id.
        id: [u8; 16],
        /// The value's canonical bytes.
        value: Vec<u8>,
    },
    /// A cell is emptied.
    Remove {
        /// The column's position.
        column: u16,
        /// The entity's id.
        id: [u8; 16],
    },
}

/// The writes of one committed transaction: what the log appends and `replay` reads.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv)]
pub(crate) struct ChangeSet {
    /// The format of these bytes, [`FORMAT`].
    pub(crate) format: u16,
    /// The tenant the writes belong to.
    pub(crate) tenant: String,
    /// The column layout the writes were made for.
    pub(crate) layout: [u8; 32],
    /// The tick the transaction began on.
    pub(crate) base_tick: u64,
    /// The link of the history the transaction continues: the digest of the change set before it.
    pub(crate) prev: [u8; 32],
    /// The digest of every field above and of the writes. It becomes the world's link once applied.
    pub(crate) digest: [u8; 32],
    /// The writes, in order.
    pub(crate) ops: Vec<Op>,
}

/// One cell of a snapshot row: a value, or the mark of a removed value, at a tick.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv)]
pub(crate) struct SnapshotCell {
    /// The column's position.
    pub(crate) column: u16,
    /// The tick the cell last moved on.
    pub(crate) tick: u64,
    /// The canonical bytes, or `None` for a removed value.
    pub(crate) value: Option<Vec<u8>>,
}

/// One slot of a snapshot: the slot's state and the cells that moved.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv)]
pub(crate) struct SnapshotRow {
    /// The dense slot index.
    pub(crate) index: u32,
    /// The slot's generation.
    pub(crate) generation: u32,
    /// Whether the slot lives.
    pub(crate) alive: bool,
    /// The id the slot holds, or held last.
    pub(crate) id: [u8; 16],
    /// The tick the slot last moved on.
    pub(crate) tick: u64,
    /// The cells that moved after the snapshot's `since`, in column order.
    pub(crate) cells: Vec<SnapshotCell>,
}

/// The rows that moved after a tick, and the state a replay of them cannot rebuild.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv)]
pub(crate) struct Snapshot {
    /// The format of these bytes, [`FORMAT`].
    pub(crate) format: u16,
    /// The tenant the world belongs to.
    pub(crate) tenant: String,
    /// The column layout of the world.
    pub(crate) layout: [u8; 32],
    /// Rows moved after this tick are present.
    pub(crate) since: u64,
    /// The tick the world stood on when the snapshot was taken.
    pub(crate) tick: u64,
    /// The link of the history the world had at that tick.
    pub(crate) link: [u8; 32],
    /// The world hash at that tick: a restore that does not reach it is refused.
    pub(crate) after: [u8; 32],
    /// How many slots the world's table holds.
    pub(crate) slots: u32,
    /// The free list in pop order, whole: it is state and no row carries it.
    pub(crate) free: Vec<u32>,
    /// The rows that moved, in slot order.
    pub(crate) rows: Vec<SnapshotRow>,
}

/// Archive a change set.
pub(super) fn encode_change(change: &ChangeSet) -> Result<AlignedVec, WorldError> {
    let bytes = to_bytes::<WireError>(change).map_err(|cause| refused(WorldError::Wire(cause)))?;
    if bytes.len() > MAX_WIRE_BYTES {
        return over(Limit::WireBytes, MAX_WIRE_BYTES, bytes.len());
    }
    Ok(bytes)
}

/// Archive a snapshot, refusing one past `limit` bytes.
pub(super) fn encode_snapshot(snapshot: &Snapshot, limit: usize) -> Result<AlignedVec, WorldError> {
    let bytes =
        to_bytes::<WireError>(snapshot).map_err(|cause| refused(WorldError::Wire(cause)))?;
    if bytes.len() > limit {
        return over(Limit::SnapshotBytes, limit, bytes.len());
    }
    Ok(bytes)
}

/// Copy `bytes` to the alignment the archive format needs.
fn aligned(bytes: &[u8]) -> AlignedVec {
    let mut copy = AlignedVec::with_capacity(bytes.len());
    copy.extend_from_slice(bytes);
    copy
}

/// Read a change set, refusing input past [`MAX_WIRE_BYTES`] before it is read at all.
pub(super) fn decode_change(bytes: &[u8]) -> Result<ChangeSet, WorldError> {
    if bytes.len() > MAX_WIRE_BYTES {
        return over(Limit::WireBytes, MAX_WIRE_BYTES, bytes.len());
    }
    let change = from_bytes::<ChangeSet, WireError>(&aligned(bytes))
        .map_err(|cause| refused(WorldError::Wire(cause)))?;
    if change.format != FORMAT {
        return refuse(WorldError::UnsupportedFormat {
            found: change.format,
        });
    }
    Ok(change)
}

/// Read a snapshot, refusing input past `limit` bytes before it is read at all.
pub(super) fn decode_snapshot(bytes: &[u8], limit: usize) -> Result<Snapshot, WorldError> {
    if bytes.len() > limit {
        return over(Limit::SnapshotBytes, limit, bytes.len());
    }
    let snapshot = from_bytes::<Snapshot, WireError>(&aligned(bytes))
        .map_err(|cause| refused(WorldError::Wire(cause)))?;
    if snapshot.format != FORMAT {
        return refuse(WorldError::UnsupportedFormat {
            found: snapshot.format,
        });
    }
    Ok(snapshot)
}
