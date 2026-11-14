//! The world's tables: the entity slots, the free list and one dense cell vector per column.
//!
//! State changes in three places only: [`World::advance`] applies a checked change set, the snapshot reader
//! rebuilds a clone, and [`WorldBuilder::build`](super::component::WorldBuilder::build) makes the empty world.
//! A change set is checked against the world by [`Tx`](super::tx::Tx) before `advance` sees it, so `advance`
//! meets no refusal a correct caller can provoke; the `Inconsistent` arms exist so that no arm panics. Each arm
//! checks everything it needs before it changes anything, so even such a refusal leaves the arm's own state as it
//! was.
//!
//! Every column holds one cell for every slot, made when the slot is made. So the table costs
//! `slots * columns * CELL_COST` bytes whatever the cells hold, and that cost counts against the bytes ceiling
//! (see [`World::footprint`]): a world cannot grow its tables with values of no length.
//!
//! Every order is an index order. The only keyed map, `by_id`, is a `BTreeMap` and is never iterated into an
//! output.

use std::collections::BTreeMap;

use lgwks_std::id::Uuid;

use super::codec::Op;
use super::component::{Ceilings, Column, ColumnDef, Component};
use super::digest::genesis;
use super::error::{Broken, WorldError, refuse, width};
use super::{Changed, Entity, column_bit};

/// What one cell costs in memory whether or not it holds a value: its tick and its optional boxed slice.
pub(crate) const CELL_COST: usize = size_of::<Cell>();

/// One row of the entity table.
#[derive(Debug, Clone, Copy)]
pub(super) struct Slot {
    /// How many times the slot was despawned. Saturates rather than wrapping.
    pub(super) generation: u32,
    /// Whether the slot holds a live entity.
    pub(super) alive: bool,
    /// The id of the live entity, or of the last one the slot held.
    pub(super) id: Uuid,
    /// The tick the slot, or any of its cells, last moved on.
    pub(super) tick: u64,
}

impl Slot {
    /// A slot that never held an entity.
    pub(super) fn vacant() -> Self {
        Self {
            generation: 0,
            alive: false,
            id: Uuid::from([0_u8; 16]),
            tick: 0,
        }
    }
}

/// One cell of a column: a value or the mark of a removed one, with its tick.
#[derive(Debug, Clone)]
pub(super) struct Cell {
    /// The tick the cell last moved on. Zero for a cell that never held a value.
    pub(super) tick: u64,
    /// The canonical bytes, or `None` when the cell is empty.
    pub(super) value: Option<Box<[u8]>>,
}

impl Cell {
    /// A cell that never held a value.
    pub(super) fn empty() -> Self {
        Self {
            tick: 0,
            value: None,
        }
    }
}

/// The slot index of a table position, refused when the position does not fit the table's width.
pub(super) fn slot_index(position: usize) -> Result<u32, WorldError> {
    u32::try_from(position).map_err(width)
}

/// The table position of a slot index.
pub(super) fn position(index: u32) -> Result<usize, WorldError> {
    usize::try_from(index).map_err(width)
}

/// One tenant's entities and columns.
///
/// `Clone` because a checkpoint is a copy: the cell clones the world to keep one, which a `bevy_ecs` world
/// cannot do (ADR 0053). The world holds no lock; the caller owns the one that guards it.
#[derive(Debug, Clone)]
pub(crate) struct World {
    /// The tenant. Every change set, snapshot and hash is bound to it.
    pub(super) tenant: String,
    /// The ceilings this world enforces.
    pub(super) ceilings: Ceilings,
    /// The registered columns, in registration order.
    pub(super) defs: Vec<ColumnDef>,
    /// The digest of the column names in order: change sets and snapshots carry it.
    pub(super) layout: [u8; 32],
    /// The tick of the last applied change set. Zero for a world that applied none.
    pub(super) tick: u64,
    /// The link of the history: the digest of the last applied change set, or the genesis link.
    pub(super) link: [u8; 32],
    /// The entity table, in index order.
    pub(super) slots: Vec<Slot>,
    /// Dead slots, popped last in, first out.
    pub(super) free: Vec<u32>,
    /// The slot of each live entity id.
    pub(super) by_id: BTreeMap<Uuid, u32>,
    /// One cell vector per column, one cell for each slot of the table.
    pub(super) columns: Vec<Vec<Cell>>,
    /// Live entities.
    pub(super) live: usize,
    /// Live value bytes across every cell.
    pub(super) bytes: usize,
}

