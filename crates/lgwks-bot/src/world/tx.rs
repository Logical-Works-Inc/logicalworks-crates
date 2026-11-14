//! Transactions: a system reads the world, buffers writes, and commits or drops them.
//!
//! A [`Tx`] holds the world by `&mut`, so nothing else reads or writes it while the transaction lives, and it
//! buffers every write in a list the world cannot see. Dropping it frees that list: there is no undo because
//! there was no do. Each buffered write is checked against the world as it would stand after the writes
//! before it (the overlay below), and every ceiling is checked then, so a refusal comes at the write that
//! crosses the line and `commit` has nothing left to refuse.
//!
//! `commit` encodes the change set first and applies it second. If encoding fails, nothing was applied.
//! [`World::replay`] feeds a logged change set through the same checks, so hostile bytes meet the ceilings a
//! live write meets.

use std::collections::BTreeMap;

use lgwks_std::id::Uuid;
use lgwks_std::wire::AlignedVec;

use super::codec::{ChangeSet, Op, decode_change, encode_change};
use super::component::{Column, Component};
use super::digest::change_digest;
use super::error::{Defect, Limit, WorldError, over, refuse};
use super::state::{CELL_COST, World};
use super::{FORMAT, MAX_CHANGE_BYTES, MAX_TX_OPS, MAX_VALUE_BYTES, OP_OVERHEAD};

/// What the transaction has done to an entity the world knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presence {
    /// Spawned here: the world's cells for the id, if any, are not this life's.
    Fresh,
    /// Despawned here: the entity is not alive, whatever the world holds.
    Gone,
}

/// Whether a value came from a codec this crate trusts or from bytes a log or a peer gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trust {
    /// Encoded by the registered component, a fixed point by the build's probe.
    Typed,
    /// Decoded from a change set: the value must be canonical for its column.
    Wire,
}

/// A transaction on one world.
#[derive(Debug)]
pub(crate) struct Tx<'world> {
    /// The world the writes will apply to.
    world: &'world mut World,
    /// The world's tick when the transaction began.
    base_tick: u64,
    /// The buffered writes, in order.
    ops: Vec<Op>,
    /// Entities spawned or despawned here.
    entities: BTreeMap<Uuid, Presence>,
    /// Cells written here: the new length of the value, or `None` for a removed one.
    cells: BTreeMap<(Uuid, u16), Option<usize>>,
    /// Live entities as the world would stand after the buffered writes.
    alive: usize,
    /// The footprint (value bytes plus cell tables) as the world would stand after the buffered writes.
    bytes: usize,
    /// Free slots as the world would hold them after the buffered writes: a spawn takes one, a despawn adds one,
    /// and a spawn with none left makes a new slot, which costs a cell in every column.
    spare: usize,
    /// Payload the change set would carry, against [`MAX_CHANGE_BYTES`].
    payload: usize,
}

impl World {
    /// Begin a transaction. Dropping it changes nothing.
    pub(crate) fn begin(&mut self) -> Tx<'_> {
        Tx {
            base_tick: self.tick,
            alive: self.live,
            bytes: self.footprint(),
            spare: self.free.len(),
            world: self,
            ops: Vec::new(),
            entities: BTreeMap::new(),
            cells: BTreeMap::new(),
            payload: 0,
        }
    }

    /// Apply a change set that `commit` returned earlier, from the log.
    ///
    /// The change set must belong to this tenant and layout and must have been built on the tick the world
    /// stands on, so a duplicate, a gap and a reorder are each a typed refusal and the world is unchanged.
    ///
    /// # Errors
    ///
    /// [`WorldError::Ceiling`] for input past the wire ceiling, [`WorldError::Wire`] for bytes that do not
    /// read, [`WorldError::UnsupportedFormat`], [`WorldError::TenantMismatch`], [`WorldError::LayoutMismatch`],
    /// [`WorldError::Stale`], [`WorldError::Diverged`] for a change set that continues another history,
    /// [`WorldError::DigestMismatch`] for one whose bytes are not the ones its digest names, and the refusal any
    /// one of its writes would have met live.
    pub(crate) fn replay(&mut self, bytes: &[u8]) -> Result<(), WorldError> {
        let change = decode_change(bytes)?;
        if change.tenant != self.tenant {
            return refuse(WorldError::TenantMismatch);
        }
        if change.layout != self.layout {
            return refuse(WorldError::LayoutMismatch);
        }
        if change.base_tick != self.tick {
            return refuse(WorldError::Stale {
                base: change.base_tick,
                now: self.tick,
            });
        }
        if change.prev != self.link {
            return refuse(WorldError::Diverged);
        }
        if change.ops.len() > MAX_TX_OPS {
            return refuse(WorldError::Malformed(Defect::TooManyOps));
        }
        if change_digest(&change)? != change.digest {
            return refuse(WorldError::DigestMismatch);
        }
        let digest = change.digest;
        let mut tx = self.begin();
        for op in change.ops {
            tx.push_wire(op)?;
        }
        let Tx { world, ops, .. } = tx;
        world.advance(&ops, digest)
    }
}

