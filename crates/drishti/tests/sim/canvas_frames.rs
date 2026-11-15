//! Frame, token and tenancy sims for the canvas scene: stale and forged
//! frame tokens, the present-after-success protocol, extreme scroll shapes,
//! reduced motion, hit testing against a geometry model, surface loss, and
//! 100 to 1,000 tenants interleaved against per-tenant reference models.

use drishti::canvas_scene::{
    Action, Artifact, Canvas, CommitStatus, Delta, Error, FrameToken, View,
};

use crate::canvas_kit::{Model, agrees, delta, draw_action, limits, present, scene, wild_i32};
use crate::fixtures::WORK_OBJECT;
use crate::motion_kit::{describe, ensure};
use crate::rng::{SimRng, TraceHash};

/// Bug caught: a frame token authorising a domain claim, or surviving the
/// frame that superseded it, or working on a scene it was never issued by.
#[test]
fn sim_canvas_input_rejects_transitions_stale_tokens_and_foreign_tokens() -> Result<(), String> {
    for seed in 6_000..6_060_u64 {
        let mut random = SimRng::seed(seed);
        let mut mine = scene("tenant-input", limits(8)?)?;
        let mut theirs = scene("tenant-input", limits(8)?)?;
        let token = present(&mut mine, 0.01)?.token();
        let foreign = present(&mut theirs, 0.01)?.token();
        let claim = CommitStatus::ALL[random.below_len(4)];
        for action in [
            Action::Transition(claim),
            Action::OptimisticTransition(claim),
            Action::RevertOptimistic,
        ] {
            ensure(
                mine.input(token.clone(), action) == Err(Error::WrongScope)
                    && mine.stats().accepted() == 0,
                &format!("seed {seed}: {action:?} rode in on a frame token"),
            )?;
        }
        ensure(
            mine.input(foreign, Action::ToggleReducedMotion) == Err(Error::Stale),
            &format!("seed {seed}: a token from another scene was accepted"),
        )?;
        ensure(
            mine.input(token.clone(), Action::Navigate(View::Evidence))
                .is_ok()
                && mine
                    .input(
                        token.clone(),
                        Action::scroll(View::Source, wild_i32(&mut random)),
                    )
                    .is_ok()
                && mine.stats().accepted() == 2,
            &format!("seed {seed}: a fresh token must admit local actions"),
        )?;
        present(&mut mine, 0.01)?;
        ensure(
            mine.input(token, Action::ToggleReducedMotion) == Err(Error::Stale),
            &format!("seed {seed}: a token survived the frame that superseded it"),
        )?;
    }
    Ok(())
}

