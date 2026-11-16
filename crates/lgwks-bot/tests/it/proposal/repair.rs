//! The T29 family: repeated unchanged failure reaches a finite typed intervention, on the ledger directly and across real runs.

use super::*;

// ── T29: finite typed intervention, bounded repair ───────────────────────────

/// Repeated unchanged failure reaches a finite typed intervention, and never an
/// unbounded stream of repairs.
#[test]
fn repeated_unchanged_failure_reaches_a_finite_intervention_t29() -> TestResult {
    let limits = LedgerLimits::new(3, 8);
    let mut ledger = RepairLedger::new(limits, provenance(b"op=read-report"));

    for attempt in 1..=limits.repeat {
        assert!(
            ledger.record_failure("boom").is_admitted(),
            "attempt {attempt} of {} may still be repaired",
            limits.repeat
        );
    }
    let outcome = ledger.record_failure("boom");
    assert_eq!(
        outcome.intervention(),
        Some(&Intervention::NoProgress {
            fingerprint: String::from("boom"),
            repetitions: limits.repeat.saturating_add(1),
        }),
        "the repetition past the ceiling is a typed intervention, not another repair: {outcome}"
    );

    // And it stays typed: every later attempt reports the same intervention
    // rather than reverting to a repair.
    for _ in 0..8 {
        assert!(
            ledger.record_failure("boom").intervention().is_some(),
            "the ledger never returns to repairing an unchanged failure"
        );
    }
    assert_eq!(
        ledger.spent(),
        u64::from(limits.repeat).saturating_add(9),
        "and every attempt is charged to the root spend"
    );
    Ok(())
}

/// A ledger full of *different* failures is its own intervention, so a run
/// making progress through new failures still reaches a person.
#[test]
fn a_ledger_of_distinct_failures_reaches_its_own_intervention() -> TestResult {
    let limits = LedgerLimits::new(3, 2);
    let mut ledger = RepairLedger::new(limits, provenance(b"op=read-report"));

    assert!(
        ledger.record_failure("a").is_admitted(),
        "the first is recorded"
    );
    assert!(ledger.record_failure("b").is_admitted(), "so is the second");
    assert_eq!(
        ledger.record_failure("c").intervention(),
        Some(&Intervention::LedgerFull { held: 2 }),
        "the third distinct fingerprint is refused with the ceiling named"
    );
    Ok(())
}

/// New evidence is recorded and does not erase root spend: the repetition count
/// the ceiling is measured against never moves.
#[test]
fn new_evidence_does_not_erase_root_spend() -> TestResult {
    let limits = LedgerLimits::new(2, 8);
    let mut ledger = RepairLedger::new(limits, provenance(b"op=read-report"));

    assert!(
        ledger.record_failure("boom").is_admitted(),
        "the first failure is recorded"
    );
    let second = ledger.record_failure("boom");
    assert!(
        second.is_admitted() && ledger.may_repair("boom"),
        "two of two failures may be repaired: {second}"
    );

    assert!(
        ledger.record_evidence("boom"),
        "new evidence is recorded against the fingerprint"
    );
    assert!(ledger.has_progress("boom"), "and is visible as progress");
    assert_eq!(
        ledger.repetitions("boom"),
        2,
        "the repetition count the ceiling is measured against has not moved"
    );
    assert_eq!(ledger.evidence(), 1, "the evidence is counted");

    let outcome = ledger.record_failure("boom");
    assert!(
        matches!(
            outcome.intervention(),
            Some(Intervention::NoProgress { repetitions: 3, .. })
        ),
        "and the next identical failure is still the intervention: evidence was recorded \
         beside the spend, not instead of it"
    );
    assert_eq!(
        ledger.spent(),
        3,
        "root spend counts every failure, evidence or not"
    );
    assert_eq!(
        ledger.evidence(),
        1,
        "and the evidence recorded is still recorded"
    );
    assert!(
        !ledger.record_evidence("never-failed"),
        "evidence for a fingerprint nobody failed under is refused: the run has not got there"
    );
    Ok(())
}

/// Bounded repair is a declared ceiling, not a property of the loop.
#[test]
fn a_plan_budget_bounds_repair() -> TestResult {
    let surface = surface()?;
    let decoder = decoder();
    let mut budget = PlanBudget::new(2);

    for attempt in 1..=2 {
        let outcome = budget.admit_once(&decoder, &surface, &well_formed(), Source::Model)?;
        assert!(
            outcome.is_admitted(),
            "admission {attempt} of 2 is within the repair budget: {outcome}"
        );
    }
    assert_eq!(budget.remaining(), 0, "both admissions were charged");

    let refusal = budget
        .admit_once(&decoder, &surface, &well_formed(), Source::Model)
        .err()
        .ok_or("the third admission is refused rather than admitted")?;
    assert!(
        matches!(refusal, Refusal::Limit { .. }),
        "a spent budget refuses by name: {refusal}"
    );
    assert_eq!(
        budget.remaining(),
        0,
        "and a refused charge does not underflow the budget"
    );
    Ok(())
}

// ── T29: one run reaches a finite typed intervention, not four refusals ───────

