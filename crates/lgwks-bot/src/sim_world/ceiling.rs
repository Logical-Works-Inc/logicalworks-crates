//! Every ceiling binds: a write past it is a typed refusal, the world stays as it was, and freeing recovers (#1002).

use super::{Failure, Hp, Name, Pos, mint, world};
use crate::sim_stream::Stream;
use crate::world::component::{Ceilings, WorldBuilder};
use crate::world::error::{Limit, WorldError};
use crate::world::state::CELL_COST;
use crate::world::{MAX_BYTES, MAX_COLUMNS, MAX_ENTITIES, MAX_TX_OPS, MAX_VALUE_BYTES};
use lgwks_std::id::Uuid;

/// Whether `result` is the ceiling refusal of `what` with exactly this limit and ask.
fn is_over<T>(result: &Result<T, WorldError>, what: Limit, limit: usize, asked: usize) -> bool {
    matches!(
        *result,
        Err(WorldError::Ceiling { what: found, limit: bound, asked: needed })
            if found == what && bound == limit && needed == asked
    )
}

/// A name of `length` bytes.
fn name(length: usize) -> Name {
    Name("n".repeat(length))
}

/// The id at `index` of the ids a simulation minted, or the refusal that it minted too few.
fn id_at(ids: &[Uuid], index: usize) -> Result<Uuid, Failure> {
    ids.get(index).copied().ok_or(Failure::Check(
        "the simulation minted fewer ids than it names",
    ))
}

/// Columns are refused past their ceiling at build, and a ceiling that could never bind is refused when
/// it is configured.
fn columns_and_config() -> Result<(), Failure> {
    let mut builder = WorldBuilder::new("acme", Ceilings::new(2, 8, 64)?)?;
    builder.column(&Pos { x: 0, y: 0 })?;
    builder.column(&Name(String::new()))?;
    assert!(
        is_over(&builder.column(&Hp(0)), Limit::Columns, 2, 3),
        "a third column passes a ceiling of two"
    );
    for (columns, entities, bytes, what, hard) in [
        (0, 8, 8, Limit::Columns, MAX_COLUMNS),
        (MAX_COLUMNS + 1, 8, 8, Limit::Columns, MAX_COLUMNS),
        (8, 0, 8, Limit::Entities, MAX_ENTITIES),
        (8, MAX_ENTITIES + 1, 8, Limit::Entities, MAX_ENTITIES),
        (8, 8, 0, Limit::Bytes, MAX_BYTES),
        (8, 8, MAX_BYTES + 1, Limit::Bytes, MAX_BYTES),
    ] {
        assert!(
            matches!(
                Ceilings::new(columns, entities, bytes),
                Err(WorldError::CeilingConfig { what: found, hard: limit, .. })
                    if found == what && limit == hard
            ),
            "{what:?} configured at {columns}/{entities}/{bytes}"
        );
    }
    Ok(())
}

/// Entities: the ninth of eight, across a despawn in the same transaction and across commits.
fn entities_ceiling(ids: &[Uuid]) -> Result<(), Failure> {
    let (first, second, ninth) = (id_at(ids, 0)?, id_at(ids, 1)?, id_at(ids, 8)?);
    let (mut small, _cols) = world("acme", 8, 1 << 20)?;
    let mut tx = small.begin();
    for id in ids.iter().take(8) {
        tx.spawn(*id)?;
    }
    assert!(
        is_over(&tx.spawn(ninth), Limit::Entities, 8, 9),
        "the ninth entity passes a ceiling of eight"
    );
    tx.despawn(first)?;
    tx.spawn(ninth)?;
    tx.commit()?;
    assert_eq!(small.live(), 8, "eight entities live after the swap");
    let full = small.hash()?;
    let mut tx = small.begin();
    assert!(
        is_over(&tx.spawn(mint(10, 10)), Limit::Entities, 8, 9),
        "a full world refuses a new entity in a new transaction"
    );
    drop(tx);
    assert_eq!(small.hash()?, full, "a refused spawn moved the world");
    let mut tx = small.begin();
    tx.despawn(second)?;
    tx.spawn(mint(10, 10))?;
    tx.commit()?;
    Ok(())
}

