//! The resume families: a compatible resume replays without a new request, and two tenants drift independently over their own stores.

use super::*;

/// One tenant of the compatible-resume sweep.
fn compatible_tenant(sim: &mut sim::Sim, tenant: &str) -> TestResult {
    let input = sim.rng().below(64);
    let dir = Scratch(sim.scratch("compatible")?);
    let host = host_for(tenant, &dir.0)?;
    let work = drift_task()?;
    let identity = first_identity(&host, input, FIRST_CODEC);
    let expected = input.saturating_add(input.saturating_add(1));
    let job = Job {
        input,
        dir: dir.0.clone(),
    };

    let first = lgwks_bot::script::block_on(host.run_under(&identity, &work, job.clone()));
    assert_eq!(
        first.disposition(),
        Disposition::Succeeded,
        "{tenant}: the first attempt must succeed"
    );
    assert_eq!(
        first.output(),
        Some(&expected),
        "{tenant}: the first attempt's own output"
    );
    let run = first.run_id().ok_or("a stored run must name its run id")?;
    assert_eq!(
        (ran(&dir.0, "alpha")?, ran(&dir.0, "beta")?),
        (1, 1),
        "{tenant}: each durable step ran exactly once"
    );
    drop(first);
    drop(host);

    let reopened = host_for(tenant, &dir.0)?;
    let before = constructions();
    let replayed = lgwks_bot::script::block_on(reopened.resume_under(run, &identity, &work, job));

    assert_eq!(
        replayed.disposition(),
        Disposition::Succeeded,
        "{tenant}: a compatible resume must succeed"
    );
    assert_eq!(
        replayed.output(),
        Some(&expected),
        "{tenant}: the resumed output is the recorded one, not a re-run's"
    );
    assert_eq!(
        constructions().saturating_sub(before),
        0,
        "{tenant}: a compatible resume constructed a step body, so a durable step was re-requested"
    );
    assert_eq!(
        (ran(&dir.0, "alpha")?, ran(&dir.0, "beta")?),
        (1, 1),
        "{tenant}: a compatible resume re-ran a durable step"
    );
    assert_eq!(
        RunStore::open(store_path(&dir.0, tenant))?.record_count(run),
        STEPS,
        "{tenant}: a compatible resume added a record"
    );

    sim.record(tenant);
    sim.trace.record_number("input", u64::from(input));
    sim.trace.record_number("output", u64::from(expected));
    Ok(())
}

/// A compatible resume replays every recorded step and runs no body.
///
/// The second half of T15: replayed model results incur no new request. The
/// evidence is the construction counter and the marker count, both untouched by
/// a resume whose every step is recorded — and the output must still be the sum
/// the first attempt produced, which is what proves the *values* were replayed
/// rather than the bodies having run and coincidentally matched.
fn compatible_resume_replays_without_a_new_request(band: sim::Band) -> TestResult {
    let body = |sim: &mut sim::Sim| -> TestResult {
        for tenant in TENANTS {
            compatible_tenant(sim, tenant)?;
        }
        Ok(())
    };
    sim::assert_replays(band, body)
}

/// Two tenants over two stores drift independently, through every kind.
///
/// The isolation claim is that one tenant's refusal is about its own records: a
/// check that consulted the other tenant's store would either succeed on a
/// compatible resume or refuse on a drifted one for the wrong reason. Both stores
/// live in one directory, which is the arrangement `HostBuilder::run_store`
/// builds — one file per tenant, named after it.
fn tenants_drift_independently(band: sim::Band) -> TestResult {
    let body = |sim: &mut sim::Sim| -> TestResult {
        let shared = Scratch(sim.scratch("tenants")?);
        for tenant in TENANTS {
            for kind in DriftKind::ALL {
                let observed = attempt(&shared.0, tenant, FIRST_INPUT, FIRST_CODEC, kind)?;
                sim.record(&format!("{tenant}:{}", kind.tag()));
                // No drift is its own trace word, outside every axis id.
                let drift = match observed.drift {
                    Some(axis) => axis_id(axis),
                    None => 255,
                };
                sim.trace.record_number("drift", drift);

                assert_eq!(
                    observed.disposition,
                    if kind.expects().is_some() {
                        Disposition::Refused
                    } else {
                        Disposition::Succeeded
                    },
                    "{tenant}/{}: the disposition does not follow from the drift",
                    kind.tag()
                );
                assert_eq!(
                    observed.drift,
                    kind.expects(),
                    "{tenant}/{}: the refusal named the wrong axis",
                    kind.tag()
                );
                assert_eq!(
                    observed.records,
                    STEPS,
                    "{tenant}/{}: this tenant's record count moved",
                    kind.tag()
                );
            }
            assert!(
                store_path(&shared.0, tenant).is_file(),
                "{tenant}: the store is one file per tenant, named after it"
            );
        }
        Ok(())
    };
    sim::assert_replays(band, body)
}

band_family::band_family! {
    compatible_resume_replays_without_a_new_request_band_04 => compatible_resume_replays_without_a_new_request, 4;
    compatible_resume_replays_without_a_new_request_band_05 => compatible_resume_replays_without_a_new_request, 5;
    compatible_resume_replays_without_a_new_request_band_06 => compatible_resume_replays_without_a_new_request, 6;
    compatible_resume_replays_without_a_new_request_band_07 => compatible_resume_replays_without_a_new_request, 7;
    tenants_drift_independently_band_08 => tenants_drift_independently, 8;
    tenants_drift_independently_band_09 => tenants_drift_independently, 9;
    tenants_drift_independently_band_10 => tenants_drift_independently, 10;
}
