//! A consumer's durable cursor in the tenant store, against a model and a saboteur.
//!
//! Source: a consumer's paging simulation (E35): a saved place survives a restart,
//! never moves back, and one tenant's place is never another's.

use std::collections::BTreeMap;
use std::path::PathBuf;

use lgwks_bot::tenant_store::{TenantStore, TenantStoreError};
use lgwks_std::seeded::Seeded;

/// A test's outcome: setup failures propagate instead of panicking.
type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A scratch directory for one store.
fn scratch(name: &str) -> Outcome<PathBuf> {
    let id = lgwks_std::id::Uuid::new_v4()?;
    let dir = std::env::temp_dir().join(format!("{name}-{id}"));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Catches a place lost at a reopen, or one that went back.
#[test]
fn sim_tenant_cursor_pulled_plug_a_place_survives_and_never_moves_back() -> Outcome {
    let dir = scratch("cursor-reopen")?;
    let store = TenantStore::open(&dir)?;
    assert_eq!(store.cursor("acme", "mailer")?, None);
    assert_eq!(store.advance_cursor("acme", "mailer", 0)?, 0);
    assert_eq!(store.cursor("acme", "mailer")?, Some(0), "zero is a place");
    assert_eq!(store.advance_cursor("acme", "mailer", 7)?, 7);
    drop(store);
    let store = TenantStore::open(&dir)?;
    assert_eq!(store.cursor("acme", "mailer")?, Some(7));
    assert!(matches!(
        store.advance_cursor("acme", "mailer", 6),
        Err(TenantStoreError::CursorBehind { held: 7, asked: 6 })
    ));
    assert_eq!(store.cursor("acme", "mailer")?, Some(7));
    Ok(())
}

/// Catches saving the same place twice writing a frame each time.
#[test]
fn sim_tenant_cursor_double_clicker_the_same_place_writes_nothing() -> Outcome {
    let dir = scratch("cursor-twice")?;
    let file = dir.join("tenant.store");
    let store = TenantStore::open(&dir)?;
    store.advance_cursor("acme", "mailer", 5)?;
    let length = std::fs::metadata(&file)?.len();
    assert_eq!(store.advance_cursor("acme", "mailer", 5)?, 5);
    assert_eq!(std::fs::metadata(&file)?.len(), length);
    Ok(())
}

/// Catches one tenant reading or moving another's place under one consumer name.
#[test]
fn sim_tenant_cursor_neighbour_two_tenants_share_no_place() -> Outcome {
    let dir = scratch("cursor-tenants")?;
    let store = TenantStore::open(&dir)?;
    store.advance_cursor("acme", "mailer", 90)?;
    assert_eq!(store.cursor("globex", "mailer")?, None);
    assert_eq!(store.advance_cursor("globex", "mailer", 3)?, 3);
    assert_eq!(store.cursor("acme", "mailer")?, Some(90));
    assert_eq!(store.cursor("acme", "other")?, None);
    Ok(())
}

/// Catches an unbounded count of consumers, and the limit binding on a known one.
#[test]
fn sim_tenant_cursor_giant_and_empty_a_tenant_holds_a_bounded_count() -> Outcome {
    let dir = scratch("cursor-limit")?;
    let store = TenantStore::open(&dir)?;
    for number in 0..64 {
        store.advance_cursor("acme", &format!("consumer-{number}"), 1)?;
    }
    assert!(matches!(
        store.advance_cursor("acme", "one-more", 1),
        Err(TenantStoreError::Limit {
            requested: 65,
            limit: 64
        })
    ));
    assert_eq!(store.advance_cursor("acme", "consumer-0", 2)?, 2);
    assert_eq!(store.advance_cursor("globex", "one-more", 1)?, 1);
    Ok(())
}

/// Catches the snapshot dropping cursors, and a damaged one losing a place.
#[test]
fn sim_tenant_cursor_saboteur_a_damaged_snapshot_costs_no_place() -> Outcome {
    let dir = scratch("cursor-snapshot")?;
    let store = TenantStore::open(&dir)?;
    for position in 0..600 {
        store.advance_cursor("acme", "mailer", position)?;
    }
    drop(store);
    let snapshot = dir.join("tenant.snapshot");
    assert!(snapshot.exists(), "600 frames pass the snapshot interval");
    let store = TenantStore::open(&dir)?;
    assert_eq!(
        store.cursor("acme", "mailer")?,
        Some(599),
        "through the snapshot"
    );
    drop(store);
    std::fs::write(&snapshot, b"not a snapshot")?;
    let store = TenantStore::open(&dir)?;
    assert_eq!(
        store.cursor("acme", "mailer")?,
        Some(599),
        "through the frames"
    );
    Ok(())
}

/// One seed's moves across reopens, held to a model, as a trace.
fn monkey(seed: u64) -> Outcome<BTreeMap<(u64, u64), u64>> {
    let mut dice = Seeded::from_seed(seed);
    let dir = scratch("cursor-monkey")?;
    let mut store = TenantStore::open(&dir)?;
    let mut model: BTreeMap<(u64, u64), u64> = BTreeMap::new();
    for _ in 0..60 {
        let (tenant, consumer) = (dice.below(3)?, dice.below(3)?);
        let (tenant_name, consumer_name) = (format!("t{tenant}"), format!("c{consumer}"));
        let place = model.get(&(tenant, consumer)).copied();
        match dice.below(3)? {
            0 => {
                let position = dice.below(100)?;
                let answer = store.advance_cursor(&tenant_name, &consumer_name, position);
                match place {
                    Some(at) if position < at => assert!(answer.is_err(), "seed {seed}"),
                    _ => {
                        assert_eq!(answer?, position, "seed {seed}");
                        model.insert((tenant, consumer), position);
                    }
                }
            }
            1 => assert_eq!(
                store.cursor(&tenant_name, &consumer_name)?,
                place,
                "seed {seed}"
            ),
            _ => {
                drop(store);
                store = TenantStore::open(&dir)?;
            }
        }
    }
    Ok(model)
}

/// Catches any drift from the model across reopens.
#[test]
fn sim_tenant_cursor_monkey_random_moves_match_the_model_and_replay_exactly() -> Outcome {
    for seed in 0..32 {
        assert_eq!(monkey(seed)?, monkey(seed)?, "seed {seed} did not replay");
    }
    Ok(())
}
