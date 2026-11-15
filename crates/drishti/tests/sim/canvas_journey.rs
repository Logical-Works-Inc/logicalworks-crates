//! Seeded canvas journeys: scripted action mixes across tenants with
//! revoke/deny/restart faults. Revision monotonicity, token freshness, and
//! tenant isolation hold on every run; the trace hash replays identically.

use drishti::canvas_protocol::SessionInvariant;
use drishti::canvas_scene::{Action, Artifact, Canvas, Delta, Limits, View};

use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// Session fixture (delegates to the shared fixtures module).
fn session(tenant: &str, session_id: &str) -> Result<SessionInvariant, String> {
    crate::fixtures::session_in(tenant, session_id)
}

/// Three-line fixture artifact shared by every journey scene.
fn fixture_artifact(limits: Limits) -> Result<Artifact, String> {
    Artifact::new(
        crate::fixtures::WORK_OBJECT,
        1,
        "line one\nline two\nline three",
        None,
        None,
        limits,
    )
    .map_err(|error| format!("{error}"))
}

/// Build one scene per tenant over the shared fixture artifact.
fn tenant_scenes(tenants: &[&str]) -> Result<Vec<Canvas>, String> {
    let limits = Limits::default();
    let mut scenes = Vec::new();
    for tenant in tenants {
        scenes.push(
            Canvas::new(
                session(tenant, crate::fixtures::SESSION_ID)?,
                1,
                fixture_artifact(limits)?,
                limits,
            )
            .map_err(|error| format!("{error}"))?,
        );
    }
    Ok(scenes)
}

/// Solo scene for the deny-and-resume journey.
fn solo_scene() -> Result<Canvas, String> {
    Canvas::new(
        session("tenant-solo", crate::fixtures::SESSION_ID)?,
        1,
        fixture_artifact(Limits::default())?,
        Limits::default(),
    )
    .map_err(|error| format!("{error}"))
}

/// Prepare one frame, hash its motion, and acknowledge presentation.
fn prepare_ack(scene: &mut Canvas, trace: &mut TraceHash) -> Result<(), String> {
    match scene.prepare(0.04, 900, 600, false) {
        Ok(Some(frame)) => {
            trace.mix_bits(frame.position());
            trace.mix_bits(frame.velocity());
            scene
                .acknowledge_present(frame.token())
                .map_err(|error| format!("{error}"))?;
        }
        Ok(None) => trace.mix_u64(0),
        Err(drishti::canvas_scene::Error::Revoked) => trace.mix_u64(2),
        Err(error) => ensure(false, || format!("unexpected prepare: {error}"))?,
    }
    Ok(())
}

/// Seeded local action.
fn action(random: &mut SimRng) -> Action {
    const VIEWS: [View; 3] = [View::Source, View::Proposal, View::Evidence];
    let view = VIEWS[random.below_len(VIEWS.len())];
    match random.below(7) {
        0 => Action::Navigate(view),
        1 => Action::Step(if random.below(2) == 0 { 1 } else { -1 }),
        2 => Action::ToggleReducedMotion,
        3 => Action::ReducedMotion(random.below(2) == 1),
        4 => {
            let magnitude = random.below_i32(41);
            Action::scroll(view, magnitude.wrapping_sub(20))
        }
        5 => Action::OptimisticTransition(drishti::canvas_scene::CommitStatus::Running),
        _ => Action::RevertOptimistic,
    }
}

