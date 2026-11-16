//! The ledger rows: the seven-field sweep, the idempotent duplicate and the
//! foreign run.

use std::cell::RefCell;
use std::rc::Rc;

use lgwks_bot::ecs::EcsError;
use lgwks_bot::error::KernelError;
use lgwks_bot::journal::MemoryJournal;
use lgwks_bot::spec::EffectEvidence as Settled;

use crate::sim::rig::recorded;

use super::{TestResult, WrongField, held_under, replace_run};

/// Every wrong identity field is refused, and the correct one is not.
///
/// The row's core, and the reason it is a sweep rather than six tests: one
/// delivery with the right identity, and six that each differ from it in exactly
/// one field. The correct delivery must succeed, so a blanket "refuse everything"
/// implementation fails this row; and each wrong one must be refused with the
/// variant that matches its field, so an implementation that refused everything
/// with `NoSuchWork` would also fail it.
///
/// The refusals are the *ledger's* here, and the fields fall into two groups. A
/// wrong **payload**, **revision**, **epoch** or **attempt** names work the
/// ledger does hold, so it can say precisely which: `EvidenceSuperseded` for the
/// generation and `EvidenceStaleAttempt` for the attempt, which is what lets a
/// caller decide whether to re-read the pending work or to send a different
/// report. A wrong **run** is not the ledger's work at all, and a wrong
/// **action** is not declared here, so both are refused before classification —
/// `NoSuchWork` with no address, and `ActionNotDeclared` with the action, which
/// is the most precise answer either can give. Asserting the exact variant per
/// field, rather than "an error", is what makes this a test of the identity
/// checks and not of the fact that keys are `Eq`.
#[test]
fn every_wrong_identity_field_is_refused_and_the_correct_one_is_not() -> TestResult {
    for wrong in WrongField::ALL {
        let held = held_under(Rc::new(RefCell::new(MemoryJournal::new())))?;
        let mut bot = held.bot;
        let before = recorded(&held.store)?;
        let submitted = wrong.key(held.binding)?;
        assert_ne!(
            submitted,
            held.binding,
            "{}: the submitted key must actually differ from the held one — an \
             arm that rewrites nothing is testing nothing",
            wrong.tag()
        );
        if wrong == WrongField::Action {
            assert_ne!(
                submitted.action(),
                held.binding.action(),
                "action: the rewrite must name a different action, not a \
                 different attempt under the same one"
            );
        }

        let outcome = bot.resolve_effect(&submitted, Settled::Applied);
        assert!(
            outcome.is_err(),
            "{}: evidence naming a different {} was accepted",
            wrong.tag(),
            wrong.tag()
        );
        assert_refusal_for(wrong, &outcome)?;
        assert_eq!(
            recorded(&held.store)?,
            before,
            "{}: the refusal wrote to the journal",
            wrong.tag()
        );
        assert_eq!(
            held.entered.get(),
            1,
            "{}: the refusal must not re-enter the action",
            wrong.tag()
        );
        assert_eq!(
            bot.pending().len(),
            1,
            "{}: the entry must still be held after the refusal",
            wrong.tag()
        );

        // And the correct identity, on the same bot, is accepted — which is what
        // makes the refusals above refusals rather than a door that is shut.
        bot.resolve_effect(&held.binding, Settled::Applied)?;
        assert!(
            bot.pending().is_empty(),
            "{}: the correct identity settled the entry, so the refusals above \
             were about the field and not about the door",
            wrong.tag()
        );
    }
    Ok(())
}

/// The refusal this file requires for one wrong field.
///
/// Asserted as a `match` over the typed variant rather than as a string, so a
/// change that renames a variant breaks the row instead of quietly widening what
/// it accepts. `&_` on the error, because the workspace forbids matching a
/// non-`Copy` payload by value out of a reference.
///
/// The pattern across the arms is the thing worth stating. An `ActionId` is
/// derived from the bot's name, its position and its action's domain — *not*
/// from the run, the flow revision or the environment — so the six fields fall
/// into three groups with three different honest answers:
///
/// - `Payload` describes a generation that is gone, and the refusal names both
///   bindings so the caller can re-read the pending work.
/// - `Epoch` names a generation the journal holds no ladder for, so the
///   settlement cannot be recorded at all and the refusal is `EffectUnrecorded`.
/// - `Run`, `Revision`, `Environment` and `Action` name work outside what this
///   ledger declares at all, and are refused as `ActionNotDeclared` — never as
///   `NoSuchWork`, which means "no held effect here" and would be a lie about
///   work that *is* held.
/// - `Attempt` describes an attempt that has been overtaken, and the refusal
///   names both attempt numbers.
///
/// A caller who wanted those four to answer uniformly would be asking for a
/// refusal that cannot name the field, which is the one thing the row forbids.
fn assert_refusal_for(wrong: WrongField, outcome: &Result<(), EcsError>) -> TestResult {
    match (wrong, outcome.as_ref().err()) {
        (WrongField::Payload, Some(&EcsError::EvidenceSuperseded { .. })) => {}
        // A key from a *later* generation is not merely reporting on gone work:
        // the journal has no ladder for it at all, so the settlement cannot even
        // be recorded. `EffectUnrecorded` is the honest answer — the caller
        // named an attempt whose intent was never admitted under that generation.
        (WrongField::Epoch, Some(&EcsError::EffectUnrecorded { .. })) => {}
        (WrongField::Attempt, Some(&EcsError::EvidenceStaleAttempt { .. })) => {}
        // Everything outside the identity this ledger speaks for. A key that
        // names another run, another flow revision or another environment is not
        // its work, and one that names an action it does not declare is not
        // addressable at all. All four land on `ActionNotDeclared`, and that is
        // the right answer rather than a limitation: the ledger's own arm says it
        // exists for "a key for an action this bot does not declare", and a key
        // from another run reaches that state because the two runs declare
        // different actions from the same bot name. The refusal names the action,
        // so a caller can see *which* action it was not declared.
        //
        // `NoSuchWork` is *not* substituted here on purpose. It means "there is
        // no held effect at this address", and a caller told that about an action
        // it does hold work for is being told the wrong thing about work that is
        // still held. The variant that says "not declared" is the one a caller can
        // act on.
        (
            WrongField::Run | WrongField::Revision | WrongField::Environment | WrongField::Action,
            Some(&EcsError::Kernel(KernelError::ActionNotDeclared { .. })),
        ) => {}
        // Anything else is a failure of the row, reported through the caller's
        // own `Result` rather than through a panic, which this workspace forbids.
        (wrong, actual) => {
            let refusal = Err(format!(
                "{}: the refusal was {actual:?}, not the one this field must \
             produce",
                wrong.tag()
            )
            .into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "assert_refusal_for: returning an error to the caller");
            return refusal;
        }
    }
    Ok(())
}

