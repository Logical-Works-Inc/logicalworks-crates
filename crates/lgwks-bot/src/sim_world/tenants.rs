//! Two tenants on the same ids see and spend nothing of each other (#1002, ADR 0053 decision 3).

use super::{Driver, Failure};
use crate::world::error::WorldError;

/// Catches a world, hash, change set or snapshot that is not bound to its tenant (the neighbour): two
/// tenants run the same seeded writes on the same ids and hold the same cells, yet their hashes differ; more
/// writes in one leave the other's hash alone; an id alive in one is `NoSuchEntity` in the other; and a
/// change set or a snapshot of one is `TenantMismatch` in the other, which stays as it was (#1002).
#[test]
fn sim_world_two_tenants_same_ids() -> Result<(), Failure> {
    let mut acme = Driver::new(0x1002_7e11, "acme", 20_000, 1 << 24)?;
    let mut globex = Driver::new(0x1002_7e11, "globex", 20_000, 1 << 24)?;
    acme.run(3_000)?;
    globex.run(3_000)?;
    assert_eq!(
        (acme.world.tenant(), globex.world.tenant()),
        ("acme", "globex"),
        "each world names its tenant"
    );
    assert_eq!(
        acme.model.cells, globex.model.cells,
        "the same writes hold the same cells"
    );
    assert_ne!(
        acme.world.hash()?,
        globex.world.hash()?,
        "equal cells under two tenants hash alike"
    );
    let before = globex.world.hash()?;
    acme.run(6_000)?;
    assert_eq!(
        globex.world.hash()?,
        before,
        "writes in one tenant moved the other"
    );
    let newcomer = *acme
        .live
        .iter()
        .find(|id| !globex.model.cells.contains_key(*id))
        .ok_or(Failure::Check("acme holds an id globex does not"))?;
    assert!(
        matches!(
            globex.world.raw(0, &newcomer),
            Err(WorldError::NoSuchEntity { .. })
        ),
        "an id of one tenant is not alive in the other"
    );
    let log = acme
        .log
        .last()
        .ok_or(Failure::Check("acme logged a change set"))?;
    assert!(
        matches!(globex.world.replay(log), Err(WorldError::TenantMismatch)),
        "a change set crosses to another tenant"
    );
    let snapshot = acme.world.snapshot_since(0)?;
    assert!(
        matches!(
            globex.world.restore(&[snapshot.as_slice()]),
            Err(WorldError::TenantMismatch)
        ),
        "a snapshot crosses to another tenant"
    );
    assert_eq!(globex.world.hash()?, before, "the refusals moved the world");
    globex.check_model()?;
    acme.check_model()
}
