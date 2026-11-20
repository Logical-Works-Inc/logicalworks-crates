//! Reducer sims for the canvas scene: wrong-order calls, double clicks,
//! stale and foreign deltas, queue limits, revocation, and a reference model
//! run beside the real scene on one seed.
//!
//! The oracle is the kit's reference reducer and invariants (a refusal
//! consumes nothing; an accepted base revision is never reused), never the
//! scene's own output.

use drishti::canvas_protocol::SessionInvariant;
use drishti::canvas_scene::{Action, Canvas, CommitStatus, Delta, Error, View};

use crate::canvas_kit::{Model, agrees, delta, draw_action, limits, present, scene, wild_i32};
use crate::motion_kit::{describe, ensure, pick};
use crate::rng::{SimRng, TraceHash};

/// Everything a refusal must leave untouched.
fn snapshot(scene: &Canvas) -> Result<(u64, u64, u64, u64, u64, u64), String> {
    let stats = scene.stats();
    Ok((
        scene.revision(),
        scene.next_base_revision().map_err(describe)?,
        stats.accepted(),
        stats.drained(),
        stats.prepared(),
        stats.presented(),
    ))
}

/// Bug caught: a revoked scene that still accepts input, owes frames or
/// advances counters, i.e. use after release.
#[test]
fn sim_canvas_wrong_order_every_call_after_revoke_is_refused_and_inert() -> Result<(), String> {
    for seed in 5_200..5_260_u64 {
        let mut random = SimRng::seed(seed);
        let mut scene = scene("tenant-revoke", limits(8)?)?;
        let token = present(&mut scene, 0.01)?.token();
        for _ in 0..random.below(5) {
            scene
                .enqueue(delta(&scene, draw_action(&mut random))?)
                .map_err(describe)?;
        }
        scene.revoke();
        let before = snapshot(&scene)?;
        ensure(
            scene.enqueue(delta(&scene, Action::ToggleReducedMotion)?) == Err(Error::Revoked)
                && scene.input(token.clone(), Action::Navigate(View::Evidence))
                    == Err(Error::Revoked)
                && matches!(scene.prepare(0.01, 900, 600, true), Err(Error::Revoked))
                && scene.acknowledge_present(token) == Err(Error::Revoked)
                && scene.deny_optimistic() == Err(Error::Revoked),
            &format!("seed {seed}: a call after revoke was not refused with Revoked"),
        )?;
        scene.invalidate_surface();
        scene.resume_without_replay();
        ensure(
            snapshot(&scene)? == before
                && !scene.needs_frame()
                && scene.presented_token().is_none(),
            &format!("seed {seed}: a revoked scene changed or owed a frame"),
        )?;
    }
    Ok(())
}

/// Bug caught: the same delta applied more than once when the user (or a
/// retry) sends it 2, 100 or 1,000 times; exactly one must land.
#[test]
fn sim_canvas_double_click_same_delta_lands_exactly_once() -> Result<(), String> {
    for copies in [2_usize, 100, 1_000] {
        let mut scene = scene("tenant-clicker", limits(256)?)?;
        let click = delta(&scene, Action::Step(1))?;
        let mut landed = 0_usize;
        for _ in 0..copies {
            match scene.enqueue(click.clone()) {
                Ok(()) => landed = landed.saturating_add(1),
                Err(error) => ensure(
                    error == Error::Stale,
                    &format!("{copies} copies: a duplicate was refused with {error}, not Stale"),
                )?,
            }
        }
        ensure(
            landed == 1 && scene.stats().accepted() == 1 && scene.next_base_revision() == Ok(1),
            &format!("{copies} copies: {landed} landed"),
        )?;
        present(&mut scene, 0.01)?;
        ensure(
            scene.revision() == 1 && scene.target() == View::Proposal,
            &format!(
                "{copies} copies: the step applied {} times",
                scene.revision()
            ),
        )?;
        ensure(
            scene.enqueue(click) == Err(Error::Stale),
            "a drained delta replayed later must still be Stale",
        )?;
    }
    Ok(())
}

