//! The journal and sweeper half of `estate sealed`: a sealed
//! [`FileJournal`] is appended, read back, shredded once and again, swept by
//! TTL and erased by tenant; then the owned [`Sweeper`] shreds an expired
//! run-store record on an injected clock and shuts down cleanly.
//!
//! Every step is asserted as it is printed, so a clean exit is a proved
//! journey, exactly as in the run-store half (`sealed.rs`).

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use lgwks_bot::journal::{EffectEvent, EffectJournal as _, FileJournal};
use lgwks_bot::sealed::journal::{SealedJournalRead, ShredOutcome};
use lgwks_bot::sealed::keys::KeyProvider;
use lgwks_bot::sealed::sweep::{SweepTarget, Sweeper};
use lgwks_bot::task::RunStore;

use super::sealed::{SealedError, refused, say};

/// The fixed world the journal half's events belong to, distinct from every
/// other estate journey so its keys never alias theirs.
const RUN_HEX: &str = "a1a2a3a4a5a6a7a8a9aaabacadaeafb0";
/// The action every journey event names.
const ACTION_HEX: &str = "b1b2b3b4b5b6b7b8b9babbbcbdbebfc0";
/// The environment every journey event is fenced against.
const ENVIRONMENT_HEX: &str = "c1c2c3c4c5c6c7c8c9cacbcccdcecfd0";
/// The flow revision and input digest every journey event binds.
const BINDING_HEX: &str = "d0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebecedeeef";

/// The admission event for attempt `attempt` of the journey's one action.
fn admitted(attempt: u32) -> Result<EffectEvent, SealedError> {
    use lgwks_bot::effect::{
        ActionDigest, ActionId, AttemptId, EffectIdentity, EnvironmentEpoch, EnvironmentId,
        FlowRevision, RunId,
    };
    let fail = |what: &'static str| move |cause: String| refused(what)(cause);
    let identity = EffectIdentity::new(
        RunId::from_hex(RUN_HEX).map_err(|error| fail("the journal run")(error.to_string()))?,
        EnvironmentId::from_hex(ENVIRONMENT_HEX)
            .map_err(|error| fail("the journal environment")(error.to_string()))?,
        FlowRevision::from_tagged("blake3_256", BINDING_HEX)
            .map_err(|error| fail("the journal flow")(error.to_string()))?,
    );
    let key = identity.key(
        ActionId::from_hex(ACTION_HEX)
            .map_err(|error| fail("the journal action")(error.to_string()))?,
        AttemptId::from_decimal(&attempt.to_string())
            .map_err(|error| fail("the journal attempt")(error.to_string()))?,
        ActionDigest::from_tagged("blake3_256", BINDING_HEX)
            .map_err(|error| fail("the journal digest")(error.to_string()))?,
        EnvironmentEpoch::from_decimal("1")
            .map_err(|error| fail("the journal epoch")(error.to_string()))?,
    );
    Ok(EffectEvent::IntentAdmitted { key })
}

/// The live and erased frames of `journal`, counted from its sealed reads,
/// with the sequence of every live frame in commit order.
fn census(journal: &FileJournal) -> Result<(Vec<u64>, u64), SealedError> {
    let reads = journal
        .sealed_entries()
        .map_err(|error| refused("the sealed journal read")(error.to_string()))?;
    let mut live = Vec::with_capacity(reads.len());
    let mut erased = 0_u64;
    for read in reads {
        match read {
            SealedJournalRead::Live { position, .. } => live.push(position.sequence()),
            SealedJournalRead::Erased { .. } => erased = erased.saturating_add(1),
            _ => {
                let refusal = SealedError::Refused(
                    "a sealed frame read as neither live nor erased".to_owned(),
                );
                lgwks_std::trace::debug!(error = ?refusal, "estate-sealed: an unknown sealed read");
                return Err(refusal);
            }
        }
    }
    Ok((live, erased))
}

/// Refuse with `why` unless `held`, saying `line` when it is.
fn require(out: &mut impl Write, held: bool, line: &str, why: String) -> Result<(), SealedError> {
    if held {
        return say(out, line);
    }
    let refusal = SealedError::Refused(why);
    lgwks_std::trace::debug!(error = ?refusal, "estate-sealed: the journal half deviated");
    Err(refusal)
}