impl World {
    /// The empty world of `tenant` with `defs` registered.
    pub(super) fn new(
        tenant: String,
        ceilings: Ceilings,
        defs: Vec<ColumnDef>,
        layout: [u8; 32],
    ) -> Self {
        let columns = defs.iter().map(|_| Vec::new()).collect();
        Self {
            link: genesis(&tenant, &layout),
            tenant,
            ceilings,
            defs,
            layout,
            tick: 0,
            slots: Vec::new(),
            free: Vec::new(),
            by_id: BTreeMap::new(),
            columns,
            live: 0,
            bytes: 0,
        }
    }

    /// The tenant the world belongs to.
    pub(crate) fn tenant(&self) -> &str {
        &self.tenant
    }

    /// The tick of the last applied change set.
    pub(crate) fn tick(&self) -> u64 {
        self.tick
    }

    /// The number of live entities.
    pub(crate) fn live(&self) -> usize {
        self.live
    }

    /// The live value bytes across every cell.
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    /// The memory the cell tables occupy: one cell for each slot of each column, whatever the cells hold.
    fn table_bytes(&self) -> usize {
        self.slots
            .len()
            .saturating_mul(self.defs.len())
            .saturating_mul(CELL_COST)
    }

    /// The memory the world is charged against its bytes ceiling: live value bytes plus the cell tables.
    pub(super) fn footprint(&self) -> usize {
        self.bytes.saturating_add(self.table_bytes())
    }

    /// Live entities and live value bytes counted from the tables, not from the running totals.
    pub(crate) fn recount(&self) -> (usize, usize) {
        let alive = self.slots.iter().filter(|slot| slot.alive).count();
        let bytes = self
            .columns
            .iter()
            .flatten()
            .filter_map(|cell| cell.value.as_ref())
            .fold(0_usize, |total, value| total.saturating_add(value.len()));
        (alive, bytes)
    }

    /// The slot and generation of a live entity.
    pub(crate) fn entity(&self, id: &Uuid) -> Option<Entity> {
        let index = *self.by_id.get(id)?;
        let slot = self.slots.get(position(index).ok()?)?;
        Some(Entity {
            index,
            generation: slot.generation,
        })
    }

    /// The position of `column` in this world, proving the handle was made for this component.
    pub(super) fn column_of<C: Component>(&self, column: Column<C>) -> Result<usize, WorldError> {
        let index = column.index();
        match self.defs.get(usize::from(index)) {
            None => refuse(WorldError::UnknownColumn { index }),
            Some(def) if def.name != C::NAME => refuse(WorldError::ColumnType {
                index,
                expected: C::NAME,
            }),
            Some(_) => Ok(usize::from(index)),
        }
    }

    /// The canonical bytes of one cell, `None` when the cell is empty.
    ///
    /// # Errors
    ///
    /// [`WorldError::UnknownColumn`] for a column past the registered ones and [`WorldError::NoSuchEntity`]
    /// for an id that is not alive.
    pub(crate) fn raw(&self, column: u16, id: &Uuid) -> Result<Option<&[u8]>, WorldError> {
        if usize::from(column) >= self.defs.len() {
            return refuse(WorldError::UnknownColumn { index: column });
        }
        let Some(&index) = self.by_id.get(id) else {
            return refuse(WorldError::NoSuchEntity { id: *id });
        };
        let held = self
            .columns
            .get(usize::from(column))
            .and_then(|cells| cells.get(position(index).ok()?))
            .and_then(|cell| cell.value.as_deref());
        Ok(held)
    }

    /// The value of one cell, `None` when the cell is empty.
    ///
    /// # Errors
    ///
    /// The refusals of [`World::raw`], [`WorldError::ColumnType`] for a handle made for another component
    /// and [`WorldError::Codec`] for stored bytes the component cannot read.
    pub(crate) fn get<C: Component>(
        &self,
        column: Column<C>,
        id: &Uuid,
    ) -> Result<Option<C>, WorldError> {
        self.column_of(column)?;
        match self.raw(column.index(), id)? {
            None => Ok(None),
            Some(bytes) => match C::from_wire(bytes) {
                Ok(value) => Ok(Some(value)),
                Err(_malformed) => refuse(WorldError::Codec { column: C::NAME }),
            },
        }
    }

    /// The length of one cell's value as the tables hold it, `None` for an empty cell or a dead entity.
    pub(super) fn cell_len(&self, column: usize, id: &Uuid) -> Option<usize> {
        let index = position(*self.by_id.get(id)?).ok()?;
        let cell = self.columns.get(column)?.get(index)?;
        cell.value.as_ref().map(|value| value.len())
    }

