//! The owned world's simulations and properties (ADR 0055, issue #1002).
//!
//! One seed drives every draw: the order of operations, the values, the ids and the cut points. No test reads a
//! clock or the OS. The oracle is never the world's own output alone: a reference model written apart from the
//! world (maps of bytes and last-touch ticks) is compared cell by cell, and two runs of one seed are compared by
//! the world hash. The fools they play are the monkey (random writes and deletes), the pulled plug (a dropped
//! or panicking system), the neighbour (two tenants on the same ids), the weird bytes (truncated and flipped
//! logs) and the saboteur (a codec that lies).
//!
//! The files here are declared from `lib.rs` and not from `world/`, because the `world-bench` bin compiles
//! `world/` by path and these tests need `crate::sim_stream`.

mod ceiling;
mod determinism;
mod dropped;
mod integrity;
mod refusals;
mod replay;
mod snapshot;
mod tenants;

use std::collections::BTreeMap;

use lgwks_std::id::Uuid;

use crate::sim_stream::Stream;
use crate::world::component::{Ceilings, Column, Component, Malformed, WorldBuilder};
use crate::world::error::WorldError;
use crate::world::state::World;
use crate::world::tx::Tx;

/// Why a simulation failed: the world refused where it should not have, or a check against the model broke.
#[derive(Debug)]
pub(super) enum Failure {
    /// The world refused.
    World(WorldError),
    /// A draw did not fit a width.
    Width(std::num::TryFromIntError),
    /// A draw from an empty range.
    Empty,
    /// Two counts that must agree did not.
    Count {
        /// What was counted.
        what: &'static str,
        /// The world's count.
        world: usize,
        /// The reference's count.
        reference: usize,
    },
    /// A check against the model or an expected outcome failed, in fixed words.
    Check(&'static str),
}

/// A failed check as a `Result`, logged so the run leaves a record of why it stopped.
fn fail<T>(failure: Failure) -> Result<T, Failure> {
    lgwks_std::trace::warn!(%failure, "sim_world: a check failed");
    Err(failure)
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::World(ref cause) => write!(formatter, "the world refused: {cause}"),
            Self::Width(ref cause) => write!(formatter, "a draw did not fit: {cause}"),
            Self::Empty => formatter.write_str("a draw from an empty range"),
            Self::Count {
                what,
                world,
                reference,
            } => write!(
                formatter,
                "{what}: the world has {world}, the model {reference}"
            ),
            Self::Check(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for Failure {}

impl From<WorldError> for Failure {
    fn from(cause: WorldError) -> Self {
        Self::World(cause)
    }
}

impl From<std::num::TryFromIntError> for Failure {
    fn from(cause: std::num::TryFromIntError) -> Self {
        Self::Width(cause)
    }
}

/// A position: two `i32`, eight bytes, column 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Pos {
    /// East.
    pub(super) x: i32,
    /// North.
    pub(super) y: i32,
}

impl Component for Pos {
    const NAME: &'static str = "pos";

    fn to_wire(&self, out: &mut Vec<u8>) {
        out.extend(self.x.to_le_bytes().into_iter().chain(self.y.to_le_bytes()));
    }

    fn from_wire(bytes: &[u8]) -> Result<Self, Malformed> {
        match *bytes {
            [x0, x1, x2, x3, y0, y1, y2, y3] => Ok(Self {
                x: i32::from_le_bytes([x0, x1, x2, x3]),
                y: i32::from_le_bytes([y0, y1, y2, y3]),
            }),
            _ => Err(Malformed),
        }
    }
}

/// A name of any length: UTF-8 bytes, column 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Name(pub(super) String);

impl Component for Name {
    const NAME: &'static str = "name";

    fn to_wire(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(self.0.as_bytes());
    }

    fn from_wire(bytes: &[u8]) -> Result<Self, Malformed> {
        match std::str::from_utf8(bytes) {
            Ok(text) => Ok(Self(text.to_owned())),
            Err(_utf8) => Err(Malformed),
        }
    }
}

/// Hit points: four bytes, column 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Hp(pub(super) u32);

impl Component for Hp {
    const NAME: &'static str = "hp";

    fn to_wire(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.0.to_le_bytes());
    }

    fn from_wire(bytes: &[u8]) -> Result<Self, Malformed> {
        match *bytes {
            [b0, b1, b2, b3] => Ok(Self(u32::from_le_bytes([b0, b1, b2, b3]))),
            _ => Err(Malformed),
        }
    }
}

/// The saboteur: a codec whose bytes are not a fixed point, because it adds one on every write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Drift(pub(super) u8);

impl Component for Drift {
    const NAME: &'static str = "drift";

