//! The repair family: a blocked request records no verdict until the repair that unblocks it settles it.

use super::*;

/// A blocked request is not settled, and the repair that unblocks it is.
///
/// `Blocked` is not the request's verdict — a repair can still move it — so the
/// first submission records none, and a repeat of the key is `InFlight` with the
/// recorded analysis never re-run. The repair is the attempt that reaches the
/// verdict, so it must record one: before `Host::repair` settled requests, the
/// fourth submission below answered `InFlight` forever after a successful repair,
/// and only a separate `Host::resume` could have made the key reattach. The
/// reopened host is the control that the verdict is on the disk rather than in the
/// handle that wrote it.
#[test]
fn a_repaired_request_is_settled_and_reattaches() -> TestResult {
    let scratch = Scratch::new("req-blocked")?;
    let host = Host::builder("acme")?
        .grants(GrantSet::empty())
        .run_store(scratch.path())?
        .repair_ledger(scratch.path())?
        .build()?;
    let analysed = Rc::new(Cell::new(0u32));
    let published = Rc::new(Cell::new(0u32));
    let work = {
        let analysed = Rc::clone(&analysed);
        let published = Rc::clone(&published);
        task("blocked-request", move |scope: Scope, value: u32| {
            let analysed = Rc::clone(&analysed);
            let published = Rc::clone(&published);
            async move {
                let analysis = remember(&scope, "analysis", || async {
                    analysed.set(analysed.get().saturating_add(1));
                    Ok::<_, FlowError>(value)
                })
                .await?;
                let publication = scope.enter("publish")?;
                publication.require(&[Cap::new(Cap::NET)])?;
                remember(&publication, "send", || async {
                    published.set(published.get().saturating_add(1));
                    Ok::<_, FlowError>(analysis)
                })
                .await
            }
        })?
    };
    let key = RequestKey::new("needs-net")?;

    let first = lgwks_bot::script::block_on(host.submit(&key, &work, 9u32))?;
    assert_eq!(
        disposition_of(&first)?,
        Disposition::Blocked,
        "a request short of authority is Blocked"
    );
    let ticket = first
        .report()
        .and_then(lgwks_bot::task::Report::repair)
        .ok_or("a blocked request on a repairable host carries a ticket")?
        .clone();
    let run = first.run_id().ok_or("the blocked request names a run")?;

    let pending = lgwks_bot::script::block_on(host.submit(&key, &work, 9u32))?;
    assert!(
        matches!(pending, Submission::InFlight(_)),
        "Blocked is not the request's verdict, so a repeat finds no recorded outcome"
    );
    assert_eq!(
        (analysed.get(), published.get()),
        (1, 0),
        "the repeat ran no body"
    );

    let repaired = lgwks_bot::script::block_on(host.repair(
        &ticket,
        &GrantSet::empty().grant(Cap::new(Cap::NET)),
        &work,
        9u32,
        1,
    ))?;
    assert_eq!(
        repaired.disposition(),
        Disposition::Succeeded,
        "the repair reaches the output: {:?}",
        repaired.error()
    );
    assert_eq!(
        (analysed.get(), published.get()),
        (1, 1),
        "the repair replayed the analysis and ran only the blocked remainder"
    );

    let settled = lgwks_bot::script::block_on(host.submit(&key, &work, 9u32))?;
    assert!(
        matches!(settled, Submission::Reattached(_)),
        "the repair recorded the request's verdict, so a repeat reattaches to it"
    );
    assert_eq!(
        settled.report().and_then(|report| report.output().copied()),
        Some(9),
        "the reattached report carries the repaired output"
    );
    assert_eq!(settled.run_id(), Some(run), "one key, one run");

    drop(host);
    let reopened = Host::builder("acme")?
        .run_store(scratch.path())?
        .repair_ledger(scratch.path())?
        .build()?;
    let durable = lgwks_bot::script::block_on(reopened.submit(&key, &work, 9u32))?;
    assert!(
        matches!(durable, Submission::Reattached(_)),
        "the verdict is read back from the file by a host that never saw the run"
    );
    assert_eq!(
        (analysed.get(), published.get()),
        (1, 1),
        "no reattach entered the body"
    );
    Ok(())
}