    /// Every entity that moved after `tick`, in slot order.
    ///
    /// This is the only change query. A despawned entity appears once as a tombstone (`alive` false, no
    /// columns) and keeps appearing until a later tick passes it.
    pub(crate) fn changed_since(&self, tick: u64) -> impl Iterator<Item = Changed> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter(move |entry| entry.1.tick > tick)
            .filter_map(move |(position, slot)| {
                Some(Changed {
                    entity: Entity {
                        index: slot_index(position).ok()?,
                        generation: slot.generation,
                    },
                    id: slot.id,
                    alive: slot.alive,
                    columns: self.moved_columns(position, tick),
                })
            })
    }

    /// The mask of columns whose cell at `position` moved after `tick`.
    fn moved_columns(&self, position: usize, tick: u64) -> u64 {
        let mut mask = 0_u64;
        for (column, cells) in self.columns.iter().enumerate() {
            let moved = cells.get(position).is_some_and(|cell| cell.tick > tick);
            if let (true, Some(bit)) = (moved, column_bit(column)) {
                mask |= bit;
            }
        }
        mask
    }

    /// Apply a checked change set at the next tick.
    ///
    /// An empty change set moves nothing, not even the tick or the link. Otherwise the tick rises by one and the
    /// link becomes `link`, the digest of the change set.
    pub(super) fn advance(&mut self, ops: &[Op], link: [u8; 32]) -> Result<(), WorldError> {
        if ops.is_empty() {
            return Ok(());
        }
        let Some(tick) = self.tick.checked_add(1) else {
            return refuse(WorldError::TickExhausted);
        };
        for op in ops {
            self.apply(op, tick)?;
        }
        self.tick = tick;
        self.link = link;
        Ok(())
    }

    /// Apply one operation at `tick`.
    fn apply(&mut self, op: &Op, tick: u64) -> Result<(), WorldError> {
        match *op {
            Op::Spawn { id } => self.spawn(Uuid::from(id), tick),
            Op::Despawn { id } => self.despawn(&Uuid::from(id), tick),
            Op::Set {
                column,
                id,
                ref value,
            } => self.set(usize::from(column), &Uuid::from(id), value, tick),
            Op::Remove { column, id } => self.remove(usize::from(column), &Uuid::from(id), tick),
        }
    }

    /// A new live entity in the last freed slot, or in a new slot with an empty cell in every column.
    fn spawn(&mut self, id: Uuid, tick: u64) -> Result<(), WorldError> {
        if self.by_id.contains_key(&id) {
            return refuse(WorldError::Inconsistent(Broken::SpawnOfLiveId));
        }
        let reused = self.free.last().copied();
        let index = match reused {
            Some(index) => index,
            None => slot_index(self.slots.len())?,
        };
        let at = position(index)?;
        if reused.is_some() && at >= self.slots.len() {
            return refuse(WorldError::Inconsistent(Broken::SlotMissing));
        }
        if reused.is_none() {
            self.slots.push(Slot::vacant());
            for cells in &mut self.columns {
                cells.push(Cell::empty());
            }
        } else {
            self.free.pop();
        }
        let Some(slot) = self.slots.get_mut(at) else {
            return refuse(WorldError::Inconsistent(Broken::SlotMissing));
        };
        slot.alive = true;
        slot.id = id;
        slot.tick = tick;
        self.by_id.insert(id, index);
        self.live = self.live.saturating_add(1);
        Ok(())
    }

    /// Kill an entity: every cell it holds is emptied back to a cell that never moved.
    fn despawn(&mut self, id: &Uuid, tick: u64) -> Result<(), WorldError> {
        let Some(&index) = self.by_id.get(id) else {
            return refuse(WorldError::Inconsistent(Broken::DespawnOfAbsentId));
        };
        let at = position(index)?;
        if at >= self.slots.len() {
            return refuse(WorldError::Inconsistent(Broken::SlotMissing));
        }
        if self.columns.iter().any(|cells| cells.len() <= at) {
            return refuse(WorldError::Inconsistent(Broken::CellMissing));
        }
        self.by_id.remove(id);
        for cell in self
            .columns
            .iter_mut()
            .filter_map(|cells| cells.get_mut(at))
        {
            if let Some(value) = cell.value.take() {
                self.bytes = self.bytes.saturating_sub(value.len());
            }
            *cell = Cell::empty();
        }
        if let Some(slot) = self.slots.get_mut(at) {
            slot.alive = false;
            slot.generation = slot.generation.saturating_add(1);
            slot.tick = tick;
        }
        self.free.push(index);
        self.live = self.live.saturating_sub(1);
        Ok(())
    }

    /// The slot, the cell of `column` and the byte total for the live entity `id`, found before anything moves.
    fn locate(
        &mut self,
        column: usize,
        id: &Uuid,
    ) -> Result<(&mut Slot, &mut Cell, &mut usize), WorldError> {
        let Some(&index) = self.by_id.get(id) else {
            return refuse(WorldError::Inconsistent(Broken::WriteToAbsentId));
        };
        let at = position(index)?;
        let Some(slot) = self.slots.get_mut(at) else {
            return refuse(WorldError::Inconsistent(Broken::SlotMissing));
        };
        let Some(cells) = self.columns.get_mut(column) else {
            return refuse(WorldError::Inconsistent(Broken::ColumnMissing));
        };
        let Some(cell) = cells.get_mut(at) else {
            return refuse(WorldError::Inconsistent(Broken::CellMissing));
        };
        Ok((slot, cell, &mut self.bytes))
    }

    /// Put `value` in the cell of `column` for the entity `id`.
    fn set(&mut self, column: usize, id: &Uuid, value: &[u8], tick: u64) -> Result<(), WorldError> {
        let (slot, cell, bytes) = self.locate(column, id)?;
        let old = match cell.value {
            Some(ref held) => held.len(),
            None => 0,
        };
        cell.value = Some(Box::from(value));
        cell.tick = tick;
        slot.tick = tick;
        *bytes = bytes.saturating_sub(old).saturating_add(value.len());
        Ok(())
    }

    /// Empty the cell of `column` for the entity `id`. An empty cell stays as it was.
    fn remove(&mut self, column: usize, id: &Uuid, tick: u64) -> Result<(), WorldError> {
        let (slot, cell, bytes) = self.locate(column, id)?;
        let Some(value) = cell.value.take() else {
            return Ok(());
        };
        cell.tick = tick;
        slot.tick = tick;
        *bytes = bytes.saturating_sub(value.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Ceilings, Op, Uuid, World, WorldError};
    use crate::world::error::Broken;

    /// A world with no columns, which `Tx` never lets a hostile write reach: the tests drive `advance` directly.
    fn bare() -> Result<World, WorldError> {
        Ok(World::new(
            "acme".to_owned(),
            Ceilings::new(1, 8, 1 << 10)?,
            Vec::new(),
            [0; 32],
        ))
    }

    /// Catches an arm of `advance` that changes state before it has checked (the dropped plug inside the world):
    /// a spawn of a live id, a despawn and a write to an id that is not alive, and a write to a column that does
    /// not exist each answer with their own named variant and leave the hash, the slot table and the free list
    /// exactly as they were, with a free slot waiting to be taken (#1002).
    #[test]
    fn sim_world_refused_arm_of_advance_leaves_the_world_as_it_was() -> Result<(), WorldError> {
        let mut world = bare()?;
        let (live, gone, absent) = ([7_u8; 16], [8_u8; 16], [9_u8; 16]);
        world.advance(&[Op::Spawn { id: live }, Op::Spawn { id: gone }], [1; 32])?;
        world.advance(&[Op::Despawn { id: gone }], [2; 32])?;
        let before = world.hash()?;
        let (slots, free) = (world.slots.len(), world.free.clone());
        let cases = [
            (Op::Spawn { id: live }, Broken::SpawnOfLiveId),
            (Op::Despawn { id: absent }, Broken::DespawnOfAbsentId),
            (
                Op::Set {
                    column: 0,
                    id: absent,
                    value: vec![1],
                },
                Broken::WriteToAbsentId,
            ),
            (
                Op::Remove {
                    column: 0,
                    id: absent,
                },
                Broken::WriteToAbsentId,
            ),
            (
                Op::Set {
                    column: 0,
                    id: live,
                    value: vec![1],
                },
                Broken::ColumnMissing,
            ),
        ];
        for (op, broken) in cases {
            let outcome = world.advance(std::slice::from_ref(&op), [3; 32]);
            assert!(
                matches!(outcome, Err(WorldError::Inconsistent(found)) if found == broken),
                "{op:?} met {outcome:?}, not {broken:?}"
            );
            assert_eq!(world.hash()?, before, "{op:?} moved the world");
            assert_eq!(world.slots.len(), slots, "{op:?} grew the slot table");
            assert_eq!(world.free, free, "{op:?} took a free slot");
            assert_eq!(
                world.entity(&Uuid::from(live)).map(|held| held.index),
                Some(0)
            );
        }
        Ok(())
    }
}
