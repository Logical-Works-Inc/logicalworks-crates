//! The property every family of the real review path asserts, in one place.

use lgwks_bot::review::ReviewOutcome;

use super::{Fault, Run};

/// The property every family asserts, in one place.
///
/// It is here rather than written out four times because four copies of a safety
/// argument is four chances for one of them to be the one that drifted, and a
/// copy that drifted would be a test that passes for the wrong reason.
pub(super) fn assert_the_invariant(
    fault: Fault,
    run: &Run,
) -> Result<(), Box<dyn std::error::Error>> {
    let trace = run.trace();

    // One. A publication is reported only when an independent read observed it.
    // Checked from both directions: the positive, and the negative — a fault
    // that cannot produce an observation must never report one.
    if let Some(outcome) = run.report.output() {
        match *outcome {
            ReviewOutcome::Published { verified, .. } => {
                if !fault.can_be_verified() {
                    let refusal = Err(format!(
                        "{} reported a publication it could not have observed: {outcome:?}\n{}",
                        fault.label(),
                        trace.argv
                    )
                    .into());
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "assert_the_invariant: returning an error to the caller");
                    return refusal;
                }
                if !verified {
                    let refusal = Err("a Published outcome must carry verified = true".into());
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "assert_the_invariant: returning an error to the caller");
                    return refusal;
                }
            }
            ReviewOutcome::Unknown { ref commit_id, .. } => {
                if fault.can_be_verified() {
                    let refusal = Err(format!(
                        "{} could have been observed but was reported unknown at {commit_id}: {outcome:?}",
                        fault.label()
                    )
                    .into());
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "assert_the_invariant: returning an error to the caller");
                    return refusal;
                }
            }
            ReviewOutcome::TargetMoved { .. } => {
                // A refusal, not a publication. It must name the commit that
                // was read, so a moved head cannot be reported against a
                // commit nobody read, and it must never appear for a fault
                // whose head never moved.
                if fault != Fault::HeadMoved {
                    let refusal = Err(format!(
                        "{} reported a moved head, but only a head that changed \
                         can produce one: {outcome:?}",
                        fault.label()
                    )
                    .into());
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "assert_the_invariant: returning an error to the caller");
                    return refusal;
                }
            }
            ReviewOutcome::Refused { .. } => {
                // Draft-only and similar profiles. No run in this family holds
                // one, so seeing it means a state machine reached a branch the
                // journey does not name.
                let refusal = Err(format!(
                    "{} reported a draft-only refusal, which no fault here produces",
                    fault.label()
                )
                .into());
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "assert_the_invariant: returning an error to the caller");
                return refusal;
            }
            ref other => {
                let refusal = Err(format!(
                    "{} reported {other:?}, which no run in this family produces",
                    fault.label()
                )
                .into());
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "assert_the_invariant: returning an error to the caller");
                return refusal;
            }
        }
    } else if run.report.disposition().label() == "Succeeded" {
        let refusal = Err(format!(
            "{} reported a successful run with no outcome at all",
            fault.label()
        )
        .into());
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "assert_the_invariant: returning an error to the caller");
        return refusal;
    }

    // Two. At most one create per run, whatever the fault. This is the number a
    // duplicate-post defect doubles, and it is measured at the receiver rather
    // than inferred from the outcome.
    if run.creates > 1 {
        let refusal = Err(format!(
            "{} issued {} creates: a run publishes at most once\n{}",
            fault.label(),
            run.creates,
            trace.argv
        )
        .into());
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "assert_the_invariant: returning an error to the caller");
        return refusal;
    }

    // Three. A fault that never reaches the create lands nothing, so the
    // receiver's record must agree with the fault's applicability.
    if !fault.applies() && run.creates != 0 {
        let refusal = Err(format!(
            "{} must publish nothing, but {} creates were issued\n{}",
            fault.label(),
            run.creates,
            trace.argv
        )
        .into());
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "assert_the_invariant: returning an error to the caller");
        return refusal;
    }

    Ok(())
}
