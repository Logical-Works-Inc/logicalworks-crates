//! Components, column handles, ceilings and the builder that registers them.
//!
//! A column is registered once, at build, with the component's canonical byte form. There is no way to
//! register a column without that form: [`Component`] is the codec, and [`WorldBuilder::column`] takes only
//! types that implement it. A registry that outlives the world does not exist; the columns live in the world
//! that registered them, so two tenants never share a registry.

use std::marker::PhantomData;

use lgwks_std::hash::Hasher;

use super::error::{Limit, WorldError, over, refuse, width};
use super::state::World;
use super::{MAX_BYTES, MAX_COLUMNS, MAX_ENTITIES, MAX_TENANT_BYTES, MAX_VALUE_BYTES};

/// Bytes a snapshot row may cost beyond its cells, in the bound [`Ceilings::snapshot_bytes`].
const ROW_BYTES: usize = 64;

/// Bytes one snapshot cell may cost beyond its value.
const CELL_BYTES: usize = 32;

/// Room a snapshot header may cost.
const HEADER_BYTES: usize = 4_096;

/// Why [`Component::from_wire`] refused bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Malformed;

/// A value one column stores, with its canonical byte form.
///
/// The bytes are the identity of the value: the world hashes them, snapshots them and logs them. So
/// `to_wire` must be a function of the value alone, and `from_wire(to_wire(v))` must give a value that
/// writes the same bytes. [`WorldBuilder::column`] checks that fixed point on a probe value.
pub(crate) trait Component: Sized {
    /// The column's name: unique in a world, and part of the layout hash.
    const NAME: &'static str;

    /// Append the canonical bytes of the value to `out`.
    fn to_wire(&self, out: &mut Vec<u8>);

    /// Read a value from its canonical bytes.
    fn from_wire(bytes: &[u8]) -> Result<Self, Malformed>;
}

/// A typed handle to one column of the world that registered it.
pub(crate) struct Column<C> {
    /// The column's position in registration order.
    index: u16,
    /// The component the column holds; no value of it is stored here.
    marker: PhantomData<fn() -> C>,
}

impl<C> std::fmt::Debug for Column<C> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Column({})", self.index)
    }
}

impl<C> Clone for Column<C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<C> Copy for Column<C> {}

impl<C> Column<C> {
    /// The column's position in registration order.
    pub(crate) fn index(self) -> u16 {
        self.index
    }
}

/// What the world keeps of one registered column.
#[derive(Debug, Clone, Copy)]
pub(super) struct ColumnDef {
    /// The component's name.
    pub(super) name: &'static str,
    /// Whether bytes are the canonical form of some value of the component.
    pub(super) canonical: fn(&[u8]) -> bool,
}

/// Whether `bytes` are the canonical bytes of a value of `C`.
fn is_canonical<C: Component>(bytes: &[u8]) -> bool {
    match C::from_wire(bytes) {
        Ok(value) => {
            let mut again = Vec::with_capacity(bytes.len());
            value.to_wire(&mut again);
            again == bytes
        }
        Err(Malformed) => false,
    }
}

/// The three ceilings of a world. Each binds: a step past one is a [`WorldError::Ceiling`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ceilings {
    /// Columns a build may register.
    pub(super) columns: usize,
    /// Live entities.
    pub(super) entities: usize,
    /// Live value bytes.
    pub(super) bytes: usize,
}

/// `asked` when it lies in `1..=hard`, and a typed refusal when it does not.
fn within(what: Limit, asked: usize, hard: usize) -> Result<usize, WorldError> {
    if (1..=hard).contains(&asked) {
        Ok(asked)
    } else {
        refuse(WorldError::CeilingConfig { what, asked, hard })
    }
}

impl Ceilings {
    /// Ceilings of `columns` columns, `entities` live entities and `bytes` live value bytes.
    ///
    /// Each lies in `1..=hard`: [`MAX_COLUMNS`], [`MAX_ENTITIES`] and [`MAX_BYTES`]. A ceiling of zero or
    /// past its hard maximum is refused, so no world is configured never to bind.
    pub(crate) fn new(columns: usize, entities: usize, bytes: usize) -> Result<Self, WorldError> {
        Ok(Self {
            columns: within(Limit::Columns, columns, MAX_COLUMNS)?,
            entities: within(Limit::Entities, entities, MAX_ENTITIES)?,
            bytes: within(Limit::Bytes, bytes, MAX_BYTES)?,
        })
    }

    /// The most bytes a snapshot of a world under these ceilings can occupy on the wire.
    pub(super) fn snapshot_bytes(self) -> usize {
        let per_row = self
            .columns
            .saturating_mul(CELL_BYTES)
            .saturating_add(ROW_BYTES);
        self.bytes
            .saturating_add(self.entities.saturating_mul(per_row))
            .saturating_add(HEADER_BYTES)
    }
}

/// Registers columns, then builds the world of one tenant.
#[derive(Debug)]
pub(crate) struct WorldBuilder {
    /// The tenant the world belongs to.
    tenant: String,
    /// The ceilings the world enforces.
    ceilings: Ceilings,
    /// The columns registered so far, in order.
    defs: Vec<ColumnDef>,
}

impl WorldBuilder {
    /// A builder for `tenant` under `ceilings`.
    ///
    /// # Errors
    ///
    /// [`WorldError::Tenant`] when the name is empty or past [`MAX_TENANT_BYTES`].
    pub(crate) fn new(tenant: &str, ceilings: Ceilings) -> Result<Self, WorldError> {
        if tenant.is_empty() || tenant.len() > MAX_TENANT_BYTES {
            return refuse(WorldError::Tenant { len: tenant.len() });
        }
        Ok(Self {
            tenant: tenant.to_owned(),
            ceilings,
            defs: Vec::new(),
        })
    }

    /// Register the column of `C`, proving its codec on `probe`.
    ///
    /// # Errors
    ///
    /// [`WorldError::Ceiling`] past the column ceiling, [`WorldError::DuplicateColumn`] for a name taken,
    /// and [`WorldError::CodecDrift`] when the probe's bytes are not a fixed point of the codec.
    pub(crate) fn column<C: Component>(&mut self, probe: &C) -> Result<Column<C>, WorldError> {
        let held = self.defs.len();
        if held >= self.ceilings.columns {
            return over(
                Limit::Columns,
                self.ceilings.columns,
                held.saturating_add(1),
            );
        }
        if self.defs.iter().any(|def| def.name == C::NAME) {
            return refuse(WorldError::DuplicateColumn { name: C::NAME });
        }
        let mut bytes = Vec::new();
        probe.to_wire(&mut bytes);
        if bytes.len() > MAX_VALUE_BYTES {
            return over(Limit::ValueBytes, MAX_VALUE_BYTES, bytes.len());
        }
        if !is_canonical::<C>(&bytes) {
            return refuse(WorldError::CodecDrift { column: C::NAME });
        }
        let index = u16::try_from(held).map_err(width)?;
        self.defs.push(ColumnDef {
            name: C::NAME,
            canonical: is_canonical::<C>,
        });
        Ok(Column {
            index,
            marker: PhantomData,
        })
    }

    /// The empty world.
    pub(crate) fn build(self) -> World {
        let mut hasher = Hasher::new();
        hasher.write_framed(b"lgwks.world.layout.v1");
        for def in &self.defs {
            hasher.write_framed(def.name.as_bytes());
        }
        let layout = *hasher.finalize().as_bytes();
        World::new(self.tenant, self.ceilings, self.defs, layout)
    }
}
