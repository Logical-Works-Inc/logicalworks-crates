//! The repair-ledger rows (`ephemeral`): T13 budget charging and T24 ticket application, with the ledger readers only they use.

use super::*;
use lgwks_bot::task::{RepairError, RepairTicket};

/// The run and ticket a blocked `Report<u32>` names, read off together because
/// the repair tests below need both and a report naming one without the other
/// is a setup failure, not a row verdict.
///
/// Repair tickets need run identities the entropy source mints, so this helper
/// and its callers need `ephemeral`, like the repair tests they serve.
fn ticket_and_run(report: &Report<u32>) -> Result<(RunId, RepairTicket), Box<dyn Error>> {
    let run = report
        .run_id()
        .ok_or("a run on a host with a store must name its run id")?;
    let ticket = report
        .repair()
        .ok_or("a blocked run on a repairable host must carry a repair ticket")?
        .clone();
    Ok((run, ticket))
}

/// The four ledger counters for `run`: attempts, spend, epoch, applied tickets.
///
/// One reader rather than four inline lookups: the T13/T24 claim is that refused
/// repairs move none of them, which is a comparison of two snapshots. Gated
/// with its callers: counters live on ledger controls, which need `ephemeral`
/// run identities.
fn ledger_snapshot(host: &Host, run: RunId) -> Result<(u64, u64, u64, usize), Box<dyn Error>> {
    let control = host
        .run_ledger()
        .ok_or("a repairable host keeps a repair ledger")?
        .control(run)
        .ok_or("a blocked run has a control state")?;
    Ok((
        control.attempts(),
        control.spend(),
        control.epoch(),
        control.applied(),
    ))
}

// ── T13 ─────────────────────────────────────────────────────────────────────

/// T13, budget half: a repair charges the run's root attempts and spend and
/// never resets them, and a denied repair leaves every counter where it was.
///
/// Needs `ephemeral`: ledger controls are keyed by run identities the entropy
/// source mints. The finite-retry half stays with the proposal and sim tests
/// the map names.
#[test]
fn a_repair_charges_the_root_budget_and_never_resets_it_t13() -> TestResult {
    let scratch = Scratch::new("t13-budget")?;
    let host = Host::builder("acme")?
        .grants(GrantSet::empty())
        .run_store(scratch.path())?
        .repair_ledger(scratch.path())?
        .build()?;
    let declared = task("publish", |scope: Scope, value: u32| async move {
        let analyzed = remember(
            &scope,
            "analysis",
            || async move { Ok::<_, FlowError>(value) },
        )
        .await?;
        let publication = scope.enter("publish")?;
        publication.require(&[Cap::new(Cap::NET)])?;
        remember(&publication, "send", || async move {
            Ok::<_, FlowError>(analyzed)
        })
        .await
    })?;

    let report: Report<u32> = lgwks_bot::script::block_on(host.run(&declared, 7u32));
    assert_eq!(
        report.disposition(),
        Disposition::Blocked,
        "the publication reaches for authority the host does not grant: {:?}",
        report.error()
    );
    let (run, ticket) = ticket_and_run(&report)?;
    let (attempts_before, spend_before, _, _) = ledger_snapshot(&host, run)?;

    let grant = GrantSet::empty().grant(Cap::new(Cap::NET));
    let repaired: Report<u32> =
        lgwks_bot::script::block_on(host.repair(&ticket, &grant, &declared, 7u32, 7))?;
    assert_eq!(
        repaired.disposition(),
        Disposition::Succeeded,
        "the authorized repair settles the run: {:?}",
        repaired.error()
    );
    assert_eq!(
        repaired.output(),
        Some(&7),
        "the repaired run carries the analysis through to its output"
    );

    let (attempts_after, spend_after, epoch_after, applied_after) = ledger_snapshot(&host, run)?;
    assert!(
        attempts_after > attempts_before,
        "a repair consumes root attempts rather than resetting them: {attempts_before} before, {attempts_after} after"
    );
    assert_eq!(
        spend_after,
        spend_before.saturating_add(7),
        "a repair charges its spend onto the root budget"
    );
    assert_eq!(epoch_after, 1, "one applied repair moves the epoch once");
    assert_eq!(applied_after, 1, "one ticket is recorded applied");

    // A denied repair charges nothing, mints no epoch and leaves every counter.
    let denied =
        lgwks_bot::script::block_on(host.repair(&ticket, &GrantSet::empty(), &declared, 7u32, 1));
    assert!(
        matches!(denied, Err(RepairError::NotAuthorized { .. })),
        "a grant that does not cover the ticket is denied before any authority is applied: {denied:?}"
    );
    assert_eq!(
        ledger_snapshot(&host, run)?,
        (attempts_after, spend_after, epoch_after, applied_after),
        "a denied repair is exactly nothing: no budget, no epoch, no step"
    );
    Ok(())
}

