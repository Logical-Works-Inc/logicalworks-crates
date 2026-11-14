//! The owned world: one tenant's typed columns, change ticks, transactions and snapshots (ADR 0055, W2 of #985).
//!
//! A world is the private heap of one tenant's cell (ADR 0053 decision 1). It holds entities and columns and no
//! lock: the caller owns the lock, and a [`state::World`] is reached through `&mut` or `&`. Four rules shape it.
//!
//! - **Entities.** A slot is a dense index plus a generation. The stable name is a version 7 `Uuid` the caller
//!   issues, so the world reads no clock and draws no entropy. Slots are visited in index order, which makes
//!   one log give one order.
//! - **Columns.** One per component type, registered at build with the component's canonical byte form
//!   ([`component::Component`]). Columns, entities and bytes have ceilings; a write past one is a typed refusal
//!   ([`error::WorldError::Ceiling`]).
//! - **Transactions.** [`state::World::begin`] gives a [`tx::Tx`] that buffers writes and checks every ceiling as
//!   it buffers. `commit` encodes the change set, applies it and returns the bytes. A dropped `Tx` holds
//!   nothing the world can see, so it changes nothing.
//! - **One hash.** [`digest`] hashes an explicit canonical column byte form with BLAKE3. It never hashes rkyv
//!   output, which `lgwks_std::wire` says is not a content identity. The hash is the oracle of every test.
//! - **Integrity.** Change sets and snapshots carry a format number. A change set carries the link of the history
//!   it continues and the digest of its content; a snapshot carries the link and the world hash it must restore
//!   to. A flipped byte, a foreign history and a stale base are each a typed refusal.
//!
//! The module is compiled under test and into the `world-bench` bin until the cell uses it (#1004). The
//! module names no `ECS` engine crate and no driver: `tests/it/core_boundary.rs` holds that.

pub(crate) mod codec;
pub(crate) mod component;
pub(crate) mod digest;
pub(crate) mod error;
pub(crate) mod snapshot;
pub(crate) mod state;
pub(crate) mod tx;

use lgwks_std::id::Uuid;

use self::component::{Column, Component};

/// The most columns a world holds. A changed-columns mask is one `u64`, so the bound is the word.
pub(crate) const MAX_COLUMNS: usize = 64;

/// The most live entities any world may be configured to hold: 2^24.
pub(crate) const MAX_ENTITIES: usize = 16_777_216;

/// The most live value bytes any world may be configured to hold: 2 GiB.
pub(crate) const MAX_BYTES: usize = 2_147_483_648;

/// The longest value one cell holds, in bytes.
pub(crate) const MAX_VALUE_BYTES: usize = 65_536;

/// The most operations one transaction buffers.
pub(crate) const MAX_TX_OPS: usize = 65_536;

/// The most payload bytes one transaction buffers: its values plus [`OP_OVERHEAD`] per operation.
pub(crate) const MAX_CHANGE_BYTES: usize = 8_388_608;

/// The most bytes a change set may occupy on the wire, and the most `replay` reads.
pub(crate) const MAX_WIRE_BYTES: usize = 16_777_216;

/// The longest tenant name, in bytes.
pub(crate) const MAX_TENANT_BYTES: usize = 128;

/// What one buffered operation costs against [`MAX_CHANGE_BYTES`], beyond its value.
pub(crate) const OP_OVERHEAD: usize = 64;

/// The format of a change set and of a snapshot. A reader refuses any other number.
pub(crate) const FORMAT: u16 = 1;

/// A slot of the entity table: a dense index and the generation that index is on.
///
/// The generation counts despawns of the slot. A handle with an older generation names an entity that is
/// gone, even when the index holds a new one. The stable name across logs and tenants is the `Uuid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Entity {
    /// The dense slot index.
    pub(crate) index: u32,
    /// How many times the slot was despawned before this life.
    pub(crate) generation: u32,
}

/// One row of [`state::World::changed_since`]: an entity that moved after a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Changed {
    /// The slot and its generation.
    pub(crate) entity: Entity,
    /// The stable id the log names.
    pub(crate) id: Uuid,
    /// Whether the entity lives. A despawned entity is reported once as a tombstone, with no columns.
    pub(crate) alive: bool,
    /// One bit per column, in registration order, set when that cell moved after the tick.
    columns: u64,
}

impl Changed {
    /// Whether the cell of `column` moved after the tick the query named.
    pub(crate) fn touches<C: Component>(&self, column: Column<C>) -> bool {
        column_bit(usize::from(column.index())).is_some_and(|bit| self.columns & bit != 0)
    }
}

/// The bit of column `index` in a changed-columns mask, or `None` past the word.
fn column_bit(index: usize) -> Option<u64> {
    u32::try_from(index)
        .ok()
        .and_then(|shift| 1_u64.checked_shl(shift))
}