    fn to_wire(&self, out: &mut Vec<u8>) {
        out.push(self.0.wrapping_add(1));
    }

    fn from_wire(bytes: &[u8]) -> Result<Self, Malformed> {
        match *bytes {
            [byte] => Ok(Self(byte)),
            _ => Err(Malformed),
        }
    }
}

/// The three registered columns of the test layout.
#[derive(Debug, Clone, Copy)]
pub(super) struct Cols {
    /// Column 0.
    pub(super) pos: Column<Pos>,
    /// Column 1.
    pub(super) name: Column<Name>,
    /// Column 2.
    pub(super) hp: Column<Hp>,
}

/// An empty world of `tenant` with the test layout under the given ceilings.
pub(super) fn world(
    tenant: &str,
    entities: usize,
    bytes: usize,
) -> Result<(World, Cols), WorldError> {
    let mut builder = WorldBuilder::new(tenant, Ceilings::new(8, entities, bytes)?)?;
    let pos = builder.column(&Pos { x: 0, y: 0 })?;
    let name = builder.column(&Name(String::new()))?;
    let hp = builder.column(&Hp(0))?;
    Ok((builder.build(), Cols { pos, name, hp }))
}

/// The version 7 id of row `seq`: the stamp is `seq` and the rest is drawn, so ids are unique and shaped as
/// RFC 9562 asks. `lgwks_std::id` offers no issuer that takes a clock and entropy, so the tests shape their own.
pub(super) fn mint(seq: u64, noise: u64) -> Uuid {
    let [
        lead,
        spare,
        stamp_a,
        stamp_b,
        stamp_c,
        stamp_d,
        stamp_e,
        stamp_f,
    ] = seq.to_be_bytes();
    let [version, counter, variant, r3, r4, r5, r6, r7] = noise.to_be_bytes();
    Uuid::from([
        stamp_a,
        stamp_b,
        stamp_c,
        stamp_d,
        stamp_e,
        stamp_f,
        0x70 | (version & 0x0f),
        counter,
        0x80 | (variant & 0x3f),
        r3,
        r4,
        r5,
        r6,
        r7,
        lead,
        spare,
    ])
}

/// A drawn index below `bound`, from the whole word.
pub(super) fn below(stream: &mut Stream, bound: usize) -> Result<usize, Failure> {
    let wide = u64::try_from(bound)?;
    let drawn = stream.next().checked_rem(wide).ok_or(Failure::Empty)?;
    Ok(usize::try_from(drawn)?)
}

/// One value a planned write carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Value {
    /// Column 0.
    Pos(Pos),
    /// Column 1.
    Name(Name),
    /// Column 2.
    Hp(Hp),
}

impl Value {
    /// The column the value belongs to.
    pub(super) fn column(&self) -> u16 {
        match *self {
            Self::Pos(_) => 0,
            Self::Name(_) => 1,
            Self::Hp(_) => 2,
        }
    }

    /// The canonical bytes the reference model keeps.
    pub(super) fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match *self {
            Self::Pos(ref value) => value.to_wire(&mut out),
            Self::Name(ref value) => value.to_wire(&mut out),
            Self::Hp(ref value) => value.to_wire(&mut out),
        }
        out
    }
}