/// Bug caught: a stale generation or base revision consuming a slot or a
/// revision, or being judged after queue capacity so the wrong error shows.
#[test]
fn sim_canvas_stale_deltas_are_refused_before_capacity_and_consume_nothing() -> Result<(), String> {
    for seed in 5_300..5_360_u64 {
        let mut random = SimRng::seed(seed);
        let capacity = random.below_len(6).saturating_add(1);
        let mut scene = scene("tenant-stale", limits(capacity)?)?;
        for _ in 0..capacity {
            scene
                .enqueue(delta(&scene, draw_action(&mut random))?)
                .map_err(describe)?;
        }
        let before = snapshot(&scene)?;
        let base = scene.next_base_revision().map_err(describe)?;
        let wrong_base = match random.below(4) {
            0 => base.saturating_add(1),
            1 => base.saturating_sub(1),
            2 => 0,
            _ => u64::MAX,
        };
        let wrong_generation = scene.generation().wrapping_add(1).max(2);
        let session = scene.session().clone();
        let bad_base = Delta::new(
            session.clone(),
            scene.generation(),
            wrong_base,
            Action::ToggleReducedMotion,
        );
        let bad_generation =
            Delta::new(session, wrong_generation, base, Action::ToggleReducedMotion);
        let full = Delta::new(
            scene.session().clone(),
            scene.generation(),
            base,
            Action::ToggleReducedMotion,
        );
        ensure(
            (wrong_base == base || scene.enqueue(bad_base) == Err(Error::Stale))
                && scene.enqueue(bad_generation) == Err(Error::Stale)
                && scene.enqueue(full) == Err(Error::QueueFull),
            &format!(
                "seed {seed}: stale deltas must say Stale while a correct one on a full queue says QueueFull"
            ),
        )?;
        ensure(
            snapshot(&scene)? == before,
            &format!("seed {seed}: a refused delta consumed a revision or slot"),
        )?;
    }
    Ok(())
}

/// A session that differs from `base` in exactly one field.
fn session_differing_in(field: u64, base: &SessionInvariant) -> Result<SessionInvariant, String> {
    let other_uuid = "99999999-9999-9999-9999-999999999999";
    let choose = |slot: u64, mine: &str, theirs: &str| -> String {
        if slot == field {
            String::from(theirs)
        } else {
            String::from(mine)
        }
    };
    SessionInvariant::new(
        choose(0, base.work_object_id(), other_uuid),
        choose(1, base.session_id(), other_uuid),
        choose(2, base.policy_bundle_id(), other_uuid),
        choose(3, base.tenant_id(), "tenant-impostor"),
        choose(4, base.user_sub(), other_uuid),
    )
    .map_err(describe)
}

/// Bug caught: tenant B's delta landing on tenant A's scene when B reuses
/// A's work object and session id, or when only one identity field differs.
#[test]
fn sim_canvas_neighbour_delta_with_a_foreign_session_is_wrong_scope() -> Result<(), String> {
    for seed in 5_400..5_440_u64 {
        let mut random = SimRng::seed(seed);
        let mut mine = scene("tenant-a", limits(32)?)?;
        let mut theirs = scene("tenant-b", limits(32)?)?;
        let mut own_accepted = 0_u64;
        for _ in 0..200 {
            let action = draw_action(&mut random);
            if random.below(3) == 0 {
                let forged = session_differing_in(random.below(5), mine.session())?;
                let forged_delta = Delta::new(
                    forged,
                    mine.generation(),
                    mine.next_base_revision().map_err(describe)?,
                    action,
                );
                let before = snapshot(&mine)?;
                ensure(
                    mine.enqueue(forged_delta) == Err(Error::WrongScope)
                        && snapshot(&mine)? == before,
                    &format!(
                        "seed {seed}: a delta with a foreign identity field was not WrongScope"
                    ),
                )?;
                let from_b = Delta::new(
                    theirs.session().clone(),
                    mine.generation(),
                    mine.next_base_revision().map_err(describe)?,
                    action,
                );
                ensure(
                    mine.enqueue(from_b) == Err(Error::WrongScope),
                    &format!("seed {seed}: tenant B's session was accepted by tenant A"),
                )?;
            } else if mine.enqueue(delta(&mine, action)?).is_ok() {
                own_accepted = own_accepted.saturating_add(1);
            } else {
                present(&mut mine, 0.01)?;
            }
            if random.below(10) == 0 {
                present(&mut theirs, 0.01)?;
            }
        }
        ensure(
            mine.stats().accepted() == own_accepted
                && theirs.stats().accepted() == 0
                && theirs.revision() == 0,
            &format!("seed {seed}: a neighbour's traffic changed a scene it never addressed"),
        )?;
    }
    Ok(())
}

