//! Sealed pending approvals under a key fault (#1085).
//!
//! Source: issue #1085, found by the axis audit of #991. A host lists parked
//! approvals by opening each sealed step through the key provider. Before the
//! fix a provider refusal, transient included, dropped the step from the
//! listing with only a debug line, so a reviewer saw fewer approvals and no
//! error while the key service had a short fault. A transient error treated as
//! "nothing there" is the silent failure a person cannot see; the listing must
//! name the fault and list the approval again once the fault clears.
//!
//! The flow, the host, the sealed store and the provider are real; the fault
//! is the shared harness's `FaultingProvider`, the same one the sealed sims arm.

#![cfg(all(feature = "script", feature = "ephemeral", feature = "time"))]

use std::sync::Arc;

use lgwks_bot::script::{FlowError, PendingApproval, Scope};
use lgwks_bot::sealed::keys::{KeyError, KeyProvider};
use lgwks_bot::task::{Disposition, Host, RunStore, StoreError, task};

use crate::scratch::Scratch;
use crate::sealed_harness::{FaultingProvider, open_store};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Approvals the large-listing case parks for one tenant.
const PARKED: usize = 64;

lgwks_bot::script! {
    /// Park the payload for a human decision, then give it back.
    pub(crate) flow hold(payload: Vec<u8>) -> Vec<u8>:
        let approved = await approval hold within 30s:
            payload
        give back approved.payload().to_vec()
}

/// A host for `tenant` over the shared sealed `store`.
pub(crate) fn host_over(
    store: &RunStore,
    tenant: &str,
) -> Result<Host, Box<dyn std::error::Error>> {
    Ok(Host::builder(tenant)?.store(store.clone()).build()?)
}

/// Run `hold` on `payload` through `host` and return the parked id.
pub(crate) fn park(host: &Host, payload: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
    let holding = task("hold", |scope: Scope, payload: Vec<u8>| async move {
        hold(&scope, payload).await
    })?;
    let report = lgwks_bot::script::block_on(host.run(&holding, payload.to_vec()));
    if !matches!(report.disposition(), Disposition::Parked) {
        let refusal = format!(
            "the first run parks, got {:?}: {:?}",
            report.disposition(),
            report.error()
        );
        lgwks_std::trace::debug!(
            refusal = refusal.as_str(),
            "sealed pending fault: the first run did not park"
        );
        return Err(refusal.into());
    }
    let Some(error) = report.error() else {
        lgwks_std::trace::debug!("sealed pending fault: a parked run carried no error");
        return Err("a parked run names its id".into());
    };
    match *error {
        FlowError::Parked { ref id, .. } => Ok(id.clone()),
        _ => {
            lgwks_std::trace::debug!("sealed pending fault: a parked run named no id");
            Err("a parked run names its id".into())
        }
    }
}

/// Whether `error` is the store's typed transient key refusal.
pub(crate) fn transient_key_refusal(error: &FlowError) -> bool {
    match *error {
        FlowError::Store { ref source, .. } => matches!(
            **source,
            StoreError::SealedKey {
                source: KeyError::Transient { .. }
            }
        ),
        _ => false,
    }
}

/// Refuse the test unless `listed` is the typed transient key refusal: a
/// faulted listing that answers at all has shrunk silently.
fn expect_refused(name: &str, listed: Result<Vec<PendingApproval>, FlowError>) -> TestResult {
    match listed {
        Err(error) if transient_key_refusal(&error) => Ok(()),
        Err(error) => {
            lgwks_std::trace::debug!(name, error = ?error, "sealed pending fault: the wrong refusal");
            Err(format!("{name}: the listing names the transient key fault, got {error:?}").into())
        }
        Ok(listed) => {
            lgwks_std::trace::debug!(
                name,
                listed = listed.len(),
                "sealed pending fault: a faulted listing answered"
            );
            Err(format!(
                "{name}: a key fault never lists a shorter set as whole, {} listed",
                listed.len()
            )
            .into())
        }
    }
}

/// The ids `listed` names, in its order.
pub(crate) fn ids(listed: &[PendingApproval]) -> Vec<String> {
    listed.iter().map(|entry| entry.id().to_owned()).collect()
}