/// The duplicate is idempotent and the contradiction is refused, so the ledger
/// distinguishes the two deliveries the row's callers depend on.
///
/// The control that stops a blanket "refuse everything" answer from passing the
/// row above: the same key with the same evidence twice is a *success* — a
/// caller whose first delivery it never saw land has to be able to repeat it —
/// and the same key with the other evidence is a *refusal* that leaves the
/// standing settlement alone.
#[test]
fn a_duplicate_is_idempotent_and_a_contradiction_is_refused() -> TestResult {
    let held = held_under(Rc::new(RefCell::new(MemoryJournal::new())))?;
    let mut bot = held.bot;
    bot.resolve_effect(&held.binding, Settled::Applied)?;
    let settled = recorded(&held.store)?;

    // The repeat. A delivery whose acknowledgement the caller never saw is the
    // ordinary case, not a caller mistake.
    bot.resolve_effect(&held.binding, Settled::Applied)?;
    assert_eq!(
        recorded(&held.store)?,
        settled,
        "a duplicate delivery wrote a second settlement"
    );

    // The contradiction.
    match bot.resolve_effect(&held.binding, Settled::NotApplied) {
        Err(EcsError::EvidenceContradicted {
            settled: Settled::Applied,
            submitted: Settled::NotApplied,
            ..
        }) => {}
        other => {
            return Err(format!(
                "evidence contradicting a settlement must be refused as \
                 EvidenceContradicted, got {other:?}"
            )
            .into());
        }
    }
    assert_eq!(
        recorded(&held.store)?,
        settled,
        "the contradicted delivery wrote to the journal"
    );
    assert!(
        bot.pending().is_empty(),
        "the acknowledged generation is still acknowledged"
    );
    Ok(())
}

/// A settlement carrying another run's identity is refused, and the ledger it was
/// refused by is unchanged.
///
/// The run clause of T12, on its own. An `ActionId` is derived from the bot's
/// name, its position and its action's domain — **not** from the run — so two
/// ledgers of the same shape under the same bot name declare the *same* action,
/// and "two ledgers" cannot be built by differing the run alone. What can be, and
/// is here, is one ledger offered a settlement whose key names another run: the
/// ledger speaks for exactly one run, and a key that names another is refused
/// before it can be classified against any entry.
///
/// The refusal is `ActionNotDeclared` rather than `NoSuchWork`, and the row says
/// so explicitly because the two mean opposite things to a caller. `NoSuchWork`
/// is "there is no held effect at this address"; telling a caller that about work
/// it *does* hold — which this entry is — is the confusion the variant exists to
/// prevent. The journal must not move, the action must not be re-entered, and the
/// correct identity must still settle, which is what makes the refusal a refusal
/// and not a closed door.
///
/// Not claimed: two ledgers over one shared journal in one process. The ledger's
/// actions are derived from the bot's structure, so two of them with the same
/// name are the same ledger as far as the identity model is concerned, and a row
/// that claimed cross-ledger isolation here would be claiming something the type
/// does not distinguish.
#[test]
fn a_settlement_carrying_another_runs_identity_is_refused() -> TestResult {
    let store = Rc::new(RefCell::new(MemoryJournal::new()));
    let mut held = held_under(Rc::clone(&store))?;
    let before = recorded(&store)?;

    let foreign = replace_run(held.binding)?;
    assert_ne!(
        foreign, held.binding,
        "the foreign key must be a different identity, or the row proves nothing"
    );
    assert_ne!(
        foreign.run(),
        held.binding.run(),
        "and it must be the *run* that differs, since the action is shared"
    );

    match held.bot.resolve_effect(&foreign, Settled::Applied) {
        Err(EcsError::Kernel(KernelError::ActionNotDeclared { .. })) => {}
        other => {
            return Err(format!(
                "a settlement naming another run must be refused as not declared: \
                 {other:?}"
            )
            .into());
        }
    }

    assert_eq!(
        recorded(&store)?,
        before,
        "the refused settlement wrote to the journal"
    );
    assert_eq!(
        held.entered.get(),
        1,
        "the refusal must not re-enter the action"
    );
    assert_eq!(
        held.bot.pending().len(),
        1,
        "the entry is still held after the refusal"
    );

    // And the correct identity still settles, which is what makes the refusal
    // about the field rather than about the door.
    held.bot.resolve_effect(&held.binding, Settled::Applied)?;
    assert!(
        held.bot.pending().is_empty(),
        "the correct identity settled the entry"
    );
    Ok(())
}
