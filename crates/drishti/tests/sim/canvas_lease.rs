//! Lease, envelope and artifact sims for the canvas scene: every constructor
//! is fed weird bytes, ceilings are driven to the edge and one past it, and
//! each verdict is compared with a predicate written from the documented
//! rule rather than from the code.

use drishti::canvas_protocol::SessionInvariant;
use drishti::canvas_scene::{
    Action, Artifact, Canvas, CommitStatus, Delta, Error, Limits, PresentationLease, View,
};

use crate::canvas_kit::{artifact, limits, scene};
use crate::fixtures::{POLICY_BUNDLE, SESSION_ID, USER_SUB, WORK_OBJECT};
use crate::motion_kit::{describe, ensure, with_member};
use crate::rng::SimRng;

/// Hostile identifier text: empty, edge lengths, control bytes, bidi and
/// zero-width marks, emoji, and a long multi-byte run.
fn hostile_parts() -> Vec<String> {
    vec![
        String::new(),
        String::from("a"),
        "x".repeat(512),
        "x".repeat(513),
        String::from("bad\nid"),
        String::from("nul\0byte"),
        String::from("del\u{7f}char"),
        String::from("tab\there"),
        String::from("rtl\u{202e}override"),
        String::from("zero\u{200b}width"),
        String::from("crab\u{1f980}"),
        "\u{e9}".repeat(257),
    ]
}

/// The documented identifier rule: non-empty, at most 512 bytes, no control
/// character.
fn part_is_valid(part: &str) -> bool {
    !part.is_empty() && part.len() <= 512 && !part.chars().any(char::is_control)
}

