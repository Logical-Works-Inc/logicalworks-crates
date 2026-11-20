//! The refusal families: each drift kind is refused with its own typed axis and exact payload, before any body runs and without moving a byte.

use super::*;

/// Every drift kind is refused with its own typed axis, and the control is not.
fn drift_kinds_are_refused_typed(band: sim::Band) -> TestResult {
    let body = |sim: &mut sim::Sim| -> TestResult {
        for tenant in TENANTS {
            let kind = drawn(sim, &DriftKind::DRIFTED)?;
            let dir = Scratch(sim.scratch("drift")?);
            let observed = attempt(&dir.0, tenant, FIRST_INPUT, FIRST_CODEC, kind)?;

            sim.record(&format!("{tenant}:{}", kind.tag()));
            sim.trace.record_number(
                "disposition",
                shared::disposition_code(observed.disposition),
            );

            assert_eq!(
                observed.disposition,
                Disposition::Refused,
                "{tenant}/{}: a drifted resume must be refused, not run",
                kind.tag()
            );
            assert_eq!(
                observed.drift,
                kind.expects(),
                "{tenant}/{}: the refusal must name this axis, saw {:?}",
                kind.tag(),
                observed.drift
            );
            assert_eq!(
                observed.records,
                STEPS,
                "{tenant}/{}: the refusal wrote a record",
                kind.tag()
            );
            assert_eq!(
                observed.constructions,
                0,
                "{tenant}/{}: a refused resume constructed a step body, so it was not refused before the body",
                kind.tag()
            );
            assert_eq!(
                observed.markers,
                (0, 0),
                "{tenant}/{}: a refused resume ran a step",
                kind.tag()
            );
        }
        Ok(())
    };
    sim::assert_replays(band, body)
}

/// The four axes are distinguishable, not one refusal wearing four names.
///
/// A caller has to know whether it needs a new run, a decision, a migration or
/// an edit, which is only possible if each kind reports its own axis.
fn every_axis_is_distinguishable(band: sim::Band) -> TestResult {
    let body = |sim: &mut sim::Sim| -> TestResult {
        let mut seen: Vec<&'static str> = Vec::new();
        for kind in DriftKind::ALL {
            let dir = Scratch(sim.scratch("axes")?);
            let observed = attempt(&dir.0, TENANTS[0], FIRST_INPUT, FIRST_CODEC, kind)?;
            if let Some(axis) = observed.drift {
                assert!(
                    !seen.contains(&axis),
                    "{axis} was reported twice: the axes are not distinguishable"
                );
                seen.push(axis);
            }
            sim.trace.record_number("axes", u64::try_from(seen.len())?);
        }
        assert_eq!(
            seen.len(),
            DriftKind::ALL.len().saturating_sub(1),
            "every drifted kind must report an axis, and each must be its own"
        );
        Ok(())
    };
    sim::assert_replays(band, body)
}

/// One tenant of the exact-replay sweep.
fn exact_tenant(sim: &mut sim::Sim, tenant: &str) -> TestResult {
    let dir = Scratch(sim.scratch("exact")?);
    let host = host_for(tenant, &dir.0)?;
    let work = drift_task()?;
    let first = first_identity(&host, FIRST_INPUT, FIRST_CODEC);
    let recorded_input = first.input();
    let job = Job {
        input: FIRST_INPUT,
        dir: dir.0.clone(),
    };
    let initial = lgwks_bot::script::block_on(host.run_under(&first, &work, job.clone()));
    assert_eq!(
        initial.disposition(),
        Disposition::Succeeded,
        "{tenant}: the first attempt must succeed before it can be drifted"
    );
    let run = initial
        .run_id()
        .ok_or("a stored run must name its run id")?;
    drop(initial);
    drop(host);

    let reopened = host_for(tenant, &dir.0)?;
    for kind in DriftKind::DRIFTED {
        let drifted = kind.declare(&first);
        let resumed =
            lgwks_bot::script::block_on(reopened.resume_under(run, &drifted, &work, job.clone()));
        let error = resumed.error().ok_or_else(|| -> Box<dyn Error> {
            format!("{tenant}/{}: a drifted resume must be refused", kind.tag()).into()
        })?;
        let drift = drift_of(error).ok_or_else(|| -> Box<dyn Error> {
            format!(
                "{tenant}/{}: the refusal must be FlowError::Incompatible",
                kind.tag()
            )
            .into()
        })?;
        sim.record(&format!("{tenant}:{}", kind.tag()));
        sim.trace.record_number("axis", axis_id(drift.kind()));
        assert_exact_drift(tenant, kind, drift, recorded_input)?;
    }
    Ok(())
}

