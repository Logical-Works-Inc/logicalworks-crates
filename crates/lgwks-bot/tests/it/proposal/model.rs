//! The model double: a deterministic stub, a tighter ceiling, capless operations and empty payloads.

use super::*;

// ── The model double ─────────────────────────────────────────────────────────

/// The model is a deterministic double: the same seed is the same bytes, and a
/// different seed is different bytes.
#[test]
fn the_model_is_a_deterministic_double() -> TestResult {
    let first = StubModel::from_seed(11);
    let again = StubModel::from_seed(11);
    assert_eq!(
        first.output(),
        again.output(),
        "the same seed replays exactly"
    );
    assert_eq!(first.shape(), again.shape(), "and draws the same shape");

    let surface = surface()?;
    let decoder = decoder();
    // Whatever shape the double produced, the boundary answered it: every
    // outcome carries provenance, and an admitted one names a registered
    // operation.
    for seed in 0..64_u64 {
        let model = StubModel::from_seed(seed);
        let outcome = decoder.decode(&surface, model.output(), Source::Model);
        assert!(
            outcome.provenance().is_some(),
            "seed {seed}: every outcome names where its bytes came from"
        );
        if let Some(plan) = outcome.plan() {
            for wanted in plan.wanted() {
                assert!(
                    surface.operation(wanted.operation()).is_some(),
                    "seed {seed}: an admitted plan names only registered operations, not {:?}",
                    wanted.operation()
                );
            }
            assert!(
                !plan.coverage().is_complete(),
                "seed {seed}: no payload shape ever yields full coverage"
            );
        }
    }
    Ok(())
}

/// A decoder at a tighter ceiling refuses what the default one admits, so the
/// ceiling is a real bound rather than documentation.
#[test]
fn a_tighter_ceiling_refuses_what_the_default_admits() -> TestResult {
    let surface = surface()?;
    let payload = b"op=read-report\nnote=0123456789";
    assert!(
        decoder()
            .decode(&surface, payload, Source::Model)
            .is_admitted(),
        "the default ceiling admits this payload"
    );

    let tight = Decoder::new(PlanLimits::default().with_max_bytes(payload.len().saturating_sub(1)));
    assert!(
        matches!(
            tight.decode(&surface, payload, Source::Model).refusal(),
            Some(Refusal::Oversized { .. })
        ),
        "one byte tighter and the same payload is Oversized, with both counts named"
    );

    // And the field-count ceiling is enforced independently of the byte ceiling.
    let counted = Decoder::new(PlanLimits::default().with_max_fields(1));
    assert!(
        matches!(
            counted
                .decode(&surface, b"op=read-report\nnote=x", Source::Model)
                .refusal(),
            Some(Refusal::Limit { .. })
        ),
        "two fields against a ceiling of one is a named Limit"
    );
    Ok(())
}

/// A `PING`-only payload is admitted on a surface that registered it, which is
/// the positive half: the boundary refuses attempts, not operations.
#[test]
fn a_registered_capless_operation_is_admitted() -> TestResult {
    let surface = surface()?;
    let outcome = decoder().decode(&surface, b"op=ping", Source::Model);
    let plan = outcome
        .plan()
        .ok_or("a registered capless operation is admitted")?;
    assert_eq!(plan.wanted().len(), 1, "the plan carries the one operation");
    let wanted = plan
        .wanted()
        .first()
        .ok_or("the plan carries its operation")?;
    assert_eq!(wanted.operation(), PING, "and it is the named one");
    assert_eq!(wanted.path(), None, "with no path, because none was given");
    Ok(())
}

/// A payload naming only a note is refused as empty: a model that produced
/// nothing must not consume a repair attempt.
#[test]
fn a_payload_that_does_nothing_is_refused_as_empty() -> TestResult {
    let surface = surface()?;
    let outcome = decoder().decode(&surface, b"note=I-considered-it", Source::Model);
    assert!(
        matches!(outcome.refusal(), Some(Refusal::Empty)),
        "a comment is not a proposal: {outcome}"
    );
    Ok(())
}