/// Bug caught: an off-by-one at the queue bound (limit vs limit+1), or a
/// queue that does not recover after the frame drains it.
#[test]
fn sim_canvas_queue_limit_and_limit_plus_one_then_recovery() -> Result<(), String> {
    for bound in [1_usize, 2, 31, 32, 255, 256] {
        let mut scene = scene("tenant-queue", limits(bound)?)?;
        for _ in 0..bound {
            scene
                .enqueue(delta(&scene, Action::ToggleReducedMotion)?)
                .map_err(describe)?;
        }
        let full = snapshot(&scene)?;
        for _ in 0..10 {
            ensure(
                scene.enqueue(delta(&scene, Action::ToggleReducedMotion)?) == Err(Error::QueueFull),
                &format!("bound {bound}: the extra delta was not QueueFull"),
            )?;
        }
        ensure(
            snapshot(&scene)? == full,
            &format!("bound {bound}: refusals grew the queue"),
        )?;
        present(&mut scene, 0.01)?;
        ensure(
            scene.stats().drained() == u64::try_from(bound).map_err(describe)?
                && scene.revision() == scene.stats().drained(),
            &format!("bound {bound}: the frame did not drain exactly the queue"),
        )?;
        scene
            .enqueue(delta(&scene, Action::ToggleReducedMotion)?)
            .map_err(describe)?;
    }
    let mut idle = scene("tenant-idle", limits(4)?)?;
    ensure(
        idle.prepare(0.0, 900, 600, false)
            .map_err(describe)?
            .is_some(),
        "a fresh scene owes its first frame",
    )?;
    present(&mut idle, 0.0)?;
    for _ in 0..5 {
        ensure(
            !idle.needs_frame()
                && idle
                    .prepare(0.0, 900, 600, false)
                    .map_err(describe)?
                    .is_none(),
            "an idle scene must not manufacture frames",
        )?;
    }
    Ok(())
}

/// Run the scene beside the model on one seed and hash the frames.
fn run_against_model(seed: u64) -> Result<u64, String> {
    const BOUND: u64 = 8;
    let mut random = SimRng::seed(seed);
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut scene = scene("tenant-model", limits(8)?)?;
    let mut model = Model::new();
    for step in 0..300_u64 {
        let label = format!("seed {seed} step {step}");
        match random.below(10) {
            0..=4 => {
                let action = draw_action(&mut random);
                let outcome = scene.enqueue(delta(&scene, action)?);
                if model.pending() >= BOUND {
                    ensure(
                        outcome == Err(Error::QueueFull),
                        &format!("{label}: wanted QueueFull"),
                    )?;
                } else {
                    ensure(
                        outcome.is_ok(),
                        &format!("{label}: the model accepts this delta"),
                    )?;
                    model.push(action);
                }
            }
            5 => {
                let base = model.next_base();
                let wrong = if base == 0 || random.below(2) == 0 {
                    base.saturating_add(1)
                } else {
                    base.saturating_sub(1)
                };
                let stale = Delta::new(
                    scene.session().clone(),
                    scene.generation(),
                    wrong,
                    Action::ToggleReducedMotion,
                );
                ensure(
                    scene.enqueue(stale) == Err(Error::Stale),
                    &format!("{label}: wanted Stale"),
                )?;
            }
            6 | 7 => {
                let seconds = f64::from(random.below_u16(51)) / 1000.0;
                let frame = scene
                    .prepare(seconds, 900, 600, true)
                    .map_err(describe)?
                    .ok_or("a forced prepare must yield a frame")?;
                model.drain()?;
                ensure(
                    frame.target() == model.target()
                        && frame.scroll() == model.scroll()
                        && frame.reduced_motion() == model.reduced()
                        && frame.commit_status() == model.visible()
                        && frame.confirmed_commit_status() == model.confirmed()
                        && frame.optimistic() == model.is_optimistic()
                        && frame.scene_revision() == model.revision(),
                    &format!("{label}: the frame left the reference model"),
                )?;
                trace.mix_bits(frame.position());
                trace.mix_bits(frame.velocity());
                if random.below(4) != 0 {
                    scene.acknowledge_present(frame.token()).map_err(describe)?;
                }
            }
            8 => {
                scene.deny_optimistic().map_err(describe)?;
                model.deny();
            }
            _ => {
                if random.below(2) == 0 {
                    scene.invalidate_surface();
                } else {
                    scene.resume_without_replay();
                }
            }
        }
        agrees(&scene, &model, &label)?;
        trace.mix_u64(scene.revision());
    }
    Ok(trace.finish())
}