/// Prove the journal half over `dir` with `provider`: append three sealed
/// events (the third expiring), shred the first twice, sweep the expired
/// one, erase the tenant, and verify the chain over the tombstones.
pub(crate) fn prove_journal(
    dir: &Path,
    provider: &Arc<dyn KeyProvider>,
    out: &mut impl Write,
) -> Result<(), SealedError> {
    let mut journal =
        FileJournal::open_active_sealed(dir.join("ledger.jrnl"), Arc::clone(provider), "acme")
            .map_err(|error| refused("the sealed ledger")(error.to_string()))?;
    require(
        out,
        journal.is_sealed(),
        "sealed: the ledger opens sealed",
        "the ledger opened plain".to_owned(),
    )?;
    for (attempt, expires_at) in [(1, None), (2, None), (3, Some(Duration::from_secs(1)))] {
        let tail = journal.tail();
        journal
            .compare_and_append_sealed(tail, &admitted(attempt)?, expires_at)
            .map_err(|error| refused("a sealed ledger append")(error.to_string()))?;
    }
    let (live, _) = census(&journal)?;
    require(
        out,
        live.len() == 3,
        "sealed: the ledger holds three live events",
        format!("the ledger holds {} live events, not 3", live.len()),
    )?;
    let first = live
        .first()
        .copied()
        .ok_or_else(|| SealedError::Refused("the ledger holds no first event".to_owned()))?;
    let once = journal
        .shred_sealed(first)
        .map_err(|error| refused("the ledger shred")(error.to_string()))?;
    let twice = journal
        .shred_sealed(first)
        .map_err(|error| refused("the ledger re-shred")(error.to_string()))?;
    let idempotent = matches!(
        (once, twice),
        (
            ShredOutcome::Shredded { .. },
            ShredOutcome::AlreadyErased { .. }
        )
    );
    require(
        out,
        idempotent,
        "sealed: a second ledger shred is a no-op",
        format!("the shreds answered {once:?} then {twice:?}"),
    )?;
    let swept = journal
        .sweep_sealed(Duration::from_secs(2))
        .map_err(|error| refused("the ledger sweep")(error.to_string()))?;
    require(
        out,
        swept == 1,
        "sealed: the ledger sweep shredded exactly the expired event",
        format!("the ledger sweep shredded {swept}, not 1"),
    )?;
    let erased = journal
        .erase_sealed_tenant()
        .map_err(|error| refused("the ledger erasure")(error.to_string()))?;
    let (still_live, tombstones) = census(&journal)?;
    require(
        out,
        erased == 1 && still_live.is_empty() && tombstones == 3,
        "sealed: tenant erasure leaves three ledger tombstones",
        format!(
            "erasure shredded {erased}, leaving {} live and {tombstones} erased",
            still_live.len()
        ),
    )?;
    journal
        .verify_sealed_chain()
        .map_err(|error| refused("the ledger chain")(error.to_string()))?;
    say(out, "sealed: the ledger chain verifies over its tombstones")
}

/// Prove the owned sweeper over a tenant-named run store in `dir`: one
/// expiring record, a virtual clock moved past its expiry, and the record
/// read as erased before the sweeper shuts down.
pub(crate) fn prove_sweeper(
    dir: &Path,
    provider: &Arc<dyn KeyProvider>,
    out: &mut impl Write,
) -> Result<(), SealedError> {
    use lgwks_bot::clock::Clock;
    use lgwks_bot::effect::RunId;
    use lgwks_bot::script::{Scope, Tenant};
    use lgwks_bot::sealed::runstore::SealedStepRead;
    use lgwks_bot::task::{DefinitionIdentity, SealedAppend};

    let store = RunStore::open_in_sealed(&dir.join("tenants"), "initech", Arc::clone(provider))
        .map_err(|error| refused("the initech store")(error.to_string()))?;
    require(
        out,
        store.is_sealed(),
        "sealed: the initech store opens sealed",
        "the initech store opened plain".to_owned(),
    )?;
    let run = RunId::from_hex("e1e2e3e4e5e6e7e8e9eaebecedeeeff0")
        .map_err(|error| refused("the initech run")(error.to_string()))?;
    let key = Scope::root(
        Tenant::new("initech").map_err(|error| refused("the initech tenant")(error.to_string()))?,
    )
    .enter("sealed-ttl")
    .map_err(|error| refused("the initech step")(error.to_string()))?
    .key();
    let definition = DefinitionIdentity::new(
        "estate-sealed-ttl",
        1,
        lgwks_std::hash::blake3(b"lgwks-estate-sealed-ttl-v1"),
        1,
    );
    store
        .append_with_expiry(SealedAppend::new(
            "initech",
            run,
            key,
            "sealed-ttl",
            &definition,
            b"an expiring secret".to_vec(),
            Some(Duration::from_secs(5)),
        ))
        .map_err(|error| refused("the initech append")(error.to_string()))?;
    let early = SweepTarget::sweep_once(&store, Duration::from_secs(1))
        .map_err(|error| refused("the early sweep")(error.to_string()))?;
    require(
        out,
        early == 0,
        "sealed: a sweep before the expiry shreds nothing",
        format!("an early sweep shredded {early}"),
    )?;
    let clock = Clock::virtual_at(Duration::from_secs(1));
    clock
        .advance(Duration::from_secs(10))
        .map_err(|error| refused("the virtual clock")(error.to_string()))?;
    let watched = store;
    let erased = lgwks_bot::block_on(async move {
        let sweeper = Sweeper::spawn(watched.clone(), clock, Duration::from_millis(5));
        let seen = lgwks_bot::rt::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(SealedStepRead::Erased { .. }) = watched.read_sealed("initech", run, key)
                {
                    break;
                }
                lgwks_bot::rt::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        sweeper.shutdown().await;
        seen
    });
    require(
        out,
        erased,
        "sealed: the owned sweeper shredded the expired record and shut down",
        "the sweeper never shredded the expired record within 5 s".to_owned(),
    )
}
