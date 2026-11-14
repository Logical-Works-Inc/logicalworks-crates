//! Snapshots: the rows that moved after a tick, and a restore that reads them in order.
//!
//! [`World::snapshot_since`] writes the slots whose tick passed `since` with the cells that moved in them, and
//! the whole free list, which is state no row carries. [`World::restore`] reads snapshots in order on a copy
//! of the world and swaps the copy in only when every snapshot read clean, so a refused restore leaves the
//! world as it was. A snapshot is checked, not trusted: tick spans, slot and column bounds, ordering, the
//! canonical form of every value and the free list are all validated before anything is kept.

use std::collections::BTreeSet;

use lgwks_std::id::Uuid;
use lgwks_std::wire::AlignedVec;

use super::codec::{Snapshot, SnapshotCell, SnapshotRow, decode_snapshot, encode_snapshot};
use super::error::{Broken, Defect, Limit, WorldError, over, refuse, width};
use super::state::{CELL_COST, Cell, Slot, World, position, slot_index};
use super::{FORMAT, MAX_VALUE_BYTES};

impl World {
    /// Write the rows that moved after `since`, as wire bytes.
    ///
    /// `since` of zero writes the whole world. The snapshot is bounded by the world's own ceilings.
    ///
    /// # Errors
    ///
    /// [`WorldError::FutureTick`] when `since` is ahead of the world, [`WorldError::Ceiling`] when the bytes
    /// pass the bound the ceilings give, and [`WorldError::Wire`].
    pub(crate) fn snapshot_since(&self, since: u64) -> Result<AlignedVec, WorldError> {
        if since > self.tick {
            return refuse(WorldError::FutureTick {
                asked: since,
                now: self.tick,
            });
        }
        let mut rows = Vec::new();
        for (at, slot) in self.slots.iter().enumerate() {
            if slot.tick > since {
                rows.push(self.row_since(at, slot, since)?);
            }
        }
        let snapshot = Snapshot {
            format: FORMAT,
            tenant: self.tenant.clone(),
            layout: self.layout,
            since,
            tick: self.tick,
            link: self.link,
            after: *self.hash()?.as_bytes(),
            slots: slot_index(self.slots.len())?,
            free: self.free.clone(),
            rows,
        };
        encode_snapshot(&snapshot, self.ceilings.snapshot_bytes())
    }

    /// The row of the slot at `at`: its state and the cells that moved after `since`.
    fn row_since(&self, at: usize, slot: &Slot, since: u64) -> Result<SnapshotRow, WorldError> {
        let mut cells = Vec::new();
        if slot.alive {
            for (column, held) in self.columns.iter().enumerate() {
                let Some(cell) = held.get(at).filter(|cell| cell.tick > since) else {
                    continue;
                };
                cells.push(SnapshotCell {
                    column: u16::try_from(column).map_err(width)?,
                    tick: cell.tick,
                    value: cell.value.as_deref().map(<[u8]>::to_vec),
                });
            }
        }
        Ok(SnapshotRow {
            index: slot_index(at)?,
            generation: slot.generation,
            alive: slot.alive,
            id: *slot.id.as_bytes(),
            tick: slot.tick,
            cells,
        })
    }

    /// Restore from snapshots read in order: the first starts after the world's tick, and each next one
    /// starts after the tick the one before it ended on.
    ///
    /// # Errors
    ///
    /// [`WorldError::TenantMismatch`], [`WorldError::LayoutMismatch`], [`WorldError::ChainBroken`],
    /// [`WorldError::Malformed`], [`WorldError::Ceiling`], [`WorldError::DuplicateEntity`],
    /// [`WorldError::NotV7`], [`WorldError::Wire`], [`WorldError::UnsupportedFormat`] and
    /// [`WorldError::HashMismatch`] for a snapshot that read clean and does not rebuild the world it was taken
    /// from. After any refusal the world is as it was.
    pub(crate) fn restore<B: AsRef<[u8]>>(&mut self, snapshots: &[B]) -> Result<(), WorldError> {
        let mut next = self.clone();
        for bytes in snapshots {
            next.apply_snapshot(bytes.as_ref())?;
        }
        *self = next;
        Ok(())
    }

