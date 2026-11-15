//! The world hash: BLAKE3 over an explicit canonical byte form of the columns, in order.
//!
//! `lgwks_std::wire` states that rkyv bytes are not a content identity, so this hash never reads them. It
//! reads the tables in index order and writes each field as fixed-width little-endian integers and
//! length-prefixed byte strings. The form, version 1:
//!
//! ```text
//! "lgwks.world.hash.v1"            framed
//! tenant                           framed
//! layout                           32 bytes
//! tick                             u64
//! link                             32 bytes
//! slot count                       u64
//! per slot, in index order         generation u32, alive u8, id 16 bytes, tick u64
//! free count                       u64
//! per free entry, in pop order     u32
//! per column, in registration order, per slot, in index order:
//!                                  tick u64, present u8, and the value framed when present
//! ```
//!
//! A cell past the end of a column's vector reads as the empty cell, so a short vector and a long one that
//! holds only empty cells hash alike. Ceilings are configuration and are not part of the form.
//!
//! The link is a hash chain over the change sets. It starts at [`genesis`] and becomes the digest of each change
//! set applied. The digest of a change set is BLAKE3 over its own explicit form, version 1:
//!
//! ```text
//! "lgwks.world.change.v1"          framed
//! format                           u16
//! tenant                           framed
//! layout                           32 bytes
//! base tick                        u64
//! prev link                        32 bytes
//! operation count                  u64
//! per operation, in order          tag u8 (0 spawn, 1 despawn, 2 set, 3 remove), column u16, id 16 bytes,
//!                                  and for a set the value framed
//! ```

use lgwks_std::hash::{Digest, Hasher};

use super::codec::{ChangeSet, Op};
use super::error::{WorldError, width};
use super::state::{Cell, World};

/// The link of a world that has applied no change set: it names the tenant and the layout.
pub(super) fn genesis(tenant: &str, layout: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.write_framed(b"lgwks.world.genesis.v1");
    hasher.write_framed(tenant.as_bytes());
    hasher.update(layout);
    *hasher.finalize().as_bytes()
}

/// The digest of a change set's content, which is every field but the digest itself.
pub(crate) fn change_digest(change: &ChangeSet) -> Result<[u8; 32], WorldError> {
    let mut hasher = Hasher::new();
    hasher.write_framed(b"lgwks.world.change.v1");
    hasher.update(&change.format.to_le_bytes());
    hasher.write_framed(change.tenant.as_bytes());
    hasher.update(&change.layout);
    hasher.update(&change.base_tick.to_le_bytes());
    hasher.update(&change.prev);
    hasher.update(&count(change.ops.len())?);
    for op in &change.ops {
        match *op {
            Op::Spawn { id } => feed_op(&mut hasher, 0, 0, &id),
            Op::Despawn { id } => feed_op(&mut hasher, 1, 0, &id),
            Op::Set {
                column,
                id,
                ref value,
            } => {
                feed_op(&mut hasher, 2, column, &id);
                hasher.write_framed(value);
            }
            Op::Remove { column, id } => feed_op(&mut hasher, 3, column, &id),
        }
    }
    Ok(*hasher.finalize().as_bytes())
}

/// Feed the fixed part of one operation into `hasher`.
fn feed_op(hasher: &mut Hasher, tag: u8, column: u16, id: &[u8; 16]) {
    hasher.update(&[tag]);
    hasher.update(&column.to_le_bytes());
    hasher.update(id);
}

/// An integer count as eight little-endian bytes, refused when it does not fit.
fn count(length: usize) -> Result<[u8; 8], WorldError> {
    u64::try_from(length).map(u64::to_le_bytes).map_err(width)
}

/// Feed one cell, or the empty cell when `cell` is `None`, into `hasher`.
fn feed_cell(hasher: &mut Hasher, cell: Option<&Cell>) {
    match cell {
        None => {
            hasher.update(&0_u64.to_le_bytes());
            hasher.update(&[0]);
        }
        Some(held) => {
            hasher.update(&held.tick.to_le_bytes());
            match held.value {
                Some(ref value) => {
                    hasher.update(&[1]);
                    hasher.write_framed(value);
                }
                None => {
                    hasher.update(&[0]);
                }
            }
        }
    }
}

impl World {
    /// The hash of the world's canonical byte form: the test oracle for equality of two worlds.
    ///
    /// # Errors
    ///
    /// [`WorldError::Width`] when a count does not fit eight bytes; no target Rust supports reaches it.
    pub(crate) fn hash(&self) -> Result<Digest, WorldError> {
        let mut hasher = Hasher::new();
        hasher.write_framed(b"lgwks.world.hash.v1");
        hasher.write_framed(self.tenant.as_bytes());
        hasher.update(&self.layout);
        hasher.update(&self.tick.to_le_bytes());
        hasher.update(&self.link);
        hasher.update(&count(self.slots.len())?);
        for slot in &self.slots {
            hasher.update(&slot.generation.to_le_bytes());
            hasher.update(&[u8::from(slot.alive)]);
            hasher.update(slot.id.as_bytes());
            hasher.update(&slot.tick.to_le_bytes());
        }
        hasher.update(&count(self.free.len())?);
        for index in &self.free {
            hasher.update(&index.to_le_bytes());
        }
        for cells in &self.columns {
            for position in 0..self.slots.len() {
                feed_cell(&mut hasher, cells.get(position));
            }
        }
        Ok(hasher.finalize())
    }
}