/// Bug caught: a lease that admits an empty, overlong or control-bearing
/// identifier (log injection, truncated keys), or refuses a legal one.
#[test]
fn sim_canvas_lease_parts_match_the_validity_predicate() -> Result<(), String> {
    let parts = hostile_parts();
    let envelope = limits(8)?;
    for lease_id in &parts {
        for surface_id in &parts {
            for tenant in &parts {
                let session = SessionInvariant::new(
                    String::from(WORK_OBJECT),
                    String::from(SESSION_ID),
                    String::from(POLICY_BUNDLE),
                    tenant.clone(),
                    String::from(USER_SUB),
                );
                let Ok(session) = session else {
                    ensure(
                        tenant.is_empty() || tenant.len() > 512,
                        "the session refused a tenant the documented rule allows",
                    )?;
                    continue;
                };
                for generation in [0_u64, 1, u64::MAX] {
                    let lease = PresentationLease::new(
                        lease_id,
                        surface_id,
                        session.clone(),
                        generation,
                        envelope,
                    );
                    let want = generation != 0
                        && part_is_valid(lease_id)
                        && part_is_valid(surface_id)
                        && part_is_valid(tenant);
                    ensure(
                        lease.is_ok() == want,
                        &format!(
                            "lease ({lease_id:?}, {surface_id:?}, {tenant:?}, {generation}): verdict {} but rule says {want}",
                            lease.is_ok()
                        ),
                    )?;
                    if let Err(error) = lease {
                        ensure(
                            error == Error::InvalidInput,
                            "a refused lease must say InvalidInput",
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Bug caught: a session decoded from the wire (serde sets the fields
/// directly, skipping the constructor's UUID checks) binds into a lease with
/// a non-UUID work object, session, policy or user, so a forged identity
/// reaches admission as a lease the scene trusts.
#[test]
fn sim_canvas_lease_refuses_wire_decoded_sessions_with_bad_identities() -> Result<(), String> {
    const FIELDS: [&str; 4] = [
        "work_object_id",
        "session_id",
        "policy_bundle_id",
        "user_sub",
    ];
    let session = SessionInvariant::new(
        String::from(WORK_OBJECT),
        String::from(SESSION_ID),
        String::from(POLICY_BUNDLE),
        String::from("tenant-wire"),
        String::from(USER_SUB),
    )
    .map_err(describe)?;
    let wire = lgwks_std::json::to_value(&session)
        .map_err(describe)?
        .to_string();
    let envelope = limits(8)?;
    let clean =
        lgwks_std::json::from_slice::<SessionInvariant>(wire.as_bytes()).map_err(describe)?;
    ensure(
        PresentationLease::new("lease", "surface", clean, 1, envelope).is_ok(),
        "a clean decoded session must bind",
    )?;
    let long = format!("\"{}\"", "a".repeat(600));
    for seed in 0..200_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(430_000));
        let field = FIELDS[random.below_len(FIELDS.len())];
        let bad = match random.below(6) {
            0 => String::from("\"not-a-uuid\""),
            1 => String::from("\"\""),
            2 => format!("\"{WORK_OBJECT}\u{200b}\""),
            3 => String::from("\"0123456789abcdef0123456789abcdef\""),
            4 => long.clone(),
            _ => format!("\"{{{SESSION_ID}}}\""),
        };
        let forged = with_member(&wire, field, &bad)?;
        let decoded =
            lgwks_std::json::from_slice::<SessionInvariant>(forged.as_bytes()).map_err(describe)?;
        ensure(
            PresentationLease::new("lease", "surface", decoded, 1, envelope)
                == Err(Error::InvalidInput),
            &format!("seed {seed}: {field} = {bad} bound into a lease"),
        )?;
    }
    let no_tenant = with_member(&wire, "tenant_id", "\"\"")?;
    let decoded =
        lgwks_std::json::from_slice::<SessionInvariant>(no_tenant.as_bytes()).map_err(describe)?;
    ensure(
        PresentationLease::new("lease", "surface", decoded, 1, envelope)
            == Err(Error::InvalidInput),
        "a decoded session with no tenant must not bind",
    )
}

/// Bug caught: envelope ceilings off by one (zero, the ceiling, ceiling+1,
/// usize::MAX), where a queue of 257 or a 1 MiB + 1 text slips through.
#[test]
fn sim_canvas_limits_new_matches_the_ceiling_predicate() -> Result<(), String> {
    let texts = [0_usize, 1, 1_048_576, 1_048_577, usize::MAX];
    let queues = [0_usize, 1, 256, 257, usize::MAX];
    let pixels = [0_usize, 1, 8_847_360, 8_847_361, usize::MAX];
    for text in texts {
        for queue in queues {
            for pixel in pixels {
                let want = (1..=1_048_576).contains(&text)
                    && (1..=256).contains(&queue)
                    && (1..=8_847_360).contains(&pixel);
                match Limits::new(text, queue, pixel) {
                    Ok(envelope) => ensure(
                        want && envelope.text_bytes() == text
                            && envelope.queued_commands() == queue
                            && envelope.pixels() == pixel
                            && envelope.validate() == Ok(envelope),
                        &format!("accepted ({text}, {queue}, {pixel}) against the ceilings"),
                    )?,
                    Err(error) => ensure(
                        !want && error == Error::InvalidInput,
                        &format!("refused ({text}, {queue}, {pixel}) with {error}"),
                    )?,
                }
            }
        }
    }
    Ok(())
}

/// Bug caught: pixel-count arithmetic that overflows or rounds, where a
/// viewport one pixel over the buffer, or u32::MAX squared, is admitted.
#[test]
fn sim_canvas_pixel_count_edges_are_exact() -> Result<(), String> {
    let envelope = Limits::default();
    let edges = [
        0_u32,
        1,
        2,
        2_160,
        3_000,
        4_096,
        4_097,
        8_847_360,
        8_847_361,
        u32::MAX,
    ];
    for width in edges {
        for height in edges {
            let product = u128::from(width).saturating_mul(u128::from(height));
            let outcome = envelope.pixel_count(width, height);
            if width == 0 || height == 0 {
                ensure(
                    outcome == Err(Error::InvalidInput),
                    &format!("{width}x{height}: a zero side must be InvalidInput"),
                )?;
            } else if product <= 8_847_360 {
                ensure(
                    outcome == Ok(usize::try_from(product).map_err(describe)?),
                    &format!("{width}x{height}: a fitting viewport must count exactly"),
                )?;
            } else {
                ensure(
                    outcome == Err(Error::BudgetExceeded),
                    &format!("{width}x{height}: an oversize viewport must be BudgetExceeded"),
                )?;
            }
        }
    }
    Ok(())
}

/// Bug caught: an artifact budget check that counts the wrong bytes or is
/// off by one, so a payload one byte over the envelope is stored.
#[test]
fn sim_canvas_artifact_payload_budget_is_exact_to_the_byte() -> Result<(), String> {
    for seed in 5_000..5_060_u64 {
        let mut random = SimRng::seed(seed);
        let source_length = random.below_len(40).saturating_add(1);
        let source = random.text(source_length);
        let proposal_length = random.below_len(40).saturating_add(1);
        let proposal = random.text(proposal_length);
        let evidence_length = random.below_len(40).saturating_add(1);
        let evidence = random.text(evidence_length);
        let total = WORK_OBJECT
            .len()
            .saturating_add(source.len())
            .saturating_add(proposal.len())
            .saturating_add(evidence.len());
        let exact = Limits::new(total, 4, 1_000).map_err(describe)?;
        let held = Artifact::new(
            WORK_OBJECT,
            1,
            &source,
            Some(&proposal),
            Some(&evidence),
            exact,
        )
        .map_err(|error| format!("seed {seed}: an exactly fitting payload was refused: {error}"))?;
        ensure(
            held.payload_bytes() == total,
            &format!(
                "seed {seed}: payload_bytes {} but the lenses sum to {total}",
                held.payload_bytes()
            ),
        )?;
        let tight = Limits::new(total.saturating_sub(1).max(1), 4, 1_000).map_err(describe)?;
        let over = Artifact::new(
            WORK_OBJECT,
            1,
            &source,
            Some(&proposal),
            Some(&evidence),
            tight,
        );
        ensure(
            matches!(over, Err(Error::BudgetExceeded)),
            &format!("seed {seed}: one byte over the envelope was stored"),
        )?;
        let roomy = limits(4)?;
        for slot in 0..3_u64 {
            let poisoned = format!("{}\0{}", &source[..1], &source[1..]);
            let attempt = match slot {
                0 => Artifact::new(
                    WORK_OBJECT,
                    1,
                    &poisoned,
                    Some(&proposal),
                    Some(&evidence),
                    roomy,
                ),
                1 => Artifact::new(
                    WORK_OBJECT,
                    1,
                    &source,
                    Some(&poisoned),
                    Some(&evidence),
                    roomy,
                ),
                _ => Artifact::new(
                    WORK_OBJECT,
                    1,
                    &source,
                    Some(&proposal),
                    Some(&poisoned),
                    roomy,
                ),
            };
            ensure(
                matches!(attempt, Err(Error::InvalidInput)),
                &format!("seed {seed}: a NUL byte in lens {slot} was stored"),
            )?;
        }
    }
    Ok(())
}

/// Bug caught: artifact keys and revisions admitted when empty, overlong,
/// control-bearing or zero, which breaks the key-equals-work-object scope.
#[test]
fn sim_canvas_artifact_key_and_revision_follow_the_rule() -> Result<(), String> {
    let roomy = limits(4)?;
    for key in hostile_parts() {
        for revision in [0_u64, 1, u64::MAX] {
            let built = Artifact::new(&key, revision, "s", None, None, roomy);
            let want = revision != 0 && part_is_valid(&key);
            ensure(
                built.is_ok() == want,
                &format!(
                    "key {key:?} revision {revision}: verdict {} but rule says {want}",
                    built.is_ok()
                ),
            )?;
            if let Ok(held) = built {
                ensure(
                    held.key() == key
                        && held.revision() == revision
                        && !held.supplied(View::Proposal),
                    "an accepted artifact must keep its key, revision and absence flags",
                )?;
            }
        }
    }
    Ok(())
}

/// Bug caught: a scene bound to an artifact of another work object, so one
/// tenant's presentation shows another object's content.
#[test]
fn sim_canvas_scope_artifact_key_must_equal_the_work_object() -> Result<(), String> {
    for seed in 5_100..5_140_u64 {
        let mut random = SimRng::seed(seed);
        let envelope = limits(4)?;
        let tenant = random.text(8);
        let session = crate::fixtures::session(&tenant)?;
        let lease =
            PresentationLease::new("lease", "surface", session, 1, envelope).map_err(describe)?;
        let stranger = artifact_for_key(
            &format!(
                "{:08x}-1111-1111-1111-111111111111",
                random.below_u32(u32::MAX)
            ),
            envelope,
        )?;
        ensure(
            matches!(
                Canvas::with_lease(lease.clone(), stranger),
                Err(Error::WrongScope)
            ),
            &format!("seed {seed}: a foreign artifact key was admitted"),
        )?;
        let native = artifact(envelope)?;
        let held = native.payload_bytes();
        let squeezed = Limits::new(held.saturating_sub(1).max(1), 4, 1_000).map_err(describe)?;
        let squeezed_lease = PresentationLease::new(
            "lease",
            "surface",
            crate::fixtures::session(&tenant)?,
            1,
            squeezed,
        )
        .map_err(describe)?;
        ensure(
            matches!(
                Canvas::with_lease(squeezed_lease, artifact(envelope)?),
                Err(Error::BudgetExceeded)
            ),
            &format!("seed {seed}: an artifact larger than the lease envelope was admitted"),
        )?;
        let foreign_and_big = artifact_for_key(WORK_OBJECT.replace('1', "2").as_str(), envelope)?;
        let squeezed_again = PresentationLease::new(
            "lease",
            "surface",
            crate::fixtures::session(&tenant)?,
            1,
            squeezed,
        )
        .map_err(describe)?;
        ensure(
            matches!(
                Canvas::with_lease(squeezed_again, foreign_and_big),
                Err(Error::WrongScope)
            ),
            &format!("seed {seed}: scope must be judged before budget"),
        )?;
        ensure(
            Canvas::with_lease(lease, artifact(envelope)?).is_ok(),
            &format!("seed {seed}: the matching artifact was refused"),
        )?;
    }
    Ok(())
}

/// An artifact under an explicit key, with the kit lenses.
fn artifact_for_key(key: &str, envelope: Limits) -> Result<Artifact, String> {
    Artifact::new(key, 1, crate::canvas_kit::SOURCE, None, None, envelope).map_err(describe)
}

/// Bug caught: a scene that trusts a session decoded from the wire, where
/// the constructor never ran, and so admits control bytes or empty tenants.
#[test]
fn sim_canvas_lease_refuses_forged_sessions_decoded_from_the_wire() -> Result<(), String> {
    let envelope = limits(4)?;
    let long = "a".repeat(513);
    for (tenant_json, admitted) in [
        ("\"tenant-ok\"", true),
        ("\"\"", false),
        ("\"bell\\u0007\"", false),
        ("\"line\\nbreak\"", false),
        ("\"nul\\u0000\"", false),
        (long.as_str(), false),
    ] {
        let tenant = if tenant_json.starts_with('"') {
            String::from(tenant_json)
        } else {
            format!("\"{tenant_json}\"")
        };
        let wire = format!(
            "{{\"work_object_id\":\"{WORK_OBJECT}\",\"session_id\":\"{SESSION_ID}\",\"policy_bundle_id\":\"{POLICY_BUNDLE}\",\"tenant_id\":{tenant},\"user_sub\":\"{USER_SUB}\"}}"
        );
        let forged: SessionInvariant = lgwks_std::json::from_str(&wire).map_err(describe)?;
        let lease = PresentationLease::new("lease", "surface", forged.clone(), 1, envelope);
        let built = Canvas::new(forged, 1, artifact(envelope)?, envelope);
        ensure(
            lease.is_ok() == admitted && built.is_ok() == admitted,
            &format!(
                "forged tenant {tenant_json}: lease {} scene {} wanted {admitted}",
                lease.is_ok(),
                built.is_ok()
            ),
        )?;
    }
    Ok(())
}

/// Bug caught: a delta aimed at another generation of the same scene being
/// accepted, so a response to a destroyed instance lands on its successor.
#[test]
fn sim_canvas_lease_generation_binds_every_delta() -> Result<(), String> {
    for generation in [1_u64, 2, 7, u64::MAX] {
        let envelope = limits(8)?;
        let session = crate::fixtures::session("tenant-gen")?;
        let lease =
            PresentationLease::new("lease", "surface", session.clone(), generation, envelope)
                .map_err(describe)?;
        let mut scene = Canvas::with_lease(lease, artifact(envelope)?).map_err(describe)?;
        ensure(
            scene.generation() == generation,
            "the scene must carry the lease generation",
        )?;
        for wrong in [
            generation.wrapping_add(1),
            generation.wrapping_sub(1),
            0,
            u64::MAX,
        ] {
            if wrong == generation {
                continue;
            }
            let attempt = scene.enqueue(Delta::new(
                session.clone(),
                wrong,
                0,
                Action::ToggleReducedMotion,
            ));
            ensure(
                attempt == Err(Error::Stale)
                    && scene.revision() == 0
                    && scene.stats().accepted() == 0,
                &format!("generation {generation}: a delta for {wrong} was not refused cleanly"),
            )?;
        }
        scene
            .enqueue(Delta::new(
                session,
                generation,
                0,
                Action::Transition(CommitStatus::Running),
            ))
            .map_err(describe)?;
        ensure(
            scene.stats().accepted() == 1,
            "the matching generation must be accepted",
        )?;
    }
    ensure(
        scene("tenant-default", limits(2)?)?.lease().is_some(),
        "a default scene carries a lease",
    )?;
    Ok(())
}