    /// Read one snapshot into this world.
    fn apply_snapshot(&mut self, bytes: &[u8]) -> Result<(), WorldError> {
        let snapshot = decode_snapshot(bytes, self.ceilings.snapshot_bytes())?;
        if snapshot.tenant != self.tenant {
            return refuse(WorldError::TenantMismatch);
        }
        if snapshot.layout != self.layout {
            return refuse(WorldError::LayoutMismatch);
        }
        if snapshot.since != self.tick {
            return refuse(WorldError::ChainBroken {
                expected: self.tick,
                found: snapshot.since,
            });
        }
        if snapshot.tick < snapshot.since {
            return refuse(WorldError::Malformed(Defect::TickRange));
        }
        self.check_rows(&snapshot)?;
        let slots = self.check_table(&snapshot)?;
        self.slots.resize(slots, Slot::vacant());
        for cells in &mut self.columns {
            cells.resize_with(slots, Cell::empty);
        }
        for row in &snapshot.rows {
            let held = self.slots.get(position(row.index)?);
            if let Some(old) = held.filter(|old| old.alive) {
                self.by_id.remove(&old.id);
            }
        }
        for row in snapshot.rows {
            self.take_row(row)?;
        }
        self.free = snapshot.free;
        self.tick = snapshot.tick;
        self.link = snapshot.link;
        self.check_free()?;
        (self.live, self.bytes) = self.recount();
        if self.live > self.ceilings.entities {
            return over(Limit::Entities, self.ceilings.entities, self.live);
        }
        if self.footprint() > self.ceilings.bytes {
            return over(Limit::Bytes, self.ceilings.bytes, self.footprint());
        }
        let found = *self.hash()?.as_bytes();
        if found != snapshot.after {
            return refuse(WorldError::HashMismatch { found });
        }
        Ok(())
    }

    /// Check what the snapshot claims about the slot table against what it carries, before the table grows by a
    /// byte, and return the slot count to grow to.
    ///
    /// A claimed slot that is not already in the table must be named by a row or by the free list, so the
    /// allocation is bounded by the snapshot's own size and not by a number it states. The cell tables the new
    /// size would take must fit the bytes ceiling.
    fn check_table(&self, snapshot: &Snapshot) -> Result<usize, WorldError> {
        let slots = position(snapshot.slots)?;
        let held = self.slots.len();
        if slots < held || slots > self.ceilings.entities {
            return refuse(WorldError::Malformed(Defect::SlotTable));
        }
        if snapshot.free.iter().any(|&index| index >= snapshot.slots) {
            return refuse(WorldError::Malformed(Defect::FreeList));
        }
        let first_new = slot_index(held)?;
        let named: BTreeSet<u32> = snapshot
            .rows
            .iter()
            .map(|row| row.index)
            .chain(snapshot.free.iter().copied())
            .filter(|&index| index >= first_new)
            .collect();
        if slots.saturating_sub(held) > named.len() {
            return refuse(WorldError::Malformed(Defect::SlotTable));
        }
        let table = slots
            .saturating_mul(self.defs.len())
            .saturating_mul(CELL_COST);
        let asked = self.bytes.saturating_add(table);
        if asked > self.ceilings.bytes {
            return over(Limit::Bytes, self.ceilings.bytes, asked);
        }
        Ok(slots)
    }