/// Bug caught: acknowledging a frame that was not the last one prepared (or
/// twice, or after surface loss), so presentation counters claim frames the
/// display never showed. Written as a state machine beside the scene.
#[test]
fn sim_canvas_acknowledge_is_present_after_success_exactly_once() -> Result<(), String> {
    for seed in 6_100..6_160_u64 {
        let mut random = SimRng::seed(seed);
        let mut scene = scene("tenant-ack", limits(8)?)?;
        let mut tokens: Vec<FrameToken> = Vec::new();
        let mut live: Option<usize> = None;
        let mut presented = 0_u64;
        for step in 0..200_u64 {
            match random.below(4) {
                0 | 1 => {
                    let frame = scene
                        .prepare(0.01, 900, 600, true)
                        .map_err(describe)?
                        .ok_or("a forced prepare must yield a frame")?;
                    tokens.push(frame.token());
                    live = Some(tokens.len().saturating_sub(1));
                }
                2 if !tokens.is_empty() => {
                    let pick = random.below_len(tokens.len());
                    let outcome = scene.acknowledge_present(tokens[pick].clone());
                    if live == Some(pick) {
                        ensure(
                            outcome.is_ok(),
                            &format!("seed {seed} step {step}: the live token was refused"),
                        )?;
                        live = None;
                        presented = presented.saturating_add(1);
                    } else {
                        ensure(
                            outcome == Err(Error::Stale),
                            &format!("seed {seed} step {step}: a dead token was acknowledged"),
                        )?;
                    }
                }
                3 => {
                    scene.invalidate_surface();
                    live = None;
                }
                _ => {}
            }
            ensure(
                scene.stats().presented() == presented
                    && scene.stats().prepared() == u64::try_from(tokens.len()).map_err(describe)?,
                &format!("seed {seed} step {step}: counters left the state machine"),
            )?;
        }
        for (left, first) in tokens.iter().enumerate() {
            ensure(
                tokens
                    .iter()
                    .skip(left.saturating_add(1))
                    .all(|second| second != first),
                &format!("seed {seed}: two frames shared a token"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: scroll clamped against the wrong line count for empty,
/// trailing-newline, CRLF and blank-line texts, or overflowing at i32 edges.
#[test]
fn sim_canvas_scroll_clamps_against_the_real_line_count_of_odd_texts() -> Result<(), String> {
    let shapes = ["", "a\n", "a\r\nb\r\nc", "\n\n\n", "one", "x\ny\nz\nw\nv"];
    for text in shapes {
        let envelope = limits(16)?;
        let artifact =
            Artifact::new(WORK_OBJECT, 1, text, None, None, envelope).map_err(describe)?;
        let mut scene = Canvas::new(
            crate::fixtures::session("tenant-scroll")?,
            1,
            artifact,
            envelope,
        )
        .map_err(describe)?;
        let last = i64::try_from(text.split_terminator('\n').count().saturating_sub(1))
            .map_err(describe)?;
        let mut here = 0_i64;
        for lines in [i32::MIN, -1, 0, 1, 2, 3, 4, i32::MAX, -2, i32::MAX] {
            scene
                .enqueue(delta(&scene, Action::scroll(View::Source, lines))?)
                .map_err(describe)?;
            let frame = present(&mut scene, 0.01)?;
            here = here.saturating_add(i64::from(lines)).clamp(0, last);
            ensure(
                i64::try_from(frame.scroll()[View::Source.index()]).map_err(describe)? == here,
                &format!(
                    "text {text:?}: scroll by {lines} landed on {}, wanted {here}",
                    frame.scroll()[0]
                ),
            )?;
            ensure(
                frame.scroll()[View::Proposal.index()] == 0
                    && frame.scroll()[View::Evidence.index()] == 0,
                &format!("text {text:?}: scrolling the source moved another view"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: reduced motion that leaves the spring mid-flight, so a user
/// who asked for no motion still sees the position glide.
#[test]
fn sim_canvas_reduced_motion_lands_on_the_target_exactly() -> Result<(), String> {
    for seed in 6_200..6_260_u64 {
        let mut random = SimRng::seed(seed);
        let mut scene = scene("tenant-reduced", limits(8)?)?;
        scene
            .enqueue(delta(&scene, Action::ReducedMotion(true))?)
            .map_err(describe)?;
        for _ in 0..8 {
            let view = View::ALL[random.below_len(3)];
            scene
                .enqueue(delta(&scene, Action::Navigate(view))?)
                .map_err(describe)?;
            let frame = present(&mut scene, f64::from(random.below_u16(50)) / 1000.0)?;
            let want = f64::from(u8::try_from(view.index()).map_err(describe)?);
            ensure(
                frame.position().to_bits() == want.to_bits()
                    && frame.velocity().to_bits() == 0.0_f64.to_bits()
                    && !frame.animating()
                    && frame.reduced_motion()
                    && !scene.needs_frame(),
                &format!(
                    "seed {seed}: reduced motion left {view:?} at {}",
                    frame.position()
                ),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: a navigation spring that overshoots its target, never
/// settles, or snaps early; from rest it must approach monotonically and end
/// exactly on the view.
#[test]
fn sim_canvas_navigation_approaches_monotonically_and_settles_exactly() -> Result<(), String> {
    for seed in 6_300..6_340_u64 {
        let mut random = SimRng::seed(seed);
        let mut scene = scene("tenant-glide", limits(8)?)?;
        let view = if random.below(2) == 0 {
            View::Proposal
        } else {
            View::Evidence
        };
        let goal = f64::from(u8::try_from(view.index()).map_err(describe)?);
        scene
            .enqueue(delta(&scene, Action::Navigate(view))?)
            .map_err(describe)?;
        let mut last = 0.0_f64;
        let mut settled = false;
        for frame_number in 0..600_u64 {
            let frame = present(&mut scene, 0.004 + f64::from(random.below_u16(30)) / 1000.0)?;
            ensure(
                frame.position() >= last - 1e-12 && frame.position() <= goal + 1e-12,
                &format!(
                    "seed {seed} frame {frame_number}: position {} left [{last}, {goal}]",
                    frame.position()
                ),
            )?;
            last = frame.position();
            if !frame.animating() {
                settled = frame.position().to_bits() == goal.to_bits();
                break;
            }
        }
        ensure(
            settled,
            &format!("seed {seed}: the glide to {view:?} never settled exactly"),
        )?;
    }
    Ok(())
}

/// Bug caught: hit testing that disagrees with the bottom-bar geometry at
/// its edges (x = width, y = height - 56, -0, NaN, infinity) or its thirds.
#[test]
fn sim_canvas_hit_test_matches_the_bottom_bar_geometry() -> Result<(), String> {
    for seed in 6_400..6_460_u64 {
        let mut random = SimRng::seed(seed);
        let width = random.below_u32(1_900).saturating_add(3);
        let height = random.below_u32(900).saturating_add(60);
        let mut scene = scene("tenant-hit", limits(4)?)?;
        let frame = scene
            .prepare(0.01, width, height, true)
            .map_err(describe)?
            .ok_or("a forced prepare must yield a frame")?;
        let (wide, tall) = (f64::from(width), f64::from(height));
        for _ in 0..200 {
            let x = f64::from(random.below_u32(width.saturating_add(40))) - 20.0;
            let y = f64::from(random.below_u32(height.saturating_add(40))) - 20.0;
            let inside = x >= 0.0 && x < wide && y >= tall - 56.0 && y < tall;
            let want = if inside {
                let slot = (x * 3.0 / wide).floor();
                Some(if slot < 1.0 {
                    View::Source
                } else if slot < 2.0 {
                    View::Proposal
                } else {
                    View::Evidence
                })
            } else {
                None
            };
            ensure(
                frame.hit_test(x, y) == want,
                &format!(
                    "seed {seed}: ({x}, {y}) in {width}x{height} hit {:?}, wanted {want:?}",
                    frame.hit_test(x, y)
                ),
            )?;
        }
        let edge = tall - 56.0;
        ensure(
            frame.hit_test(-0.0, edge) == Some(View::Source)
                && frame.hit_test(0.0, edge - 0.001).is_none()
                && frame.hit_test(wide, edge).is_none()
                && frame.hit_test(1.0, tall).is_none()
                && frame.hit_test(f64::NAN, edge).is_none()
                && frame.hit_test(1.0, f64::INFINITY).is_none()
                && frame.hit_test(f64::NEG_INFINITY, edge).is_none(),
            &format!("seed {seed}: an edge of the bar in {width}x{height} disagreed"),
        )?;
    }
    Ok(())
}

/// Bug caught: frame tokens equal across scenes with the same generation,
/// serial and revision (identity ignored), or equal within one scene.
#[test]
fn sim_canvas_tokens_carry_instance_identity_and_never_repeat() -> Result<(), String> {
    for seed in 6_500..6_540_u64 {
        let mut random = SimRng::seed(seed);
        let mut first = scene("tenant-twin", limits(4)?)?;
        let mut second = scene("tenant-twin", limits(4)?)?;
        let rounds = random.below(8).saturating_add(2);
        let mut seen: Vec<FrameToken> = Vec::new();
        for _ in 0..rounds {
            let left = present(&mut first, 0.01)?.token();
            let right = present(&mut second, 0.01)?.token();
            ensure(
                left != right,
                &format!("seed {seed}: twin scenes issued equal tokens"),
            )?;
            ensure(
                !seen.contains(&left) && !seen.contains(&right),
                &format!("seed {seed}: a token repeated within a scene"),
            )?;
            seen.push(left);
            seen.push(right);
        }
        ensure(
            first.presented_token() == seen.get(seen.len().saturating_sub(2)).cloned(),
            &format!("seed {seed}: the presented token is not the last acknowledged"),
        )?;
        ensure(
            !first.needs_frame()
                && first
                    .prepare(0.0, 900, 600, false)
                    .map_err(describe)?
                    .is_none(),
            &format!("seed {seed}: an idle scene produced an unforced frame"),
        )?;
    }
    Ok(())
}

/// Bug caught: surface loss that leaves old tokens usable or forgets the
/// owed frame, and a resume that replays the glide instead of snapping.
#[test]
fn sim_canvas_surface_loss_kills_tokens_and_resume_snaps_motion() -> Result<(), String> {
    for seed in 6_600..6_640_u64 {
        let mut random = SimRng::seed(seed);
        let mut scene = scene("tenant-surface", limits(8)?)?;
        let token = present(&mut scene, 0.01)?.token();
        ensure(
            scene.presented_token().as_ref() == Some(&token) && !scene.needs_frame(),
            &format!("seed {seed}: an acknowledged scene must be idle"),
        )?;
        scene.invalidate_surface();
        ensure(
            scene.presented_token().is_none()
                && scene.needs_frame()
                && scene.input(token, Action::ToggleReducedMotion) == Err(Error::Stale),
            &format!("seed {seed}: surface loss left a usable token or no owed frame"),
        )?;
        let view = if random.below(2) == 0 {
            View::Proposal
        } else {
            View::Evidence
        };
        scene
            .enqueue(delta(&scene, Action::Navigate(view))?)
            .map_err(describe)?;
        let moving = scene
            .prepare(0.016, 900, 600, true)
            .map_err(describe)?
            .ok_or("a forced prepare must yield a frame")?;
        ensure(
            moving.animating(),
            &format!("seed {seed}: the glide to {view:?} was not in flight"),
        )?;
        scene.resume_without_replay();
        let resumed = present(&mut scene, 0.0)?;
        let goal = f64::from(u8::try_from(view.index()).map_err(describe)?);
        ensure(
            resumed.position().to_bits() == goal.to_bits() && !resumed.animating(),
            &format!(
                "seed {seed}: resume replayed the glide at {}",
                resumed.position()
            ),
        )?;
    }
    Ok(())
}

/// Walk the whole scene API on one seed, checking the counters at every
/// step, and hash what the walk observed.
fn run_monkey(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let mut scene = scene("tenant-monkey", limits(6)?)?;
    let mut tokens: Vec<FrameToken> = Vec::new();
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    {
        for step in 0..400_u64 {
            if step == 360 {
                scene.revoke();
            }
            match random.below(9) {
                0..=2 => {
                    if scene
                        .enqueue(delta(&scene, draw_action(&mut random))?)
                        .is_err()
                    {
                        trace.mix_str("refused");
                    }
                }
                3 => {
                    let seconds = match random.below(4) {
                        0 => f64::NAN,
                        _ => f64::from(random.below_u16(40)) / 1000.0,
                    };
                    if let Ok(Some(frame)) = scene.prepare(seconds, 900, 600, random.below(2) == 0)
                    {
                        trace.mix_bits(frame.position());
                        tokens.push(frame.token());
                    }
                }
                4 if !tokens.is_empty() => {
                    let pick = random.below_len(tokens.len());
                    if scene.acknowledge_present(tokens[pick].clone()).is_ok() {
                        trace.mix_str("ack");
                    }
                }
                5 if !tokens.is_empty() => {
                    let pick = random.below_len(tokens.len());
                    if scene
                        .input(tokens[pick].clone(), draw_action(&mut random))
                        .is_ok()
                    {
                        trace.mix_str("input");
                    }
                }
                6 => {
                    if scene.deny_optimistic().is_ok() {
                        trace.mix_str("deny");
                    }
                }
                7 => scene.invalidate_surface(),
                _ => scene.resume_without_replay(),
            }
            let stats = scene.stats();
            let queued = scene
                .next_base_revision()
                .map_err(describe)?
                .saturating_sub(scene.revision());
            ensure(
                stats.accepted() >= stats.drained()
                    && stats.accepted().saturating_sub(stats.drained()) >= queued
                    && stats.prepared() >= stats.presented()
                    && stats.prepared() >= stats.sampled()
                    && scene.revision() >= stats.drained()
                    && queued <= 6,
                &format!("seed {seed} step {step}: counters stopped adding up"),
            )?;
            trace.mix_u64(stats.accepted());
            trace.mix_u64(scene.revision());
        }
    }
    ensure(
        !scene.needs_frame() && scene.next_base_revision() == Ok(scene.revision()),
        &format!("seed {seed}: a revoked scene still owed work or held a queue"),
    )?;
    Ok(trace.finish())
}

/// Bug caught: counters that stop adding up under a monkey (drained more
/// than accepted, presented more than prepared, a queue count off from the
/// revision gap), state that moves after revoke, or a walk that does not
/// replay from its seed.
#[test]
fn sim_canvas_statistics_conserve_under_a_monkey_walk_and_replay() -> Result<(), String> {
    for seed in 6_700..6_740_u64 {
        ensure(
            run_monkey(seed)? == run_monkey(seed)?,
            &format!("seed {seed}: the walk did not replay"),
        )?;
    }
    ensure(
        run_monkey(6_700)? != run_monkey(6_701)?,
        "distinct seeds must diverge",
    )
}

/// Reference-checked crowd of tenants sharing one work object and session id.
fn run_crowd(tenants: usize, steps: usize, seed: u64) -> Result<(), String> {
    let mut random = SimRng::seed(seed);
    let mut scenes = Vec::new();
    let mut models = Vec::new();
    for index in 0..tenants {
        scenes.push(scene(&format!("tenant-{index}"), limits(8)?)?);
        models.push(Model::new());
    }
    let mut cross = 0_u64;
    for _ in 0..tenants.saturating_mul(steps) {
        let pick = random.below_len(tenants);
        let action = draw_action(&mut random);
        match random.below(5) {
            0 => {
                let other = random.below_len(tenants);
                if other != pick {
                    let forged = Delta::new(
                        scenes[other].session().clone(),
                        scenes[pick].generation(),
                        models[pick].next_base(),
                        action,
                    );
                    ensure(
                        scenes[pick].enqueue(forged) == Err(Error::WrongScope),
                        &format!(
                            "{tenants} tenants: tenant {other} landed a delta on tenant {pick}"
                        ),
                    )?;
                    cross = cross.saturating_add(1);
                }
            }
            1 => {
                let frame = scenes[pick]
                    .prepare(0.01, 900, 600, true)
                    .map_err(describe)?
                    .ok_or("a forced prepare must yield a frame")?;
                models[pick].drain()?;
                ensure(
                    frame.scene_revision() == models[pick].revision(),
                    &format!("{tenants} tenants: tenant {pick} drained someone else's queue"),
                )?;
            }
            _ => {
                let own = delta(&scenes[pick], action)?;
                let outcome = scenes[pick].enqueue(own);
                if models[pick].pending() >= 8 {
                    ensure(
                        outcome == Err(Error::QueueFull),
                        "a full tenant queue must say QueueFull",
                    )?;
                } else {
                    ensure(outcome.is_ok(), "an own delta below the bound must land")?;
                    models[pick].push(action);
                }
            }
        }
    }
    for (index, (live, model)) in scenes.iter().zip(models.iter()).enumerate() {
        agrees(
            live,
            model,
            &format!("{tenants} tenants, tenant {index} after {cross} cross attempts"),
        )?;
    }
    Ok(())
}

/// Bug caught: any shared state between scenes of different tenants on the
/// same work object and session id, from 100 up to 1,000 tenants.
#[test]
fn sim_canvas_tenants_100_and_1000_interleaved_match_their_own_models() -> Result<(), String> {
    run_crowd(100, 40, 6_800)?;
    run_crowd(1_000, 8, 6_801)
}