/// World bytes: replacing a value frees its old length before the new one is charged. Each slot also costs
/// a cell in each of the three columns, so two slots add `tables` to the ceiling and to every ask.
fn bytes_ceiling(ids: &[Uuid]) -> Result<(), Failure> {
    let (first, second) = (id_at(ids, 0)?, id_at(ids, 1)?);
    let tables = 2 * 3 * CELL_COST;
    let (mut tight, cols_tight) = world("acme", 8, tables + 64)?;
    let mut tx = tight.begin();
    tx.spawn(first)?;
    tx.spawn(second)?;
    tx.set(cols_tight.name, first, &name(40))?;
    assert!(
        is_over(
            &tx.set(cols_tight.name, second, &name(40)),
            Limit::Bytes,
            tables + 64,
            tables + 80
        ),
        "forty bytes beside forty pass sixty-four"
    );
    tx.set(cols_tight.name, first, &name(10))?;
    tx.set(cols_tight.name, second, &name(40))?;
    tx.commit()?;
    assert_eq!(tight.bytes(), 50, "ten beside forty");
    let mut tx = tight.begin();
    assert!(
        tx.remove(cols_tight.name, second)?,
        "the held value is removed"
    );
    tx.set(cols_tight.hp, first, &Hp(1))?;
    tx.commit()?;
    assert_eq!(tight.bytes(), 14, "ten and four after the removal");
    Ok(())
}

/// One value: exactly the ceiling passes, one more does not.
fn value_ceiling(first: Uuid) -> Result<(), Failure> {
    let (mut wide, cols_wide) = world("acme", 8, 1 << 20)?;
    let mut tx = wide.begin();
    tx.spawn(first)?;
    tx.set(cols_wide.name, first, &name(MAX_VALUE_BYTES))?;
    assert!(
        is_over(
            &tx.set(cols_wide.name, first, &name(MAX_VALUE_BYTES + 1)),
            Limit::ValueBytes,
            MAX_VALUE_BYTES,
            MAX_VALUE_BYTES + 1
        ),
        "one byte past the value ceiling"
    );
    tx.commit()?;
    Ok(())
}

/// Operations in one transaction.
fn operations_ceiling() -> Result<(), Failure> {
    let (mut many, _cols_many) = world("acme", MAX_TX_OPS + 8, 1 << 23)?;
    let mut tx = many.begin();
    for seq in 1..=u64::try_from(MAX_TX_OPS)? {
        tx.spawn(mint(seq, seq))?;
    }
    assert!(
        is_over(
            &tx.spawn(mint(u64::MAX, 1)),
            Limit::TxOps,
            MAX_TX_OPS,
            MAX_TX_OPS + 1
        ),
        "one operation past the transaction ceiling"
    );
    tx.commit()?;
    assert_eq!(many.live(), MAX_TX_OPS, "the full transaction committed");
    let mut tx = many.begin();
    tx.spawn(mint(u64::MAX, 1))?;
    tx.commit()?;
    Ok(())
}

/// Payload bytes in one transaction: 127 maximal values fit, the 128th does not.
fn payload_ceiling(first: Uuid) -> Result<(), Failure> {
    let (mut heavy, cols_heavy) = world("acme", 8, 1 << 24)?;
    let mut tx = heavy.begin();
    tx.spawn(first)?;
    for _ in 0..127 {
        tx.set(cols_heavy.name, first, &name(MAX_VALUE_BYTES))?;
    }
    assert!(
        matches!(
            tx.set(cols_heavy.name, first, &name(MAX_VALUE_BYTES)),
            Err(WorldError::Ceiling {
                what: Limit::ChangeBytes,
                ..
            })
        ),
        "the 128th maximal value passes the payload ceiling"
    );
    tx.commit()?;
    assert_eq!(heavy.bytes(), MAX_VALUE_BYTES, "the last value stands");
    Ok(())
}

