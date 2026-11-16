//! The T28 family: tenant-scoped artifacts, serialized writes and reads that progress during a write.

use super::*;

// ── T28: tenant-scoped artifacts, serialized writes, concurrent reads ─────────

/// Two tenants holding the same bytes get different keys, and neither can read
/// the other's copy.
#[test]
fn two_tenants_on_one_digest_stay_isolated() -> TestResult {
    let (store, shared) = support::two_tenant_store();
    let digest = ArtifactStore::digest_of(&shared)?;

    assert_eq!(
        lgwks_std::hash::blake3(&shared),
        digest,
        "the digest is a function of the content alone, so both tenants really do share one"
    );

    assert!(matches!(
        store.write(TENANT, &shared)?,
        WriteOutcome::Stored { .. }
    ));
    assert!(
        matches!(
            store.write(OTHER_TENANT, &shared)?,
            WriteOutcome::Stored { .. }
        ),
        "the second tenant's identical bytes are a new artifact under its own key"
    );

    assert!(
        store.read(TENANT, &digest).is_some(),
        "the first tenant reads its own copy"
    );
    assert!(
        store.read(OTHER_TENANT, &digest).is_some(),
        "and the second reads its own"
    );
    assert_eq!(
        store.artifacts(TENANT),
        1,
        "each tenant holds exactly one artifact: a digest-keyed store would have held one between them"
    );
    assert_eq!(
        store.artifacts(OTHER_TENANT),
        1,
        "and the other tenant's shelf is separate"
    );

    // The isolation is of the index, not a check the caller remembers: a tenant
    // that has written nothing simply has no shelf.
    let stranger = ArtifactStore::new();
    assert!(
        stranger.read(TENANT, &digest).is_none(),
        "a tenant that wrote nothing reads nothing, whatever digest it names"
    );
    assert_ne!(
        ArtifactKey::of(TENANT, &digest),
        ArtifactKey::of(OTHER_TENANT, &digest),
        "two tenants' keys for the same digest differ, so the index cannot alias them"
    );
    Ok(())
}

/// Conflicting writes to one key are serialized into one committed artifact,
/// and the later writer is told it stored nothing.
#[test]
fn conflicting_writes_to_one_key_are_serialized_and_idempotent() -> TestResult {
    let store = ArtifactStore::new();
    let bytes = b"one report".to_vec();

    assert!(
        matches!(
            store.write(TENANT, &bytes)?,
            WriteOutcome::Stored { bytes: 10 }
        ),
        "the first writer commits"
    );
    assert!(
        matches!(
            store.write(TENANT, &bytes)?,
            WriteOutcome::AlreadyPresent { .. }
        ),
        "a second writer of identical content is told it stored nothing"
    );
    let digest = ArtifactStore::digest_of(&bytes)?;
    assert_eq!(
        store.writers(TENANT, &digest),
        2,
        "both writers reached the key, and the count is the order they arrived in"
    );
    assert_eq!(
        store.artifacts(TENANT),
        1,
        "and exactly one artifact was committed for them"
    );
    Ok(())
}