/// One write a simulation plans before it buffers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Planned {
    /// A new entity.
    Spawn(Uuid),
    /// A dead entity.
    Despawn(Uuid),
    /// A cell takes a value.
    Set(Uuid, Value),
    /// A cell is emptied.
    Remove(Uuid, u16),
}

/// Buffer one planned write into `tx`.
pub(super) fn buffer(tx: &mut Tx<'_>, cols: Cols, planned: &Planned) -> Result<(), WorldError> {
    match *planned {
        Planned::Spawn(id) => tx.spawn(id),
        Planned::Despawn(id) => tx.despawn(id),
        Planned::Set(id, Value::Pos(ref value)) => tx.set(cols.pos, id, value),
        Planned::Set(id, Value::Name(ref value)) => tx.set(cols.name, id, value),
        Planned::Set(id, Value::Hp(ref value)) => tx.set(cols.hp, id, value),
        Planned::Remove(id, 0) => tx.remove(cols.pos, id).map(|_removed| ()),
        Planned::Remove(id, 1) => tx.remove(cols.name, id).map(|_removed| ()),
        Planned::Remove(id, _) => tx.remove(cols.hp, id).map(|_removed| ()),
    }
}

/// The reference model: what the world must hold, written apart from the world.
#[derive(Debug, Default)]
pub(super) struct Model {
    /// The bytes of every live cell, by entity and column.
    pub(super) cells: BTreeMap<Uuid, BTreeMap<u16, Vec<u8>>>,
    /// The tick each live entity last moved on.
    pub(super) touched: BTreeMap<Uuid, u64>,
    /// The tick each cell last moved on, for cells of live entities.
    pub(super) cell_touched: BTreeMap<(Uuid, u16), u64>,
}

impl Model {
    /// Apply one committed write at `tick`.
    pub(super) fn apply(&mut self, planned: &Planned, tick: u64) {
        match *planned {
            Planned::Spawn(id) => {
                self.cells.insert(id, BTreeMap::new());
                self.touched.insert(id, tick);
            }
            Planned::Despawn(id) => {
                self.cells.remove(&id);
                self.touched.remove(&id);
                for column in 0..3 {
                    self.cell_touched.remove(&(id, column));
                }
            }
            Planned::Set(id, ref value) => {
                if let Some(cells) = self.cells.get_mut(&id) {
                    cells.insert(value.column(), value.bytes());
                }
                self.touched.insert(id, tick);
                self.cell_touched.insert((id, value.column()), tick);
            }
            Planned::Remove(id, column) => {
                let held = self
                    .cells
                    .get_mut(&id)
                    .and_then(|cells| cells.remove(&column));
                if held.is_some() {
                    self.touched.insert(id, tick);
                    self.cell_touched.insert((id, column), tick);
                }
            }
        }
    }

    /// The live value bytes the model holds.
    pub(super) fn bytes(&self) -> usize {
        self.cells
            .values()
            .flat_map(BTreeMap::values)
            .map(Vec::len)
            .sum()
    }
}

/// A seeded stream of random transactions on one world, with the model beside it.
pub(super) struct Driver {
    /// The world under test.
    pub(super) world: World,
    /// Its columns.
    pub(super) cols: Cols,
    /// The reference model.
    pub(super) model: Model,
    /// The live ids in draw order, kept apart from the world.
    pub(super) live: Vec<Uuid>,
    /// Every committed change set, in order.
    pub(super) log: Vec<Vec<u8>>,
    /// How many writes were planned and committed.
    pub(super) writes: usize,
    /// The draws.
    pub(super) stream: Stream,
    /// The next id's stamp.
    seq: u64,
}

impl Driver {
    /// A driver on an empty world of `tenant`, drawing from `seed`.
    pub(super) fn new(
        seed: u64,
        tenant: &str,
        entities: usize,
        bytes: usize,
    ) -> Result<Self, Failure> {
        let (world, cols) = world(tenant, entities, bytes)?;
        Ok(Self {
            world,
            cols,
            model: Model::default(),
            live: Vec::new(),
            log: Vec::new(),
            writes: 0,
            stream: Stream::new(seed),
            seq: 0,
        })
    }

