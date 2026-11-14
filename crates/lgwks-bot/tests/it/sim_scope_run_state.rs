//! Simulation family: a run's store, definition and effect policy ride on its
//! `Scope` (#909).
//!
//! A task-local does not cross a spawn, and
//! [`join_all_bounded`](lgwks_bot::rt::task::join_all_bounded) spawns every
//! input. Before this change the run's store, its definition identity and its
//! effect policy lived in task-locals, so a step polled on a spawned task read
//! none of them:
//!
//! | Test | Bug it catches |
//! |---|---|
//! | `a_spawned_act_over_an_ephemeral_store_is_refused` | `act` skipped `admit_external_handoff`, so an external effect ran over an ephemeral store |
//! | `a_spawned_remember_records_and_replays` | `remember` ran its body unrecorded and could not replay it on a resume |
//! | `a_spawned_step_keeps_a_custom_effect_policy` | `gate_approval` fell back to `DefaultPolicy`, so a stricter policy stopped applying |
//!
//! A script word's future is deliberately not `Send` (it carries a non-`Send`
//! store write), so `join_all_bounded` cannot take one directly. Each spawned
//! input hands the word to `lgwks_std::task::spawn_blocking`, whose own OS
//! thread reports through a future: that is a spawn like any other, and no
//! task-local travels across it either.

#![cfg(all(
    feature = "script",
    feature = "rt",
    feature = "sync",
    feature = "ephemeral",
    feature = "testing",
    feature = "ecs",
    feature = "wire"
))]

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lgwks_bot::rt::task::join_all_bounded;
use lgwks_bot::script::{
    EffectDisposition, EffectPolicy, FlowError, Scope, act, remember, with_effect_policy,
};
use lgwks_bot::task::{Disposition, Host, StoreError, task};
use lgwks_bot::verb::EffectLifetime;
use lgwks_bot::{Action, Auth, BotError, Cap, Execute, domains};
use lgwks_std::task::spawn_blocking;

use crate::memory_records::MemoryRecords;
use crate::scratch::Scratch;
use crate::spec_materialize::SPEC_DOMAINS;

/// What every fallible test here returns, and a host builder's own result.
type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// The tenant every run belongs to.
const TENANT: &str = "acme";

/// The task name every run records under.
const TASK: &str = "fan";

/// How many spawned steps one fan-out drives.
const WIDTH: usize = 4;

/// How many spawned steps run at once, the fan-out's own bound.
const FAN_LIMIT: usize = 4;

/// Run `step` on a thread of its own and await its answer.
///
/// `join_all_bounded` spawns only `Send` futures, and a script word's future is
/// deliberately not `Send`, so the word rides a `spawn_blocking` job instead:
/// its own OS thread, reporting through a future. No task-local crosses it,
/// which is the spawn boundary these tests need.
async fn on_a_spawned_thread<T, F>(step: F) -> Result<T, FlowError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, FlowError> + Send + 'static,
{
    spawn_blocking(step).await
}

/// Drive `width` spawned acts of `action`, each on its own thread, and collect
/// their answers.
async fn spawned_acts(scope: Scope, width: usize, action: &'static str) -> Result<(), FlowError> {
    let steps = (0..width).map(|index| {
        let scope = scope.clone();
        async move {
            on_a_spawned_thread(move || {
                lgwks_bot::block_on(async move {
                    let child = scope.item("act", index)?;
                    act(&child, action, "label", 1_u16).await.map(drop)
                })
            })
            .await
        }
    });
    join_all_bounded(FAN_LIMIT, steps)
        .await
        .into_iter()
        .collect::<Result<Vec<()>, FlowError>>()
        .map(drop)
}

/// Whether `error` is the store's ephemeral-store refusal.
fn promise_unmet(error: Option<&FlowError>) -> bool {
    let Some(error) = error else {
        return false;
    };
    match *error {
        FlowError::Store { ref source, .. } => {
            matches!(**source, StoreError::PromiseUnmet { .. })
        }
        _ => false,
    }
}

#[test]
fn a_spawned_act_over_an_ephemeral_store_is_refused() -> TestResult {
    let ephemeral = || -> TestResult<Host> {
        Ok(Host::builder(TENANT)?
            .domains(&SPEC_DOMAINS)
            .records(Arc::new(MemoryRecords::default()))
            .build()?)
    };

    // The same act, unspawned: the answer the spawned step must match.
    let body = task(TASK, |scope: Scope, (): ()| async move {
        let child = scope.enter("act")?;
        act(&child, "test::page", "label", 1_u16).await.map(drop)
    })?;
    let direct = lgwks_bot::block_on(ephemeral()?.run(&body, ()));
    assert!(
        promise_unmet(direct.error()),
        "an unspawned act over an ephemeral store is refused: {:?}",
        direct.result()
    );

    let fanned = task(TASK, |scope: Scope, width: usize| async move {
        spawned_acts(scope, width, "test::page").await
    })?;
    let report = lgwks_bot::block_on(ephemeral()?.run(&fanned, WIDTH));
    assert!(
        promise_unmet(report.error()),
        "a spawned act over an ephemeral store must be refused exactly as the \
         unspawned one is, got {:?}",
        report.result()
    );
    Ok(())
}