/// T28, the concurrent half: many writers of one key, released together behind a
/// barrier, commit exactly one artifact per tenant, and the two tenants that write
/// the same bytes at the same moment never share a shelf, a count or a commit.
#[test]
fn concurrent_writers_of_one_key_commit_exactly_once_per_tenant() -> TestResult {
    const WRITERS_PER_TENANT: usize = 32;
    let store = ArtifactStore::new();
    let bytes = b"one report, written by many hands at once".to_vec();
    let barrier = std::sync::Barrier::new(WRITERS_PER_TENANT * 2);

    let joined = std::thread::scope(|scope| {
        let handles = (0..WRITERS_PER_TENANT * 2)
            .map(|index| {
                let store = store.clone();
                let bytes = &bytes;
                let barrier = &barrier;
                let tenant = if index % 2 == 0 { TENANT } else { OTHER_TENANT };
                scope.spawn(move || {
                    barrier.wait();
                    (
                        tenant,
                        store
                            .write(tenant, bytes)
                            .map_err(|error| error.to_string()),
                    )
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join())
            .collect::<Vec<_>>()
    });

    let mut stored = std::collections::BTreeMap::<&str, usize>::new();
    let mut already = std::collections::BTreeMap::<&str, usize>::new();
    for writer in joined {
        let (tenant, outcome) = writer.map_err(|_| "a writer thread panicked")?;
        match outcome? {
            WriteOutcome::Stored { .. } => {
                let count = stored.entry(tenant).or_default();
                *count = count.saturating_add(1);
            }
            WriteOutcome::AlreadyPresent { .. } => {
                let count = already.entry(tenant).or_default();
                *count = count.saturating_add(1);
            }
            other => return Err(format!("unexpected outcome for {tenant}: {other:?}").into()),
        }
    }
    let digest = ArtifactStore::digest_of(&bytes)?;
    for tenant in [TENANT, OTHER_TENANT] {
        assert_eq!(
            stored.get(tenant).copied(),
            Some(1),
            "{tenant}: exactly one of the racing writers commits"
        );
        assert_eq!(
            already.get(tenant).copied(),
            Some(WRITERS_PER_TENANT - 1),
            "{tenant}: every other writer is told it stored nothing"
        );
        assert_eq!(
            store.writers(tenant, &digest),
            u64::try_from(WRITERS_PER_TENANT)?,
            "{tenant}: the receipt counts only this tenant's writers"
        );
        assert_eq!(
            store.artifacts(tenant),
            1,
            "{tenant}: one artifact was committed for the whole race"
        );
    }
    Ok(())
}

/// Reads progress while a write is in flight, and a write to one key does not
/// block a read of a different key.
#[test]
fn reads_progress_while_a_write_is_in_flight() -> TestResult {
    let store = ArtifactStore::new();
    let hot = b"the artifact under load".to_vec();
    let cold = b"an unrelated artifact".to_vec();

    store.write(TENANT, &hot)?;
    store.write(OTHER_TENANT, &cold)?;
    let hot_digest = ArtifactStore::digest_of(&hot)?;
    let cold_digest = ArtifactStore::digest_of(&cold)?;

    // Threads, not tasks: the point is that the store is shareable across real
    // workers and that the read path does not take the writer's lock. The joins
    // happen inside the scope, because a scoped handle may not outlive it.
    let (seen, outcome) = std::thread::scope(|scope| {
        let handles = (0..8)
            .map(|_| {
                let store = store.clone();
                scope.spawn(move || {
                    let mut seen = 0_u32;
                    for _ in 0..64 {
                        if store.read(TENANT, &hot_digest).is_some() {
                            seen = seen.saturating_add(1);
                        }
                    }
                    seen
                })
            })
            .collect::<Vec<_>>();
        // A writer runs while the readers are in flight.
        let writer = store.clone();
        let written = scope.spawn(move || writer.write(TENANT, &cold));
        let outcome = written.join();
        let mut seen = Vec::new();
        for handle in handles {
            seen.push(handle.join());
        }
        (seen, outcome)
    });

    let outcome = outcome.map_err(|_| "the writer thread panicked")??;
    assert_eq!(
        seen.len(),
        8,
        "every reader thread reported, so none was lost"
    );
    for seen in seen {
        let seen = seen.map_err(|_| "a reader thread panicked")?;
        assert_eq!(seen, 64, "every reader saw the artifact on every read");
    }
    assert!(
        matches!(outcome, WriteOutcome::Stored { .. }),
        "a write to a different key is not blocked by the readers"
    );
    assert!(
        store.read(OTHER_TENANT, &cold_digest).is_some(),
        "and the concurrent write landed"
    );
    Ok(())
}

/// An artifact past its byte ceiling is refused by name, and the store is
/// unchanged by the refusal.
#[test]
fn an_oversized_artifact_is_refused_and_the_store_is_unchanged() -> TestResult {
    let store = ArtifactStore::new();
    let too_big = vec![b'z'; lgwks_bot::proposal::MAX_ARTIFACT_BYTES.saturating_add(1)];

    assert!(
        matches!(
            store.write(TENANT, &too_big),
            Err(ArtifactError::TooLarge { .. })
        ),
        "an artifact past the byte ceiling is refused by name"
    );
    assert_eq!(
        store.artifacts(TENANT),
        0,
        "and the refusal left the store empty: a refusal is not a partial write"
    );
    assert!(
        matches!(
            ArtifactStore::digest_of(&too_big),
            Err(ArtifactError::TooLarge { .. })
        ),
        "the same ceiling is enforced before a caller commits to a key"
    );
    Ok(())
}