// ── T24 ─────────────────────────────────────────────────────────────────────

/// T24: a ticket applies once; a denied repair and an over-wide grant change
/// nothing countable; the epoch moved exactly once.
///
/// Needs `ephemeral`: tickets and ledger controls are keyed by run identities
/// the entropy source mints.
///
/// The journey reaches once and sends once, with no analysis step: the arms
/// under test are the ticket's, and an analysis here would only re-prove T23's
/// replay beside them.
#[test]
fn a_ticket_applies_once_and_a_denied_or_wide_grant_changes_nothing_t24() -> TestResult {
    let scratch = Scratch::new("t24-arms")?;
    let host = Host::builder("acme")?
        .grants(GrantSet::empty())
        .run_store(scratch.path())?
        .repair_ledger(scratch.path())?
        .build()?;
    let sends = Rc::new(Cell::new(0_u64));
    let declared = {
        let sends = Rc::clone(&sends);
        task("send-once", move |scope: Scope, value: u32| {
            let sends = Rc::clone(&sends);
            async move {
                let step = scope.enter("send")?;
                step.require(&[Cap::new(Cap::NET)])?;
                sends.set(sends.get().saturating_add(1));
                Ok(value)
            }
        })?
    };
    let report: Report<u32> = lgwks_bot::script::block_on(host.run(&declared, 7u32));
    assert_eq!(
        report.disposition(),
        Disposition::Blocked,
        "the send reaches for authority the host does not grant: {:?}",
        report.error()
    );
    let (run, ticket) = ticket_and_run(&report)?;
    let exact = GrantSet::empty().grant(Cap::new(Cap::NET));

    let first: Report<u32> =
        lgwks_bot::script::block_on(host.repair(&ticket, &exact, &declared, 7u32, 1))?;
    assert_eq!(
        first.disposition(),
        Disposition::Succeeded,
        "the authorized repair settles the run: {:?}",
        first.error()
    );
    assert_eq!(sends.get(), 1, "the send ran once, after the repair");

    // The same ticket delivered twice applies once.
    let dupe = lgwks_bot::script::block_on(host.repair(&ticket, &exact, &declared, 7u32, 1));
    assert!(
        matches!(
            dupe,
            Err(RepairError::AlreadyApplied) | Err(RepairError::StaleEpoch { .. })
        ),
        "a redelivered ticket is refused: {dupe:?}"
    );
    assert_eq!(sends.get(), 1, "the second delivery repeated no effect");

    // A denied repair and an over-wide grant change nothing countable.
    let snapshot = ledger_snapshot(&host, run)?;
    let denied =
        lgwks_bot::script::block_on(host.repair(&ticket, &GrantSet::empty(), &declared, 7u32, 1));
    assert!(
        matches!(denied, Err(RepairError::NotAuthorized { .. })),
        "a grant short of the ticket is denied: {denied:?}"
    );
    let wide = exact.grant(Cap::new("custom.new.capability"));
    let over = lgwks_bot::script::block_on(host.repair(&ticket, &wide, &declared, 7u32, 1));
    assert!(
        matches!(over, Err(RepairError::OverWide { .. })),
        "a grant reaching past the ticket is refused rather than narrowed: {over:?}"
    );
    assert_eq!(
        ledger_snapshot(&host, run)?,
        snapshot,
        "refused repairs leave every counter where it was"
    );
    assert_eq!(
        snapshot.2, 1,
        "one applied repair moved the epoch exactly once"
    );
    Ok(())
}
