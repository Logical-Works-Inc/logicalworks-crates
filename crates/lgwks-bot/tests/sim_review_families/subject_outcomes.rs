//! The journey-wide properties of the subject family: permission against
//! transport outcomes, saturation, tenant isolation and commit pinning (T31,
//! T33, T34).
//!
//! Split out of `subject_arms.rs` by responsibility (the size law). The per-arm
//! properties live there; the fault space and the run live in `subject.rs`.

use lgwks_bot::domain::gh::CommitId;
use lgwks_bot::review::ReviewOutcome;

use crate::saturation::run_saturation_tier;
use crate::scripted_gh::{Scenario, ScriptedGh};
use crate::subject::{SubjectFault, over_ceiling_draw, subject_scenario};
use crate::{CAPTURE, HEAD, TestResult, gh_for, host, request, review_task, sim};

/// The same two faults reach different outcomes through the whole journey.
///
/// The classification above is the adapter's; this is the journey's. A run
/// whose read-back lost permission reports `Unverified` with the id retained,
/// while a run whose read-back failed for a transport reason reports `Unknown`.
/// Merging them would either discard applied-effect evidence or over-report a
/// blocking state for a failure nobody can attribute to permissions.
#[test]
fn permission_loss_and_transport_outcome_differ() -> TestResult {
    let band = sim::Band::new(24, 8);
    for index in band.seeds() {
        let denied = ScriptedGh::install("outcome-denied", HEAD)?;
        denied.configure(Scenario::new(HEAD).deny_reads())?;
        let host = host()?;
        let job = review_task()?;
        let denied_report = host.block_on(&job, (gh_for(&denied, CAPTURE)?, request(7)?))?;

        let outage = ScriptedGh::install("outcome-transported", HEAD)?;
        outage.configure(Scenario::new(HEAD).fail_reads(4))?;
        let outage_report = host.block_on(&job, (gh_for(&outage, CAPTURE)?, request(7)?))?;

        assert!(
            denied_report
                .output()
                .is_some_and(ReviewOutcome::is_unverified),
            "seed {index}: a lost permission after a create is Unverified: {:?}",
            denied_report.output()
        );
        assert!(
            outage_report
                .output()
                .is_some_and(ReviewOutcome::is_unknown),
            "seed {index}: a transport failure leaves the effect unobserved, so it \
             is Unknown — not Unverified, which would claim a permission loss \
             nobody observed: {:?}",
            outage_report.output()
        );
        assert_eq!(
            denied.creates()?,
            1,
            "seed {index}: the denied run applied exactly one create"
        );
    }
    Ok(())
}

/// Saturation over the subject path: many concurrent runs on one pull request,
/// each receiver's own record asserted.
///
/// The concurrency counterpart to the arm families above: a per-seed arm is one
/// world at a time, and a bound that holds for one run says nothing about a host
/// running many. The runs go through [`run_saturation_tier`], so the receiver is
/// sharded at [`RUNS_PER_RECEIVER`] and the equalities are the strict ones — with
/// no fault configured, creates *equal* runs and every run verifies — rather
/// than the weak `creates <= runs` a degenerate world satisfies just as readily.
#[test]
fn subject_saturation_conserves_creates() -> TestResult {
    for _ in 0..16u64 {
        run_saturation_tier(64, 16, "subject-saturation")?;
    }
    Ok(())
}

/// Two tenants' coverage decisions on one pull request stay apart.
///
/// A host that leaked a diff verdict between runs would tell one tenant its
/// coverage was complete when the other tenant's oversized diff was the reason
/// it was refused. The two tenants' inventories differ in size — one clean, one
/// past the file ceiling — so a crossed verdict would be visible.
#[test]
fn two_tenants_coverage_stays_isolated() -> TestResult {
    let band = sim::Band::new(25, 16);
    for index in band.seeds() {
        let clean = ScriptedGh::install("tenant-clean", HEAD)?;
        clean.configure(Scenario::new(HEAD))?;
        let refused = ScriptedGh::install("tenant-refused", HEAD)?;
        refused.configure(subject_scenario(
            SubjectFault::DiffOverFiles,
            HEAD,
            over_ceiling_draw(index),
        )?)?;

        let host = host()?;
        let job = review_task()?;
        let clean_report = host.block_on(&job, (gh_for(&clean, CAPTURE)?, request(7)?))?;
        let refused_report = host.block_on(
            &job,
            (
                gh_for(&refused, fault_capture(SubjectFault::DiffOverFiles))?,
                request(7)?,
            ),
        )?;

        assert!(
            clean_report
                .output()
                .is_some_and(ReviewOutcome::is_published),
            "seed {index}: the clean tenant's coverage is complete and it publishes: {:?}",
            clean_report.output()
        );
        assert!(
            refused_report
                .output()
                .is_some_and(ReviewOutcome::is_incomplete),
            "seed {index}: the over-ceiling tenant's coverage is incomplete and it \
             publishes nothing — the two verdicts must not cross: {:?}",
            refused_report.output()
        );
        assert_eq!(
            refused.creates()?,
            0,
            "seed {index}: the refused tenant's receiver saw no create"
        );
        assert_eq!(
            clean.creates()?,
            1,
            "seed {index}: the clean tenant's receiver saw exactly one create"
        );
    }
    Ok(())
}

/// The capture ceiling the file-ceiling arm runs with.
///
/// One helper because the byte arm shares it and a per-family literal is a
/// place for the two to drift: a family running the over-ceiling inventory
/// under a capture ceiling that refused it for the *wrong* reason would report
/// an incomplete coverage for a capture truncation and prove nothing about the
/// file bound.
fn fault_capture(fault: SubjectFault) -> usize {
    fault.capture()
}

/// The review is pinned to the commit that was read, across every arm.
///
/// Every arm above checks a *refusal* arm. This one checks the success arm and
/// the pinning that T31/T33/T34 all rest on: a publication is reported at the
/// commit the run read and nowhere else. It runs through the whole path, so a
/// run that somehow re-pointed the subject at the canonical name after a rename
/// would publish at a commit it never read.
#[test]
fn a_publication_is_pinned_to_the_read_commit() -> TestResult {
    let band = sim::Band::new(26, 16);
    for index in band.seeds() {
        let scripted = ScriptedGh::install("pinned", HEAD)?;
        scripted.configure(Scenario::new(HEAD))?;
        let host = host()?;
        let job = review_task()?;
        let report = host.block_on(&job, (gh_for(&scripted, CAPTURE)?, request(7)?))?;

        let outcome = report
            .output()
            .ok_or_else(|| format!("seed {index}: a clean run reports an outcome"))?;
        assert!(
            outcome.is_published(),
            "seed {index}: a clean run publishes: {outcome:?}"
        );
        assert_eq!(
            outcome.commit_id().map(CommitId::as_str),
            Some(HEAD),
            "seed {index}: the publication is pinned to the commit that was read, \
             never re-pointed: {outcome:?}"
        );
        assert_eq!(
            scripted.creates()?,
            1,
            "seed {index}: a clean run creates exactly once"
        );
    }
    Ok(())
}