    /// Check every row of `snapshot` against the world and the span it covers, changing nothing.
    fn check_rows(&self, snapshot: &Snapshot) -> Result<(), WorldError> {
        let span = snapshot.since.saturating_add(1)..=snapshot.tick;
        let mut previous: Option<u32> = None;
        for row in &snapshot.rows {
            if previous.is_some_and(|last| row.index <= last) {
                return refuse(WorldError::Malformed(Defect::RowOrder));
            }
            previous = Some(row.index);
            if row.index >= snapshot.slots {
                return refuse(WorldError::Malformed(Defect::RowIndex));
            }
            if !span.contains(&row.tick) {
                return refuse(WorldError::Malformed(Defect::TickRange));
            }
            if let Some(old) = self.slots.get(position(row.index)?)
                && row.generation < old.generation
            {
                return refuse(WorldError::Malformed(Defect::Generation));
            }
            if row.alive {
                let id = Uuid::from(row.id);
                if id.version() != Some(7) {
                    return refuse(WorldError::NotV7 { id });
                }
            } else if !row.cells.is_empty() {
                return refuse(WorldError::Malformed(Defect::DeadRowCells));
            }
            self.check_cells(&row.cells, &span)?;
        }
        Ok(())
    }

    /// Check the cells of one row: order, column, tick span, size and canonical form.
    fn check_cells(
        &self,
        cells: &[SnapshotCell],
        span: &std::ops::RangeInclusive<u64>,
    ) -> Result<(), WorldError> {
        let mut previous: Option<u16> = None;
        for cell in cells {
            if previous.is_some_and(|last| cell.column <= last) {
                return refuse(WorldError::Malformed(Defect::CellOrder));
            }
            previous = Some(cell.column);
            let Some(def) = self.defs.get(usize::from(cell.column)) else {
                return refuse(WorldError::Malformed(Defect::CellColumn));
            };
            if !span.contains(&cell.tick) {
                return refuse(WorldError::Malformed(Defect::TickRange));
            }
            if let Some(ref value) = cell.value {
                if value.len() > MAX_VALUE_BYTES {
                    return over(Limit::ValueBytes, MAX_VALUE_BYTES, value.len());
                }
                if !(def.canonical)(value) {
                    return refuse(WorldError::Malformed(Defect::NotCanonical));
                }
            }
        }
        Ok(())
    }

    /// Write one checked row into the tables.
    fn take_row(&mut self, row: SnapshotRow) -> Result<(), WorldError> {
        let at = position(row.index)?;
        let id = Uuid::from(row.id);
        let Some(slot) = self.slots.get_mut(at) else {
            return refuse(WorldError::Inconsistent(Broken::SlotMissing));
        };
        let fresh = slot.generation != row.generation;
        *slot = Slot {
            generation: row.generation,
            alive: row.alive,
            id,
            tick: row.tick,
        };
        if fresh || !row.alive {
            for cells in &mut self.columns {
                if let Some(cell) = cells.get_mut(at) {
                    *cell = Cell::empty();
                }
            }
        }
        for cell in row.cells {
            let Some(held) = self
                .columns
                .get_mut(usize::from(cell.column))
                .and_then(|cells| cells.get_mut(at))
            else {
                return refuse(WorldError::Inconsistent(Broken::CellMissing));
            };
            held.tick = cell.tick;
            held.value = cell.value.map(Vec::into_boxed_slice);
        }
        if row.alive && self.by_id.insert(id, row.index).is_some() {
            return refuse(WorldError::DuplicateEntity { id });
        }
        Ok(())
    }

    /// Check that the free list names each dead slot once and no live one.
    fn check_free(&self) -> Result<(), WorldError> {
        let mut seen = self.free.clone();
        seen.sort_unstable();
        seen.dedup();
        let all_dead = self.free.iter().all(|&index| {
            position(index)
                .ok()
                .and_then(|at| self.slots.get(at))
                .is_some_and(|slot| !slot.alive)
        });
        let dead = self.slots.iter().filter(|slot| !slot.alive).count();
        if seen.len() == self.free.len() && all_dead && self.free.len() == dead {
            Ok(())
        } else {
            refuse(WorldError::Malformed(Defect::FreeList))
        }
    }
}