    /// A drawn value for column `column`.
    fn value(&mut self, column: u16) -> Result<Value, Failure> {
        let [x0, x1, x2, x3, y0, y1, y2, y3] = self.stream.next().to_le_bytes();
        Ok(match column {
            0 => Value::Pos(Pos {
                x: i32::from_le_bytes([x0, x1, x2, x3]),
                y: i32::from_le_bytes([y0, y1, y2, y3]),
            }),
            1 => {
                let length = below(&mut self.stream, 25)?;
                let mut name = String::new();
                for _ in 0..length {
                    let letter = u8::try_from(below(&mut self.stream, 26)?)?;
                    name.push(char::from(b'a'.saturating_add(letter)));
                }
                Value::Name(Name(name))
            }
            _ => Value::Hp(Hp(u32::from_le_bytes([y0, y1, y2, y3]))),
        })
    }

    /// Plan `count` writes against the ids the previous writes left alive. The live list moves as if the
    /// writes had committed; a caller that drops them restores a copy it kept.
    pub(super) fn plan(&mut self, count: usize) -> Result<Vec<Planned>, Failure> {
        let mut planned = Vec::new();
        for _ in 0..count {
            let kind = below(&mut self.stream, 10)?;
            if self.live.is_empty() || kind < 3 {
                self.seq = self.seq.saturating_add(1);
                let id = mint(self.seq, self.stream.next());
                self.live.push(id);
                planned.push(Planned::Spawn(id));
                continue;
            }
            let at = below(&mut self.stream, self.live.len())?;
            let id = *self
                .live
                .get(at)
                .ok_or(Failure::Check("a drawn index is past the live ids"))?;
            let column = u16::try_from(below(&mut self.stream, 3)?)?;
            if kind < 7 {
                planned.push(Planned::Set(id, self.value(column)?));
            } else if kind == 7 {
                planned.push(Planned::Remove(id, column));
            } else {
                self.live.swap_remove(at);
                planned.push(Planned::Despawn(id));
            }
        }
        Ok(planned)
    }

    /// Plan, buffer and commit one transaction of one to twelve writes. Returns how many writes it held.
    pub(super) fn step(&mut self) -> Result<usize, Failure> {
        let count = below(&mut self.stream, 12)?.saturating_add(1);
        let planned = self.plan(count)?;
        let mut tx = self.world.begin();
        for write in &planned {
            buffer(&mut tx, self.cols, write)?;
        }
        let bytes = tx.commit()?;
        self.log.push(bytes.to_vec());
        let tick = self.world.tick();
        for write in &planned {
            self.model.apply(write, tick);
        }
        self.writes = self.writes.saturating_add(planned.len());
        Ok(planned.len())
    }

    /// Run transactions until at least `writes` writes were committed.
    pub(super) fn run(&mut self, writes: usize) -> Result<(), Failure> {
        while self.writes < writes {
            self.step()?;
        }
        Ok(())
    }

    /// Check the world against the model: counts, bytes and every cell.
    pub(super) fn check_model(&self) -> Result<(), Failure> {
        let (alive, bytes) = self.world.recount();
        let counts = [
            ("entities", self.world.live(), self.model.cells.len()),
            ("recounted entities", alive, self.world.live()),
            ("value bytes", self.world.bytes(), self.model.bytes()),
            ("recounted bytes", bytes, self.world.bytes()),
        ];
        for (what, world, reference) in counts {
            if world != reference {
                return fail(Failure::Count {
                    what,
                    world,
                    reference,
                });
            }
        }
        for (id, cells) in &self.model.cells {
            for column in 0..3_u16 {
                let held = self.world.raw(column, id)?;
                if held != cells.get(&column).map(Vec::as_slice) {
                    return fail(Failure::Check("a cell differs from the model"));
                }
            }
        }
        Ok(())
    }
}
