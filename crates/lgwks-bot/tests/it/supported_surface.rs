//! A consumer that names only the supported surface builds what the crate is for.
//!
//! Source: #1016 ("the crate root exports the supported surface and nothing else") and its proof
//! line: a consumer-shaped test that imports only `script!`, `try_join!`, `script`, `kernel` and
//! `app` and builds a durable two-step flow and a two-route app. Every `use` below is one of those
//! paths, so an item the root gave up that a consumer still needs fails this file to compile.
//!
//! | Test | What it pins |
//! |---|---|
//! | `a_durable_two_step_flow_runs_and_resumes_through_the_supported_paths` | a two-step task run on a stored host, then resumed on a fresh host over the same store, answers from the record and never re-runs the first step |
//! | `a_two_route_app_builds_and_checks_through_the_supported_paths` | an app with two routes and a granted capability passes its own check |

#![cfg(feature = "script")]

/// A test's result: an error fails it with the error's text.
#[cfg(any(feature = "ephemeral", feature = "edge"))]
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A stored run mints its run id from `lgwks_std::random`, which `ephemeral` brings.
#[cfg(feature = "ephemeral")]
#[test]
fn a_durable_two_step_flow_runs_and_resumes_through_the_supported_paths() -> TestResult {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use lgwks_bot::kernel::task::{Disposition, Host, task};
    use lgwks_bot::script::{FlowError, Scope, block_on, remember};

    use crate::scratch::Scratch;

    let scratch = Scratch::new("supported-surface")?;
    let first_runs = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&first_runs);
    let work = task("two-step", move |scope: Scope, value: u32| {
        let counter = Arc::clone(&counter);
        async move {
            let doubled = remember(&scope, "first", || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, FlowError>(value.saturating_mul(2))
                }
            })
            .await?;
            remember(&scope, "second", || async move {
                Ok::<_, FlowError>(doubled.saturating_add(1))
            })
            .await
        }
    })?;

    let host = Host::builder("acme")?.run_store(scratch.path())?.build()?;
    let report = block_on(host.run(&work, 20u32));
    assert_eq!(
        report.disposition(),
        Disposition::Succeeded,
        "{:?}",
        report.error()
    );
    assert_eq!(report.output(), Some(&41));
    let run = report.run_id().ok_or("a stored run names its run id")?;
    drop(report);
    drop(host);

    // A fresh host over the same store: the resume answers from the two records.
    let reopened = Host::builder("acme")?.run_store(scratch.path())?.build()?;
    let resumed = block_on(reopened.resume(run, &work, 20u32));
    assert_eq!(resumed.output(), Some(&41), "{:?}", resumed.error());
    assert_eq!(
        first_runs.load(Ordering::SeqCst),
        1,
        "the resume replayed the first step's record instead of running it again"
    );
    Ok(())
}

#[cfg(feature = "edge")]
#[test]
fn a_two_route_app_builds_and_checks_through_the_supported_paths() -> TestResult {
    use lgwks_bot::app::{App, Effect, Method};
    use lgwks_bot::kernel::cap::Cap;

    let app = App::new("supported")
        .grant(Cap::new("app.ask"))
        .route(Method::POST, "/echo", |_: &mut (), body: String| {
            vec![Effect::reply(200, body.into_bytes())]
        })
        .route(Method::GET, "/health", |_: &mut (), (): ()| {
            vec![Effect::reply(200, b"ok".to_vec())]
        });
    app.check()?;
    Ok(())
}