/// Catches a ceiling that never binds (the giant): the column, entity, byte, value, operation and payload
/// ceilings each refuse with their exact numbers at the first step past them, a refused step moves nothing,
/// and freeing room lets the same step through (#1002).
#[test]
fn sim_world_refuses_past_ceiling() -> Result<(), Failure> {
    columns_and_config()?;
    let ids: Vec<_> = (1..=9_u64).map(|seq| mint(seq, seq)).collect();
    entities_ceiling(&ids)?;
    bytes_ceiling(&ids)?;
    value_ceiling(id_at(&ids, 0)?)?;
    operations_ceiling()?;
    payload_ceiling(id_at(&ids, 0)?)
}

/// Catches a bytes ceiling off by one (the fat finger at the edge): a transaction that fills the world to
/// exactly its ceiling passes, one byte more is refused with the exact numbers, and the refused step leaves
/// the transaction able to commit what it held (#1002).
#[test]
fn sim_world_tx_fills_bytes_ceiling_exactly() -> Result<(), Failure> {
    let tables = 2 * 3 * CELL_COST;
    let (mut edge, cols) = world("acme", 8, tables + 64)?;
    let (first, second) = (mint(1, 1), mint(2, 2));
    let mut tx = edge.begin();
    tx.spawn(first)?;
    tx.spawn(second)?;
    tx.set(cols.name, second, &name(40))?;
    tx.set(cols.name, first, &name(24))?;
    assert!(
        is_over(
            &tx.set(cols.name, first, &name(25)),
            Limit::Bytes,
            tables + 64,
            tables + 65
        ),
        "one byte past a full world passes"
    );
    tx.commit()?;
    assert_eq!(edge.bytes(), 64, "the world holds exactly its ceiling");
    Ok(())
}

/// Catches a bytes ceiling that values of no length walk around (the giant): entities that hold only empty
/// values still cost a cell in every column, so a world with room for a million entities and sixty-four
/// kibibytes stops at the slot that does not fit, with the exact numbers; a freed slot is reused at no new
/// cost; and the refused spawn moves nothing (#1002).
#[test]
fn sim_world_giant_and_empty_zero_byte_values_hit_a_ceiling() -> Result<(), Failure> {
    let ceiling = 1_usize << 16;
    let slot = 3 * CELL_COST;
    let fits = ceiling.checked_div(slot).ok_or(Failure::Empty)?;
    let (mut giant, cols) = world("acme", 1 << 20, ceiling)?;
    let mut stream = Stream::new(0x1002_6147);
    let mut ids = Vec::new();
    let mut tx = giant.begin();
    for seq in 1..=u64::try_from(fits)? {
        let id = mint(seq, stream.next());
        tx.spawn(id)?;
        tx.set(cols.name, id, &name(0))?;
        ids.push(id);
    }
    let past = mint(u64::MAX, stream.next());
    assert!(
        is_over(&tx.spawn(past), Limit::Bytes, ceiling, (fits + 1) * slot),
        "slot {} passes the ceiling with empty values only",
        fits + 1
    );
    tx.commit()?;
    assert_eq!(giant.live(), fits, "the slots that fit are alive");
    assert_eq!(giant.bytes(), 0, "every value was of no length");
    let full = giant.hash()?;
    let mut tx = giant.begin();
    assert!(
        is_over(&tx.spawn(past), Limit::Bytes, ceiling, (fits + 1) * slot),
        "a full table refuses a new slot in a new transaction"
    );
    drop(tx);
    assert_eq!(giant.hash()?, full, "a refused spawn moved the world");
    let victim = *ids.first().ok_or(Failure::Empty)?;
    let mut tx = giant.begin();
    tx.despawn(victim)?;
    tx.spawn(past)?;
    tx.commit()?;
    assert_eq!(giant.live(), fits, "the freed slot was reused");
    Ok(())
}