/// Four refusals of the *same* reason inside one run reach a finite typed
/// intervention on the fourth, and the run's report carries that intervention
/// rather than a fourth refusal.
///
/// The point is "across calls in one run": a per-call ledger would refuse every
/// time and never intervene, so the falsifier has to drive four *separate*
/// `Host::run`s sharing one gate rather than four admissions in one body.
#[test]
fn repeated_unchanged_failure_reaches_a_finite_intervention_across_runs() -> TestResult {
    let host = Host::builder(TENANT)?.build()?;
    let work = support::admitting_task()?;
    let gate = support::gate(TENANT)?;
    let payload = installs();
    let arm = Refusal::install_tool("ripgrep").label();

    // Three refusals are tolerated: the run may still repair. The fourth is the
    // intervention, because `REPEAT` is three.
    for attempt in 1..=support::REPEAT {
        let report = drive(host.run(&work, (gate.clone(), payload.clone())));
        assert_eq!(
            report.disposition(),
            lgwks_bot::task::Disposition::Failed,
            "attempt {attempt} of {} still fails its run",
            support::REPEAT
        );
        let error = report.error().ok_or("each failed run carries its error")?;
        assert_eq!(
            lgwks_bot::script::refusal_of(error).map(Refusal::label),
            Some(arm),
            "attempt {attempt} is a refusal, not yet an intervention"
        );
        assert_eq!(
            gate.repetitions(arm),
            attempt,
            "the ledger counted the refusal under its arm"
        );
    }

    // The fourth identical refusal is the intervention.
    let report = drive(host.run(&work, (gate.clone(), payload)));
    let error = report
        .error()
        .ok_or("the fourth refused run carries its error")?;
    assert!(
        lgwks_bot::script::refusal_of(error).is_none(),
        "the fourth identical failure is no longer a refusal"
    );
    let intervention =
        lgwks_bot::script::intervention_of(error).ok_or("it is the intervention arm")?;
    assert!(
        matches!(intervention, Intervention::NoProgress { .. }),
        "a run repeating one unchanged failure reaches NoProgress, not an infinite repair: \
         {intervention}"
    );
    assert_eq!(
        gate.spent(),
        u64::from(support::REPEAT).saturating_add(1),
        "the run's root spend is the four failures it actually recorded, counted once"
    );

    // And the ledger never returns to repairing that fingerprint: the run gathers
    // new evidence, which is recorded, and the *next* identical failure is still
    // past the ceiling rather than another repair.
    assert!(
        gate.record_evidence(arm),
        "evidence is recorded beside the root spend for a fingerprint the run has failed under"
    );
    assert!(
        gate.has_progress(arm),
        "and the ledger reports the progress it recorded"
    );
    let after = drive(host.run(&work, (gate.clone(), installs())));
    let error = after.error().ok_or("the fifth run carries its error")?;
    assert!(
        matches!(
            lgwks_bot::script::intervention_of(error),
            Some(Intervention::NoProgress { .. })
        ),
        "new evidence did not buy another pass at a failure already past its ceiling: {error}"
    );
    assert_eq!(
        gate.repetitions(arm),
        support::REPEAT.saturating_add(2),
        "and the count the ceiling is measured against has moved only by the failures that \
         actually arrived, not by the evidence recorded between them"
    );
    Ok(())
}

/// The admission budget bounds repair on its own axis: once spent, the next
/// admission is refused with its ceiling named, and a refusal of the budget is not
/// counted as a payload failure.
///
/// Distinct from the ledger above: the ledger counts *unchanged* failures and the
/// budget counts *admissions*, so a run feeding the ledger distinct failures still
/// runs out of admissions, and a run spending its budget on refusals must not
/// reach the ledger ceiling instead.
#[test]
fn a_plan_budget_bounds_repair_across_runs() -> TestResult {
    let host = Host::builder(TENANT)?.build()?;
    let work = support::admitting_task()?;
    let gate = support::gate(TENANT)?;

    assert_eq!(
        gate.remaining(),
        support::ADMISSIONS,
        "the gate opens with its whole admission ceiling"
    );
    for attempt in 1..=support::ADMISSIONS {
        // A *different* refusal arm each time, so the ledger stays under its own
        // ceiling and the budget is the only thing that can bind.
        let mut payload = b"op=read-report\ninstall=tool-".to_vec();
        payload.extend_from_slice(attempt.to_string().as_bytes());
        payload.push(b'\n');
        let report = drive(host.run(&work, (gate.clone(), payload)));
        assert_eq!(
            report.disposition(),
            lgwks_bot::task::Disposition::Failed,
            "admission {attempt} of {} fails its run",
            support::ADMISSIONS
        );
    }

    // The budget is spent. The next admission is refused with its ceiling named,
    // and this refusal is *not* a payload failure — it did not try one.
    let report = drive(host.run(&work, (gate.clone(), well_formed())));
    let error = report
        .error()
        .ok_or("a run past its admission budget carries its error")?;
    assert!(
        matches!(
            lgwks_bot::script::refusal_of(error),
            Some(Refusal::Limit { .. })
        ),
        "a spent budget is refused with its ceiling named, not admitted: {error}"
    );
    assert_eq!(
        gate.spent(),
        u64::from(support::ADMISSIONS),
        "a budget refusal is not counted against the ledger: only the {} payloads that \
         were tried were recorded",
        support::ADMISSIONS
    );
    Ok(())
}