#[test]
fn a_spawned_remember_records_and_replays() -> TestResult {
    let scratch = Scratch::new("spawned-remember")?;
    let calls = Arc::new(AtomicUsize::new(0));
    let work = task(
        TASK,
        |scope: Scope, (calls, width): (Arc<AtomicUsize>, usize)| async move {
            let steps = (0..width).map(|index| {
                let scope = scope.clone();
                let calls = Arc::clone(&calls);
                async move {
                    on_a_spawned_thread(move || {
                        lgwks_bot::block_on(async move {
                            let child = scope.item("remembered", index)?;
                            remember(&child, "value", || async move {
                                calls.fetch_add(1, Ordering::SeqCst);
                                u32::try_from(index)
                                    .map_err(|why| FlowError::failed(why.to_string()))
                            })
                            .await
                        })
                    })
                    .await
                }
            });
            let values = join_all_bounded(FAN_LIMIT, steps)
                .await
                .into_iter()
                .collect::<Result<Vec<u32>, FlowError>>()?;
            Ok(values
                .into_iter()
                .fold(0_u32, |sum, value| sum.saturating_add(value)))
        },
    )?;

    let host = Host::builder(TENANT)?.run_store(scratch.path())?.build()?;
    let identity = host.definition(TASK, 1, None, 1);
    let first = lgwks_bot::block_on(host.run_under(&identity, &work, (Arc::clone(&calls), WIDTH)));
    assert_eq!(
        first.disposition(),
        Disposition::Succeeded,
        "the first run records every spawned step: {:?}",
        first.error()
    );
    let run = first.run_id().ok_or("a stored run names its run id")?;
    let recorded = first.output().copied();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        WIDTH,
        "each spawned body ran once on the first run"
    );
    drop(first);
    drop(host);

    let reopened = Host::builder(TENANT)?.run_store(scratch.path())?.build()?;
    let replayed = lgwks_bot::block_on(reopened.resume_under(
        run,
        &identity,
        &work,
        (Arc::clone(&calls), WIDTH),
    ));
    assert_eq!(
        replayed.disposition(),
        Disposition::Succeeded,
        "the resume replays every spawned step: {:?}",
        replayed.error()
    );
    assert_eq!(
        replayed.output().copied(),
        recorded,
        "the resume gives back the recorded values"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        WIDTH,
        "no spawned body ran again on the resume"
    );
    Ok(())
}

/// A local action that archives its input, so a policy that gates a local
/// effect has bytes to name.
struct LocalPage;

impl LocalPage {
    /// Build from any target.
    fn from_target(_target: &str) -> Result<Action, BotError> {
        Ok(Action::new(Self))
    }
}

impl Execute for LocalPage {
    type Input = u16;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::Local
    }

    fn archive_input(&self, input: &u16) -> Option<Vec<u8>> {
        match lgwks_bot::script::Durable::to_record(input) {
            Ok(bytes) => Some(bytes),
            Err(why) => {
                lgwks_std::trace::debug!(error = ?why, "archive_input: the local action's input did not archive");
                None
            }
        }
    }

    async fn execute_action(&self, call: (Auth, &u16)) -> Result<(), BotError> {
        call.0.check(&[])?;
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "test::local_page"
    }
}

domains! {
    /// A local action that archives its input, for the policy family.
    pub(crate) GATED_DOMAINS {
        observe {}
        execute {
            "test::local_page" => LocalPage::from_target,
        }
    }
}

/// A policy that gates every action, whatever its lifetime.
struct GateEveryLifetime;

impl EffectPolicy for GateEveryLifetime {
    fn disposition(&self, _lifetime: EffectLifetime) -> EffectDisposition {
        EffectDisposition::NeedsConfirmation
    }
}

#[test]
fn a_spawned_step_keeps_a_custom_effect_policy() -> TestResult {
    let host = Host::builder(TENANT)?.domains(&GATED_DOMAINS).build()?;
    let fanned = task(TASK, |scope: Scope, width: usize| async move {
        spawned_acts(scope, width, "test::local_page").await
    })?;
    let report = lgwks_bot::block_on(with_effect_policy(
        GateEveryLifetime,
        host.run(&fanned, WIDTH),
    ));
    assert!(
        matches!(report.error(), Some(FlowError::NeedsApproval { .. })),
        "a spawned Local act must still answer to the run's custom policy, got {:?}",
        report.result()
    );
    Ok(())
}