/// Each axis is refused with its exact typed `Drift`, payloads included.
///
/// The typed half of T15 and R2. [`drift_axis`] checks *which* axis a refusal
/// named; this family checks *what it said*, by requiring
/// [`FlowError::Incompatible`] and destructuring the `Drift` down to the two
/// values the axis moved. A refusal that reported the right axis word while
/// carrying another run's revision, another input's digest, another flow's step
/// count or another codec id would satisfy an axis-only check and fail here,
/// which is what makes the payload part of the claim rather than decoration.
///
/// Every attempt goes through `Host::resume_under` over a **reopened** file
/// store — the first host is dropped and the resume is on a fresh handle over the
/// same bytes — so the typed refusal is the one a restart meets, not one a warm
/// in-memory index could have answered.
fn every_axis_is_refused_with_its_exact_drift(band: sim::Band) -> TestResult {
    let body = |sim: &mut sim::Sim| -> TestResult {
        for tenant in TENANTS {
            exact_tenant(sim, tenant)?;
        }
        Ok(())
    };
    sim::assert_replays(band, body)
}

/// One tenant of the drifted-resume sweep.
fn drifted_tenant(sim: &mut sim::Sim, tenant: &str) -> TestResult {
    let kind = drawn(sim, &DriftKind::DRIFTED)?;
    let dir = Scratch(sim.scratch("bytes")?);
    let path = store_path(&dir.0, tenant);

    let host = host_for(tenant, &dir.0)?;
    let work = drift_task()?;
    let first = first_identity(&host, FIRST_INPUT, FIRST_CODEC);
    let drifted = kind.declare(&first);
    let job = Job {
        input: FIRST_INPUT,
        dir: dir.0.clone(),
    };
    let initial = lgwks_bot::script::block_on(host.run_under(&first, &work, job.clone()));
    let run = initial
        .run_id()
        .ok_or("a stored run must name its run id")?;
    drop(initial);
    drop(host);

    let before = std::fs::read(&path)?;
    let reopened = host_for(tenant, &dir.0)?;
    let refused = lgwks_bot::script::block_on(reopened.resume_under(run, &drifted, &work, job));

    assert_eq!(
        refused.disposition(),
        Disposition::Refused,
        "{tenant}/{}: a drifted resume must be refused",
        kind.tag()
    );
    assert_eq!(
        refused.error().and_then(drift_axis),
        kind.expects(),
        "{tenant}/{}: the refusal must name this axis",
        kind.tag()
    );
    assert_eq!(
        std::fs::read(&path)?,
        before,
        "{tenant}/{}: the refusal moved bytes in the store",
        kind.tag()
    );

    sim.record(&format!("{tenant}:{}", kind.tag()));
    sim.trace
        .record_number("store-bytes", u64::try_from(before.len())?);
    Ok(())
}

/// A refused resume leaves the store byte-identical.
///
/// The device half of "before any new effect": a typed refusal that wrote a
/// partial frame, repaired a torn tail or moved the tail would change what a
/// restart recovers, so the file is compared against what it was before the
/// refusal rather than against its record count.
fn a_refusal_leaves_the_store_byte_identical(band: sim::Band) -> TestResult {
    let body = |sim: &mut sim::Sim| -> TestResult {
        for tenant in TENANTS {
            drifted_tenant(sim, tenant)?;
        }
        Ok(())
    };
    sim::assert_replays(band, body)
}
band_family::band_family! {
    drift_kinds_are_refused_typed_band_00 => drift_kinds_are_refused_typed, 0;
    drift_kinds_are_refused_typed_band_01 => drift_kinds_are_refused_typed, 1;
    drift_kinds_are_refused_typed_band_02 => drift_kinds_are_refused_typed, 2;
    drift_kinds_are_refused_typed_band_03 => drift_kinds_are_refused_typed, 3;
    every_axis_is_distinguishable_band_11_t15 => every_axis_is_distinguishable, 11;
    a_refusal_leaves_the_store_byte_identical_band_12 => a_refusal_leaves_the_store_byte_identical, 12;
    a_refusal_leaves_the_store_byte_identical_band_13 => a_refusal_leaves_the_store_byte_identical, 13;
    every_axis_is_refused_with_its_exact_drift_band_20 => every_axis_is_refused_with_its_exact_drift, 20;
    every_axis_is_refused_with_its_exact_drift_band_21 => every_axis_is_refused_with_its_exact_drift, 21;
}