/// Bug caught: any drift between the scene and the documented reducer
/// across enqueue, drain, deny, stale refusal and surface loss, with
/// extreme step and scroll amounts, and a replay hash that must repeat.
#[test]
fn sim_canvas_reducer_matches_the_reference_model_and_replays() -> Result<(), String> {
    for seed in 5_500..5_560_u64 {
        ensure(
            run_against_model(seed)? == run_against_model(seed)?,
            &format!("seed {seed}: the replay hash diverged"),
        )?;
    }
    ensure(
        run_against_model(5_500)? != run_against_model(5_501)?,
        "distinct seeds must diverge",
    )
}

/// Bug caught: prepare that drains the inbox before validating time or the
/// viewport, so a rejected frame still eats queued actions.
#[test]
fn sim_canvas_prepare_rejects_hostile_time_and_viewport_without_draining() -> Result<(), String> {
    let hostile_seconds = [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -0.001,
        60.000_001,
        f64::MAX,
    ];
    let hostile_viewports = [
        (0_u32, 600_u32),
        (900, 0),
        (8_847_361, 1),
        (u32::MAX, u32::MAX),
    ];
    let mut scene = scene("tenant-prepare", limits(8)?)?;
    for _ in 0..3 {
        scene
            .enqueue(delta(&scene, Action::ToggleReducedMotion)?)
            .map_err(describe)?;
    }
    let before = snapshot(&scene)?;
    for seconds in hostile_seconds {
        ensure(
            matches!(
                scene.prepare(seconds, 900, 600, true),
                Err(Error::InvalidInput)
            ),
            &format!("seconds {seconds}: not refused as InvalidInput"),
        )?;
    }
    for (width, height) in hostile_viewports {
        let outcome = scene.prepare(0.01, width, height, true);
        let want = if width == 0 || height == 0 {
            Error::InvalidInput
        } else {
            Error::BudgetExceeded
        };
        ensure(
            matches!(outcome, Err(ref error) if *error == want),
            &format!("viewport {width}x{height}: wanted {want}"),
        )?;
    }
    ensure(
        snapshot(&scene)? == before,
        "a rejected frame consumed queued actions",
    )?;
    for seconds in [0.0, -0.0, 60.0] {
        ensure(
            scene
                .prepare(seconds, 4_096, 2_160, true)
                .map_err(describe)?
                .is_some(),
            &format!("boundary seconds {seconds} with the full buffer were refused"),
        )?;
    }
    Ok(())
}

/// Bug caught: revoke leaving queued actions behind that a later call could
/// drain, or revoked scenes that resurrect on surface recovery.
#[test]
fn sim_canvas_revoke_discards_the_queue_and_cannot_be_undone() -> Result<(), String> {
    for seed in 5_600..5_640_u64 {
        let mut random = SimRng::seed(seed);
        let mut scene = scene("tenant-discard", limits(32)?)?;
        let queued = random.below(32).saturating_add(1);
        for _ in 0..queued {
            scene
                .enqueue(delta(&scene, draw_action(&mut random))?)
                .map_err(describe)?;
        }
        scene.revoke();
        ensure(
            scene.next_base_revision() == Ok(scene.revision())
                && scene.revision() == 0
                && scene.stats().accepted() == queued
                && scene.stats().drained() == 0,
            &format!("seed {seed}: revoke left queued actions or rewrote the counters"),
        )?;
        scene.revoke();
        scene.resume_without_replay();
        ensure(
            matches!(scene.prepare(0.0, 1, 1, true), Err(Error::Revoked)) && !scene.needs_frame(),
            &format!("seed {seed}: surface recovery resurrected a revoked scene"),
        )?;
    }
    Ok(())
}