impl Tx<'_> {
    /// How many writes the transaction holds.
    pub(crate) fn pending(&self) -> usize {
        self.ops.len()
    }

    /// Read a cell as the world stands. The transaction's own writes are not visible until it commits.
    ///
    /// # Errors
    ///
    /// The refusals of [`World::get`].
    pub(crate) fn get<C: Component>(
        &self,
        column: Column<C>,
        id: &Uuid,
    ) -> Result<Option<C>, WorldError> {
        self.world.get(column, id)
    }

    /// Buffer a new entity named by `id`, a version 7 `Uuid` the caller issued.
    ///
    /// # Errors
    ///
    /// [`WorldError::NotV7`], [`WorldError::DuplicateEntity`], and [`WorldError::Ceiling`] past the entity
    /// or the transaction ceilings. The transaction is unchanged by a refusal.
    pub(crate) fn spawn(&mut self, id: Uuid) -> Result<(), WorldError> {
        if id.version() != Some(7) {
            return refuse(WorldError::NotV7 { id });
        }
        if self.is_alive(&id) {
            return refuse(WorldError::DuplicateEntity { id });
        }
        let limit = self.world.ceilings.entities;
        if self.alive >= limit {
            return over(Limit::Entities, limit, self.alive.saturating_add(1));
        }
        let table = match self.spare {
            0 => self.world.defs.len().saturating_mul(CELL_COST),
            _ => 0,
        };
        let grown = self.bytes.saturating_add(table);
        let ceiling = self.world.ceilings.bytes;
        if grown > ceiling {
            return over(Limit::Bytes, ceiling, grown);
        }
        self.charge(0)?;
        self.clear_cells(&id);
        self.entities.insert(id, Presence::Fresh);
        self.alive = self.alive.saturating_add(1);
        self.bytes = grown;
        self.spare = self.spare.saturating_sub(1);
        self.ops.push(Op::Spawn { id: *id.as_bytes() });
        Ok(())
    }

    /// Buffer the death of a live entity and the loss of every cell it holds.
    ///
    /// # Errors
    ///
    /// [`WorldError::NoSuchEntity`] and [`WorldError::Ceiling`] past the transaction ceilings.
    pub(crate) fn despawn(&mut self, id: Uuid) -> Result<(), WorldError> {
        if !self.is_alive(&id) {
            return refuse(WorldError::NoSuchEntity { id });
        }
        let mut held = 0_usize;
        for column in 0..self.world.defs.len() {
            if let Some(len) = self.cell_len(column, &id) {
                held = held.saturating_add(len);
            }
        }
        self.charge(0)?;
        self.clear_cells(&id);
        self.entities.insert(id, Presence::Gone);
        self.alive = self.alive.saturating_sub(1);
        self.spare = self.spare.saturating_add(1);
        self.bytes = self.bytes.saturating_sub(held);
        self.ops.push(Op::Despawn { id: *id.as_bytes() });
        Ok(())
    }

    /// Buffer `value` into the cell of `column` for the entity `id`.
    ///
    /// # Errors
    ///
    /// [`WorldError::UnknownColumn`], [`WorldError::ColumnType`], [`WorldError::NoSuchEntity`], and
    /// [`WorldError::Ceiling`] past the value, world-bytes or transaction ceilings.
    pub(crate) fn set<C: Component>(
        &mut self,
        column: Column<C>,
        id: Uuid,
        value: &C,
    ) -> Result<(), WorldError> {
        self.world.column_of(column)?;
        let mut bytes = Vec::new();
        value.to_wire(&mut bytes);
        self.set_bytes(column.index(), id, bytes, Trust::Typed)
    }

    /// Buffer the emptying of the cell of `column` for the entity `id`; `false` when it was empty already.
    ///
    /// # Errors
    ///
    /// [`WorldError::UnknownColumn`], [`WorldError::ColumnType`], [`WorldError::NoSuchEntity`], and
    /// [`WorldError::Ceiling`] past the transaction ceilings.
    pub(crate) fn remove<C: Component>(
        &mut self,
        column: Column<C>,
        id: Uuid,
    ) -> Result<bool, WorldError> {
        self.world.column_of(column)?;
        self.remove_cell(column.index(), id)
    }

    /// Buffer one operation read from a change set, under the checks a live write meets.
    fn push_wire(&mut self, op: Op) -> Result<(), WorldError> {
        match op {
            Op::Spawn { id } => self.spawn(Uuid::from(id)),
            Op::Despawn { id } => self.despawn(Uuid::from(id)),
            Op::Set { column, id, value } => {
                self.set_bytes(column, Uuid::from(id), value, Trust::Wire)
            }
            Op::Remove { column, id } => {
                self.remove_cell(column, Uuid::from(id)).map(|_removed| ())
            }
        }
    }

    /// Whether `id` is alive as the transaction would leave the world.
    fn is_alive(&self, id: &Uuid) -> bool {
        match self.entities.get(id) {
            Some(&Presence::Fresh) => true,
            Some(&Presence::Gone) => false,
            None => self.world.by_id.contains_key(id),
        }
    }

    /// The length of the value in a cell as the transaction would leave it.
    fn cell_len(&self, column: usize, id: &Uuid) -> Option<usize> {
        let key = (*id, u16::try_from(column).ok()?);
        match self.entities.get(id) {
            Some(&Presence::Gone) => None,
            Some(&Presence::Fresh) => self.cells.get(&key).copied().flatten(),
            None => match self.cells.get(&key) {
                Some(&written) => written,
                None => self.world.cell_len(column, id),
            },
        }
    }

    /// Drop the overlay's cells for `id`: a spawn or a despawn ends them.
    fn clear_cells(&mut self, id: &Uuid) {
        while let Some(key) = self
            .cells
            .range((*id, 0)..=(*id, u16::MAX))
            .next()
            .map(|entry| *entry.0)
        {
            self.cells.remove(&key);
        }
    }

    /// Charge one operation of `extra` value bytes against the transaction ceilings.
    fn charge(&mut self, extra: usize) -> Result<(), WorldError> {
        if self.ops.len() >= MAX_TX_OPS {
            return over(Limit::TxOps, MAX_TX_OPS, self.ops.len().saturating_add(1));
        }
        let payload = self
            .payload
            .saturating_add(extra)
            .saturating_add(OP_OVERHEAD);
        if payload > MAX_CHANGE_BYTES {
            return over(Limit::ChangeBytes, MAX_CHANGE_BYTES, payload);
        }
        self.payload = payload;
        Ok(())
    }

    /// Buffer a write of canonical `value` bytes.
    fn set_bytes(
        &mut self,
        column: u16,
        id: Uuid,
        value: Vec<u8>,
        trust: Trust,
    ) -> Result<(), WorldError> {
        let Some(def) = self.world.defs.get(usize::from(column)) else {
            return refuse(WorldError::UnknownColumn { index: column });
        };
        if value.len() > MAX_VALUE_BYTES {
            return over(Limit::ValueBytes, MAX_VALUE_BYTES, value.len());
        }
        if !self.is_alive(&id) {
            return refuse(WorldError::NoSuchEntity { id });
        }
        if trust == Trust::Wire && !(def.canonical)(&value) {
            return refuse(WorldError::Malformed(Defect::NotCanonical));
        }
        let before: usize = self.cell_len(usize::from(column), &id).into_iter().sum();
        let after = self
            .bytes
            .saturating_sub(before)
            .saturating_add(value.len());
        let limit = self.world.ceilings.bytes;
        if after > limit {
            return over(Limit::Bytes, limit, after);
        }
        self.charge(value.len())?;
        self.bytes = after;
        self.cells.insert((id, column), Some(value.len()));
        self.ops.push(Op::Set {
            column,
            id: *id.as_bytes(),
            value,
        });
        Ok(())
    }

    /// Buffer the emptying of a cell; `false` when there is nothing to empty.
    fn remove_cell(&mut self, column: u16, id: Uuid) -> Result<bool, WorldError> {
        if usize::from(column) >= self.world.defs.len() {
            return refuse(WorldError::UnknownColumn { index: column });
        }
        if !self.is_alive(&id) {
            return refuse(WorldError::NoSuchEntity { id });
        }
        let Some(held) = self.cell_len(usize::from(column), &id) else {
            return Ok(false);
        };
        self.charge(0)?;
        self.bytes = self.bytes.saturating_sub(held);
        self.cells.insert((id, column), None);
        self.ops.push(Op::Remove {
            column,
            id: *id.as_bytes(),
        });
        Ok(true)
    }

    /// Commit: encode the change set, apply it, and return its wire bytes.
    ///
    /// An empty transaction returns the bytes of an empty change set and moves nothing, not even the tick.
    /// The bytes are what the cell's log appends; `replay` on another world of the same tenant and layout
    /// gives that world the same hash.
    ///
    /// # Errors
    ///
    /// [`WorldError::Ceiling`] when the encoded change set passes the wire ceiling, [`WorldError::Wire`]
    /// when the archive cannot be written, and [`WorldError::TickExhausted`]. After any refusal the world
    /// is as it was: the write checks in [`Tx::set`] and its kin left `advance` nothing to refuse, and each
    /// arm of `advance` checks before it changes anything.
    pub(crate) fn commit(self) -> Result<AlignedVec, WorldError> {
        let Tx {
            world,
            base_tick,
            ops,
            ..
        } = self;
        let mut change = ChangeSet {
            format: FORMAT,
            tenant: world.tenant.clone(),
            layout: world.layout,
            base_tick,
            prev: world.link,
            digest: [0; 32],
            ops,
        };
        change.digest = change_digest(&change)?;
        let bytes = encode_change(&change)?;
        world.advance(&change.ops, change.digest)?;
        Ok(bytes)
    }
}