/// A sealed store over a fresh faulting provider, with the provider kept so
/// the test can arm it.
fn sealed_store(
    scratch: &Scratch,
) -> Result<(Arc<FaultingProvider>, RunStore), Box<dyn std::error::Error>> {
    let provider = Arc::new(FaultingProvider::open(&scratch.path().join("keys"))?);
    let keyed: Arc<dyn KeyProvider> = Arc::<FaultingProvider>::clone(&provider);
    let store = open_store(scratch.path(), keyed)?;
    Ok((provider, store))
}

/// A transient key fault makes the pending listing fail typed, never shrink;
/// once the fault clears the same approval is listed again under its id.
#[test]
fn sealed_impl_a_transient_key_fault_fails_the_pending_listing_typed() -> TestResult {
    let scratch = Scratch::new("sealed-pending-fault")?;
    let (provider, store) = sealed_store(&scratch)?;
    let host = host_over(&store, "acme")?;
    let id = park(&host, b"release-42")?;

    provider.fail_next_unwraps(1);
    expect_refused("acme", host.pending(100))?;

    assert_eq!(
        ids(&host.pending(100)?),
        vec![id],
        "the same approval is listed again"
    );
    Ok(())
}

/// One tenant's fault does not hide another tenant's approvals: while the
/// provider refuses, each tenant's listing fails typed; healed, each tenant
/// lists exactly its own approval.
#[test]
fn sealed_impl_a_key_fault_hides_no_tenant_approval() -> TestResult {
    let scratch = Scratch::new("sealed-pending-fault-tenants")?;
    let (provider, store) = sealed_store(&scratch)?;
    let acme = host_over(&store, "acme")?;
    let globex = host_over(&store, "globex")?;
    let acme_id = park(&acme, b"acme-release")?;
    let globex_id = park(&globex, b"globex-release")?;

    provider.fail_all_unwraps(true);
    expect_refused("acme", acme.pending(100))?;
    expect_refused("globex", globex.pending(100))?;
    provider.fail_all_unwraps(false);

    assert_eq!(
        ids(&acme.pending(100)?),
        vec![acme_id],
        "acme lists only its own"
    );
    assert_eq!(
        ids(&globex.pending(100)?),
        vec![globex_id],
        "globex lists only its own"
    );
    Ok(())
}

/// A host reopened over the same sealed store while the fault holds fails its
/// listing typed, then lists what the previous process parked once healed.
#[test]
fn sealed_impl_a_reopened_host_lists_the_approval_after_the_fault() -> TestResult {
    let scratch = Scratch::new("sealed-pending-fault-reopen")?;
    let (provider, store) = sealed_store(&scratch)?;
    let id = park(&host_over(&store, "acme")?, b"before-restart")?;
    drop(store);

    let keyed: Arc<dyn KeyProvider> = Arc::<FaultingProvider>::clone(&provider);
    let reopened = open_store(scratch.path(), keyed)?;
    let host = host_over(&reopened, "acme")?;
    provider.fail_all_unwraps(true);
    expect_refused("reopened", host.pending(100))?;
    provider.fail_all_unwraps(false);

    assert_eq!(
        ids(&host.pending(100)?),
        vec![id],
        "the restart lists the earlier park"
    );
    Ok(())
}

/// With many approvals parked, one refused unwrap fails the whole listing
/// rather than returning all but one; healed, every approval is listed.
#[test]
fn sealed_impl_a_fault_in_a_large_listing_fails_it_whole() -> TestResult {
    let scratch = Scratch::new("sealed-pending-fault-large")?;
    let (provider, store) = sealed_store(&scratch)?;
    let host = host_over(&store, "acme")?;
    let mut parked = Vec::with_capacity(PARKED);
    for index in 0..PARKED {
        parked.push(park(&host, format!("release-{index}").as_bytes())?);
    }

    provider.fail_next_unwraps(1);
    expect_refused("large", host.pending(PARKED))?;

    let mut listed = ids(&host.pending(PARKED)?);
    listed.sort();
    parked.sort();
    assert_eq!(
        listed, parked,
        "every parked approval is listed once healed"
    );
    Ok(())
}
