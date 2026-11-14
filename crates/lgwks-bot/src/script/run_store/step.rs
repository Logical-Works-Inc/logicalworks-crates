//! The durable step: run a body once and remember its value across runs.

use std::future::Future;
use std::sync::Arc;

use crate::effect::RunId;
use crate::task::DefinitionIdentity;

use super::super::{FlowError, Scope, Site};
use super::{Durable, StoredValue, installed, installed_definition};

/// The definition a durable step records under.
///
/// The run's host-installed identity when one is installed, otherwise one derived
/// from `scope` and `step`. One resolution point, so a refusal [`remember_at`]
/// writes and a value [`remember`] writes cannot disagree about the definition
/// their run is using.
pub(crate) fn step_definition(scope: &Scope, step: &str) -> Arc<DefinitionIdentity> {
    match installed_definition(scope) {
        Some(installed) => installed,
        None => Arc::new(definition_of(scope, step)),
    }
}

/// Run `body` once and remember its value across runs of this step.
///
/// The public spelling of the durable step. `step` is the step's name and enters
/// a child scope for the key and the location; `body` is the work, and is
/// polled only when no completed record for this step is committed under the
/// current run. Without a host that installed a store, this is exactly `body`:
/// the value is returned and nothing is claimed to be durable.
///
/// `T` must be archivable through [`lgwks_std::wire`], which is what makes a
/// recorded value readable after the process that produced it is gone.
///
/// # Errors
///
/// Whatever `body` returns, or [`FlowError`] when the store refuses to read or
/// write the record.
///
/// # Examples
///
/// With no host store installed, `remember` is exactly its body:
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use lgwks_bot::script::{FlowError, Scope, Tenant, remember};
///
/// let scope = Scope::root(Tenant::new("acme")?);
/// let fetched = lgwks_bot::block_on(remember(&scope, "fetch", || async {
///     Ok::<_, FlowError>(41u32)
/// }))?;
/// assert_eq!(fetched, 41);
/// # Ok(())
/// # }
/// ```
pub async fn remember<T, Fut>(
    scope: &Scope,
    step: &str,
    body: impl FnOnce() -> Fut,
) -> Result<T, FlowError>
where
    T: lgwks_std::wire::Archive + Durable,
    Fut: Future<Output = Result<T, FlowError>>,
{
    let child = scope.enter(step)?;
    // The host-installed identity, or one derived from this scope's own facts when
    // no run above declared one. Derived rather than defaulted so two runs on one
    // host cannot claim one definition, and so a hand-built scope with no host
    // above it still records something that is stable for its own run.
    let definition = step_definition(scope, step);
    step_in(&child, &definition, body).await
}

/// [`remember`] for a `remember name:` that `script!` wrote at `site`.
///
/// The block is handed the step's own scope, so what it runs is keyed and
/// located under the step, and the trail entry carries `site`. Called by
/// `script!`.
///
/// # Errors
///
/// As [`remember`], located at the step.
#[doc(hidden)]
pub async fn remembered_at<T, F, Fut>(
    scope: &Scope,
    step: &str,
    site: &'static Site,
    body: F,
) -> Result<T, FlowError>
where
    T: lgwks_std::wire::Archive + Durable,
    F: FnOnce(Scope) -> Fut,
    Fut: Future<Output = Result<T, FlowError>>,
{
    let child = scope.enter_at(step, site)?;
    let definition = step_definition(scope, step);
    let inner = child.clone();
    let outcome = step_in(&child, &definition, move || body(inner)).await;
    child.settle(&outcome);
    outcome.map_err(|error| error.located(&child))
}