/// Bug caught: denying optimism that lets a later delta reuse a revision an
/// earlier accepted delta already held (the next base must never fall).
#[test]
fn sim_canvas_deny_optimistic_never_reuses_an_accepted_revision() -> Result<(), String> {
    for seed in 5_700..5_760_u64 {
        let mut random = SimRng::seed(seed);
        let mut scene = scene("tenant-deny", limits(16)?)?;
        let mut accepted_bases: Vec<u64> = Vec::new();
        let mut floor = 0_u64;
        for step in 0..300_u64 {
            match random.below(5) {
                0 | 1 => {
                    let action = if random.below(2) == 0 {
                        Action::OptimisticTransition(pick(&mut random, &CommitStatus::ALL)?)
                    } else {
                        draw_action(&mut random)
                    };
                    let base = scene.next_base_revision().map_err(describe)?;
                    if scene.enqueue(delta(&scene, action)?).is_ok() {
                        accepted_bases.push(base);
                    }
                }
                2 => scene.deny_optimistic().map_err(describe)?,
                3 => {
                    present(&mut scene, 0.01)?;
                }
                _ => scene.invalidate_surface(),
            }
            let next = scene.next_base_revision().map_err(describe)?;
            ensure(
                next >= floor,
                &format!("seed {seed} step {step}: the next base fell from {floor} to {next}"),
            )?;
            floor = next;
        }
        ensure(
            accepted_bases
                .iter()
                .zip(accepted_bases.iter().skip(1))
                .all(|(earlier, later)| earlier < later),
            &format!("seed {seed}: an accepted base revision was reused"),
        )?;
    }
    Ok(())
}

/// Bug caught: an optimistic overlay shown as confirmed, so a pending claim
/// reads as the domain's truth, or a transition that fails to clear it.
#[test]
fn sim_canvas_optimistic_overlay_never_impersonates_confirmed_status() -> Result<(), String> {
    for seed in 5_800..5_860_u64 {
        let mut random = SimRng::seed(seed);
        let mut scene = scene("tenant-overlay", limits(8)?)?;
        let claimed = pick(&mut random, &CommitStatus::ALL)?;
        let confirmed = pick(&mut random, &CommitStatus::ALL)?;
        scene
            .enqueue(delta(&scene, Action::Transition(confirmed))?)
            .map_err(describe)?;
        scene
            .enqueue(delta(&scene, Action::OptimisticTransition(claimed))?)
            .map_err(describe)?;
        let pending = present(&mut scene, 0.01)?;
        ensure(
            pending.optimistic()
                && pending.commit_status() == claimed
                && pending.confirmed_commit_status() == confirmed
                && pending.status_label() != claimed.label()
                && pending.status_label().ends_with("pending"),
            &format!("seed {seed}: an overlay on {claimed:?} over {confirmed:?} read as confirmed"),
        )?;
        let after = if random.below(2) == 0 {
            scene.deny_optimistic().map_err(describe)?;
            present(&mut scene, 0.01)?
        } else {
            scene
                .enqueue(delta(&scene, Action::RevertOptimistic)?)
                .map_err(describe)?;
            present(&mut scene, 0.01)?
        };
        ensure(
            !after.optimistic()
                && after.commit_status() == confirmed
                && after.status_label() == confirmed.label(),
            &format!("seed {seed}: reverting did not restore the confirmed status"),
        )?;
    }
    Ok(())
}

/// Bug caught: step arithmetic that overflows or mishandles negatives at
/// i32::MIN and i32::MAX, so the target lands on the wrong view.
#[test]
fn sim_canvas_step_wraps_modulo_three_for_extreme_amounts() -> Result<(), String> {
    for seed in 5_900..5_960_u64 {
        let mut random = SimRng::seed(seed);
        let mut scene = scene("tenant-step", limits(8)?)?;
        let mut here = View::Source;
        for _ in 0..40 {
            let amount = wild_i32(&mut random);
            scene
                .enqueue(delta(&scene, Action::Step(amount))?)
                .map_err(describe)?;
            let frame = present(&mut scene, 0.01)?;
            let start = i64::try_from(here.index()).map_err(describe)?;
            let landed = start.saturating_add(i64::from(amount)).rem_euclid(3);
            here = View::ALL
                .get(usize::try_from(landed).map_err(describe)?)
                .copied()
                .ok_or("the step landed outside the three views")?;
            ensure(
                frame.target() == here && scene.target() == here,
                &format!(
                    "seed {seed}: step {amount} from index {start} landed on {:?}, wanted {here:?}",
                    frame.target()
                ),
            )?;
            if random.below(5) == 0 {
                here = pick(&mut random, &View::ALL)?;
                scene
                    .enqueue(delta(&scene, Action::Navigate(here))?)
                    .map_err(describe)?;
                present(&mut scene, 0.01)?;
            }
        }
    }
    Ok(())
}