/// Two-tenant journey campaign state: seeded RNG, tenant names, scenes,
/// trace, and per-side revisions.
struct Journey {
    /// Seeded RNG driving faults and choices.
    random: SimRng,
    /// Tenant names, one per scene.
    tenants: [&'static str; 2],
    /// One scene per tenant.
    scenes: Vec<Canvas>,
    /// Observable trace hash.
    trace: TraceHash,
    /// Per-side base revisions.
    last_revision: [u64; 2],
}

/// Start a two-tenant journey campaign.
fn journey_prelude(seed: u64) -> Result<Journey, String> {
    let random = SimRng::seed(seed);
    let tenants = ["tenant-a", "tenant-b"];
    let scenes = tenant_scenes(&tenants)?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    Ok(Journey {
        random,
        tenants,
        scenes,
        trace,
        last_revision: [0_u64, 0_u64],
    })
}

/// Run one journey from `seed` and hash the trace.
fn run_journey(seed: u64) -> Result<u64, String> {
    let Journey {
        mut random,
        tenants,
        mut scenes,
        mut trace,
        mut last_revision,
    } = journey_prelude(seed)?;
    let mut revoked_side: Option<usize> = None;
    for step in 0..200_u64 {
        let side = random.below_len(tenants.len());
        if step == 60 {
            let scene = scenes
                .get_mut(side)
                .ok_or_else(|| String::from("side out of range"))?;
            scene.revoke();
            revoked_side = Some(side);
            trace.mix_str("revoke");
            continue;
        }
        if step == 61 {
            // The revoked side must refuse even a fresh delta.
            let revoked = revoked_side.ok_or_else(|| String::from("no revocation to probe"))?;
            let probed = scenes
                .get_mut(revoked)
                .ok_or_else(|| String::from("revoked side out of range"))?;
            let base = probed
                .next_base_revision()
                .map_err(|error| format!("{error}"))?;
            let probe = probed.enqueue(Delta::new(
                session(tenants[revoked], crate::fixtures::SESSION_ID)?,
                probed.generation(),
                base,
                Action::Navigate(View::Source),
            ));
            ensure(probe == Err(drishti::canvas_scene::Error::Revoked), || {
                String::from("revoked scene must refuse")
            })?;
            trace.mix_str("revoked-refused");
            continue;
        }
        let scene = scenes
            .get_mut(side)
            .ok_or_else(|| String::from("side out of range"))?;
        // Faults: deny optimism, restart the surface.
        if step == 120 {
            match scene.deny_optimistic() {
                Ok(()) => trace.mix_u64(3),
                Err(drishti::canvas_scene::Error::Revoked) => trace.mix_u64(2),
                Err(error) => ensure(false, || format!("unexpected denial: {error}"))?,
            }
            trace.mix_str("deny");
        }
        if step == 150 {
            scene.resume_without_replay();
            trace.mix_str("resume");
        }
        ensure(scene.revision() >= last_revision[side], || {
            String::from("revision must be monotonic")
        })?;
        last_revision[side] = scene.revision();
        let base = scene
            .next_base_revision()
            .map_err(|error| format!("{error}"))?;
        let tenant = tenants[side];
        let refused = scene.enqueue(Delta::new(
            session(tenant, crate::fixtures::SESSION_ID)?,
            scene.generation(),
            base,
            action(&mut random),
        ));
        match refused {
            Ok(()) => trace.mix_u64(1),
            Err(drishti::canvas_scene::Error::Revoked) => trace.mix_u64(2),
            Err(error) => ensure(false, || format!("unexpected refusal: {error}"))?,
        }
        if step.wrapping_rem(5) == 0 {
            prepare_ack(&mut *scene, &mut trace)?;
        }
    }
    Ok(trace.finish())
}

/// Same seed replays the same hash, faults included.
#[test]
fn sim_canvas_journey_replays_identically() -> Result<(), String> {
    for seed in [17_u64, 23, 101] {
        if run_journey(seed)? != run_journey(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_journey(17)? == run_journey(18)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}

/// Run one isolation journey from `seed`: two tenants act on their own
/// scenes while a mid-run revoke fires; revisions stay monotonic per scene
/// and every scene keeps its own tenant.
fn run_isolation(seed: u64) -> Result<u64, String> {
    let Journey {
        mut random,
        tenants,
        mut scenes,
        mut trace,
        mut last_revision,
    } = journey_prelude(seed)?;
    for step in 0..200_u64 {
        let side = random.below_len(tenants.len());
        if step == 90 {
            let scene = scenes
                .get_mut(side)
                .ok_or_else(|| String::from("side out of range"))?;
            scene.revoke();
            trace.mix_str("revoke");
            continue;
        }
        let scene = scenes
            .get_mut(side)
            .ok_or_else(|| String::from("side out of range"))?;
        ensure(scene.revision() >= last_revision[side], || {
            String::from("revision must be monotonic")
        })?;
        last_revision[side] = scene.revision();
        ensure(scene.session().tenant_id() == tenants[side], || {
            String::from("scene must keep its tenant")
        })?;
        let base = scene
            .next_base_revision()
            .map_err(|error| format!("{error}"))?;
        let tenant = tenants[side];
        match scene.enqueue(Delta::new(
            session(tenant, crate::fixtures::SESSION_ID)?,
            scene.generation(),
            base,
            action(&mut random),
        )) {
            Ok(()) => trace.mix_u64(1),
            Err(drishti::canvas_scene::Error::Revoked) => trace.mix_u64(2),
            Err(error) => ensure(false, || format!("unexpected refusal: {error}"))?,
        }
        if step.wrapping_rem(4) == 0 {
            match scene.prepare(0.04, 900, 600, false) {
                Ok(Some(frame)) => {
                    trace.mix_bits(frame.position());
                    scene
                        .acknowledge_present(frame.token())
                        .map_err(|error| format!("{error}"))?;
                }
                Ok(None) => trace.mix_u64(0),
                Err(drishti::canvas_scene::Error::Revoked) => trace.mix_u64(2),
                Err(error) => ensure(false, || format!("unexpected prepare: {error}"))?,
            }
        }
    }
    Ok(trace.finish())
}

/// Same seed replays the same isolation hash with identical per-tenant tracks.
#[test]
fn sim_canvas_tenant_isolation_replays_identically() -> Result<(), String> {
    for seed in [7_u64, 29, 211] {
        if run_isolation(seed)? != run_isolation(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_isolation(7)? == run_isolation(8)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}

/// Run one deny-and-resume journey from `seed`: optimism is denied often and
/// the surface restarts often; revisions stay monotonic and the trace replays.
fn run_deny_resume(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let mut scene = solo_scene()?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut last_revision = 0_u64;
    for step in 0..200_u64 {
        // Faults: deny optimism often, restart the surface often.
        if step.wrapping_rem(7) == 0 {
            match scene.deny_optimistic() {
                Ok(()) => trace.mix_u64(3),
                Err(drishti::canvas_scene::Error::Revoked) => trace.mix_u64(2),
                Err(error) => ensure(false, || format!("unexpected denial: {error}"))?,
            }
        }
        if step.wrapping_rem(11) == 0 {
            scene.resume_without_replay();
            trace.mix_str("resume");
        }
        ensure(scene.revision() >= last_revision, || {
            String::from("revision must be monotonic")
        })?;
        last_revision = scene.revision();
        let base = scene
            .next_base_revision()
            .map_err(|error| format!("{error}"))?;
        // Bias toward optimism so denials have something to deny.
        let act = if random.below(2) == 0 {
            Action::OptimisticTransition(drishti::canvas_scene::CommitStatus::Running)
        } else {
            action(&mut random)
        };
        match scene.enqueue(Delta::new(
            session("tenant-solo", crate::fixtures::SESSION_ID)?,
            scene.generation(),
            base,
            act,
        )) {
            Ok(()) => trace.mix_u64(1),
            Err(drishti::canvas_scene::Error::Revoked) => trace.mix_u64(2),
            Err(error) => ensure(false, || format!("unexpected refusal: {error}"))?,
        }
        if step.wrapping_rem(5) == 0 {
            prepare_ack(&mut scene, &mut trace)?;
        }
    }
    Ok(trace.finish())
}

/// Same seed replays the same deny-and-resume hash.
#[test]
fn sim_canvas_deny_resume_replays_identically() -> Result<(), String> {
    for seed in [5_u64, 37, 307] {
        if run_deny_resume(seed)? != run_deny_resume(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_deny_resume(5)? == run_deny_resume(6)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}