/// The definition a step records under when no host installed one.
///
/// The per-step fallback, and the reason it is a digest rather than a fixed
/// constant: the run id and the step's own path are both already facts this
/// store holds, so naming them together gives a stable identity for a flow that
/// never declared a revision — and it is deliberately *not* stable across two
/// different run ids, so a caller that resumes a run it does not hold cannot
/// inherit a definition from wherever it looked.
///
/// `pub(crate)` because it is derived once here rather than at each call site:
/// a caller that stages or appends a record by hand — the storage owner's own
/// tests do — needs the same identity a `remember` would have written, or the
/// record it commits is one no replay would recognise.
pub(crate) fn definition_of(scope: &Scope, step: &str) -> DefinitionIdentity {
    let mut hasher = lgwks_std::hash::Hasher::new();
    hasher.write_framed(b"lgwks.bot.definition.v1");
    hasher.write_framed(scope.tenant().as_str().as_bytes());
    hasher.write_framed(step.as_bytes());
    if let Some(run) = scope.run() {
        hasher.write_framed(run.id().to_hex().as_bytes());
    } else {
        hasher.write_framed(b"no-run");
    }
    DefinitionIdentity::new(scope.path(), u64::MAX, hasher.finalize(), 1)
}

/// [`remember`] against a scope the caller already entered.
async fn step_in<T, Fut>(
    scope: &Scope,
    definition: &DefinitionIdentity,
    body: impl FnOnce() -> Fut,
) -> Result<T, FlowError>
where
    T: lgwks_std::wire::Archive + Durable,
    Fut: Future<Output = Result<T, FlowError>>,
{
    remember_at(scope, scope.run(), definition, body).await
}

/// [`remember`] against a scope already carrying a run, or `remember_at` for a
/// resume where the caller supplies the run explicitly.
pub(crate) async fn remember_at<T, Fut>(
    scope: &Scope,
    run: Option<RunId>,
    definition: &DefinitionIdentity,
    body: impl FnOnce() -> Fut,
) -> Result<T, FlowError>
where
    T: lgwks_std::wire::Archive + Durable,
    Fut: Future<Output = Result<T, FlowError>>,
{
    let Some(records) = installed(scope) else {
        return body().await;
    };
    let Some(run) = run else {
        return body().await;
    };
    // Before the lookup, not after it. A record found under another definition is
    // a value this build never produced, and returning it is the failure T15
    // names; asking first means the refusal costs a hash-map probe rather than a
    // step body having already been trusted.
    //
    // Both failures arrive as themselves. A genuine drift is the typed
    // `Incompatible` arm carrying its axis, so a caller can tell whether it needs
    // a new run, a decision, a migration or an edit; a store that could not read
    // its own records arrives as the store's own error rather than as a claim
    // about the definition. Neither is a `Failed` reason string: that form could
    // express neither, and the caller had no way to repair a drift from it.
    //
    // `located_at` fills the step path on the refusal. The typed arm is built
    // with an empty location because it does not yet know the step, and a refusal
    // that keeps its constructor's empty path would name the run boundary instead
    // of the step that was about to replay.
    records
        .agrees(run, definition)
        .map_err(|error| error.located_at(scope.shared_path()))?;
    // The store keys records by the run-less form (#646): a record's identity
    // is the step's definition-level key, so a resumed run replays the records
    // a previous run wrote without a migration.
    let key = scope.record_key();
    let tenant = scope.tenant().as_str();

    if let Some(stored) = records.lookup(tenant, run, key)? {
        return decode(&stored);
    }

    // The body's own failure is located at *this* step rather than at the run's
    // boundary, so a ticket names the step a caller must look at rather than the
    // task that happened to contain it.
    let value = body()
        .await
        .map_err(|error| error.located_at(scope.shared_path()))?;
    let bytes = encode(&value).map_err(|error| error.located_at(scope.shared_path()))?;
    records
        .record(tenant, run, key, scope.path(), definition, bytes)
        .await?;
    Ok(value)
}

/// Decode a recorded value, locating a failure at the step it belongs to.
fn decode<T>(stored: &StoredValue) -> Result<T, FlowError>
where
    T: Durable,
{
    T::from_record(&stored.bytes).map_err(|cause| FlowError::Failed {
        at: std::sync::Arc::from(stored.path.as_str()),
        reason: format!("the value recorded for this step is not the type it returns: {cause}"),
    })
}

/// Archive a value the way the store frames it, so encode and decode agree.
fn encode<T>(value: &T) -> Result<Vec<u8>, FlowError>
where
    T: Durable,
{
    value.to_record().map_err(|cause| FlowError::Failed {
        at: std::sync::Arc::from(""),
        reason: format!("this step's value could not be archived: {cause}"),
    })
}
