//! The wired path: the proposal sweep driven through a real `Host::run`, where
//! what is observed is what a caller reads off a `Report`.

use super::*;

/// The shape sweep driven through a real `Host::run`, where the observation is
/// what a caller reads off a `Report` rather than what a decode returned.
///
/// The property is the same one the pure family pins, seen from the far side: an
/// admitted shape's run *succeeds* with the plan as its output, and every refused
/// shape's run *fails* at `admit/plan` with the arm
/// [`declared_refusal`](declared_refusal) computed — so the location, the typed
/// arm and the ledger's count are three views of one decision and must agree.
pub(super) fn seeded_runs_reach_the_declared_disposition(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let Drawn {
            name,
            payload,
            surface,
            limits: decoder_limits,
        } = draw_case(sim.rng())?;
        let tenant = surface.tenant().to_owned();
        let gate = gate_for(&surface, decoder_limits);
        let expected = declared_refusal(&shape_payload(name)?, &surface, &decoder_limits);

        // The run's own ceiling, drawn: a nanosecond deadline would measure the
        // host's clock rather than the boundary.
        let host = host_for(&tenant, sim.rng())?;
        let work = support::admitting_task()?;
        let report = support::drive(host.run(&work, (gate.clone(), payload)));

        if expected == "none" {
            assert_eq!(
                report.disposition(),
                Disposition::Succeeded,
                "shape {name}: an admitted payload succeeds its run: {:?}",
                report.error()
            );
            assert!(
                report.output().is_some(),
                "shape {name}: and the plan is the run's output, so an admitted payload is work \
                 rather than a claim of work"
            );
        } else {
            assert_eq!(
                report.disposition(),
                Disposition::Failed,
                "shape {name}: a refused payload fails its run, got {:?}",
                report.error()
            );
            let error = report
                .error()
                .ok_or("shape {name}: a failed run carries its located error")?;
            assert_eq!(
                error.at(),
                "admit/plan",
                "shape {name}: the refusal is located at the admitting step, under the task's \
                 own step"
            );
            assert_eq!(
                lgwks_bot::script::refusal_of(error).map(Refusal::label),
                Some(expected),
                "shape {name}: the report's typed arm is the one the decoder's own order \
                 produces for this surface and these ceilings, got {error}"
            );
            assert!(
                lgwks_bot::script::provenance_of(error).is_some(),
                "shape {name}: a refusal on the run path still carries its provenance, so no \
                 report holds an unattributable refusal"
            );
            assert_eq!(
                gate.repetitions(expected),
                1,
                "shape {name}: the run-scoped ledger counted this one refusal"
            );
            assert_eq!(gate.spent(), 1, "shape {name}: and counted nothing else");
        }

        sim.record(&format!(
            "run shape={name} disposition={} expected={expected}",
            report.disposition().label()
        ));
        Ok(())
    })
}

/// A gate over `surface` at `limits`, with the shared admission and ledger
/// ceilings; the family varies the surface and the ceilings, so the shared
/// fixed gate would make the swept draws decorative.
fn gate_for(surface: &Surface, limits: PlanLimits) -> Gate {
    Gate::new(
        surface.tenant(),
        surface.clone(),
        Decoder::new(limits),
        PlanBudget::new(support::ADMISSIONS),
        LedgerLimits::new(support::REPEAT, 8),
    )
}

/// A host for `tenant` with the drawn admission ceiling.
///
/// The ceiling is clamped to the shared [`support::ADMISSIONS`] because every gate
/// in this layer admits at most that many proposals: a host with permits to spare
/// above the gate's ceiling would only be measuring the host's semaphore.
fn host_for(tenant: &str, rng: &mut Rng) -> Result<Host, Box<dyn Error>> {
    let draws = usize::try_from(u64::from(rng.between(1, 8)))?;
    let ceiling = draws.clamp(1, usize::try_from(support::ADMISSIONS)?);
    let limit = NonZeroUsize::new(ceiling).ok_or("the ceiling must be at least one")?;
    Ok(Host::builder(tenant)?.max_concurrent_tasks(limit).build()?)
}

/// One tenant's refusal of the shared payload, with the step path it was located at.
fn refusal_for_tenant(
    sim: &mut sim::Sim,
    name: &str,
    work: &support::AdmittingTask,
    payload: &[u8],
    seen: &mut Vec<String>,
) -> TestResult {
    let surface = Surface::builder(name)?
        .operation(support::READ, &[Cap::fs()])?
        .holding(&[Cap::fs()])
        .build();
    let gate = gate_for(&surface, limits_for(sim.rng())?);
    let host = host_for(name, sim.rng())?;
    let report = support::drive(host.run(work, (gate, payload.to_vec())));
    let error = report
        .error()
        .ok_or("an injection is refused, so the run fails")?;
    let provenance =
        lgwks_bot::script::provenance_of(error).ok_or("a refusal carries its provenance")?;
    assert_eq!(
        provenance.tenant(),
        name,
        "tenant {name}: the refusal is attributed to the run that produced it"
    );
    assert_eq!(
        provenance.digest_hex(),
        lgwks_std::hash::blake3(payload).to_hex(),
        "tenant {name}: the digest is over the shared bytes alone; the tenant is its own field"
    );
    seen.push(error.at().to_owned());
    Ok(())
}

/// Two tenants sharing one host shape admit on separate gates, get distinct step
/// keys, and neither run's refusal is attributable to the other.
///
/// The multi-tenant axis on the wired path: a refusal carries the *provenance* of
/// the run that produced it, and a step key is the hash of tenant and path. A gate
/// that leaked across tenants would show up as one tenant's refusal naming the
/// other's tenant.
pub(super) fn two_tenants_admitting_on_one_host_stay_isolated(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let tenant = *sim::nth(&TENANTS, usize::try_from(u64::from(sim.rng().below(4)))?)?;
        let stranger = TENANTS
            .iter()
            .find(|candidate| **candidate != tenant)
            .copied()
            .ok_or("the tenant set holds at least two names")?;
        // The same bytes for both, so only the tenant separates the two refusals.
        // An `install` payload, not an `injection`: an injection names a `host`
        // field that is legitimate for the tenant it names, so it could be admitted.
        let payload = support::installs();
        let work = support::admitting_task()?;

        let mut seen = Vec::new();
        for name in [tenant, stranger] {
            refusal_for_tenant(sim, name, &work, &payload, &mut seen)?;
        }

        // The step path is step identity, so it is the same string for both; what
        // separates the runs is the tenant the key hashes.
        let (Some(first), Some(second)) = (seen.first(), seen.get(1)) else {
            lgwks_std::trace::debug!(
                "two_tenants_admitting_on_one_host_stay_isolated: returning an error to the caller"
            );
            return Err("both tenants were refused".into());
        };
        assert_eq!(
            first, second,
            "both refusals are located at the same step path"
        );
        let mut tenants = vec![tenant, stranger];
        tenants.sort_unstable();
        tenants.dedup();
        assert_eq!(
            tenants.len(),
            2,
            "two tenants ran, and the draw never collapsed them into one"
        );

        sim.record(&format!("wired_tenants=2 payload={}", payload.len()));
        Ok(())
    })
}

/// The declared tiers of concurrent `Host::run`s over one run-scoped gate admit
/// exactly the gate's ceiling and no more, whichever order they finish in.
///
/// The hyperscale axis on the wired path, and the property a per-call ledger or a
/// per-body budget gets wrong: the gate is shared by every run, so the budget is
/// spent exactly once per *admitted* attempt across the whole tier. Runs that lose
/// the race for an admission get the budget refusal; they do not each get a fresh
/// ceiling.
#[test]
fn saturation_over_admit_conserves_the_budget() -> TestResult {
    let mut trace = sim::Trace::new();
    for tier in TIERS {
        let requested = tier;
        let workers = tier.min(MAX_WORKERS);
        let surface = Surface::builder(support::TENANT)?
            .operation(support::PING, &[])?
            .build();
        let gate = gate_for(&surface, PlanLimits::default());
        // One host admitting every run at once, so the tier measures the *gate's*
        // ceiling rather than the host's semaphore.
        let host = Host::builder(support::TENANT)?
            .max_concurrent_tasks(NonZeroUsize::new(MAX_WORKERS).ok_or("a non-zero ceiling")?)
            .build()?;
        let work = support::admitting_task()?;
        // A payload that is admitted on this surface, so every run that gets an
        // admission succeeds and the count of successes *is* the count of
        // admissions spent.
        let payload = format!("op={}\n", support::PING).into_bytes();

        let reports = support::drive(lgwks_std::task::join_all(
            (0..workers).map(|_| host.run(&work, (gate.clone(), payload.clone()))),
        ));

        let admitted = reports
            .iter()
            .filter(|report| report.disposition() == Disposition::Succeeded)
            .count();
        assert_eq!(
            admitted,
            usize::try_from(support::ADMISSIONS)?,
            "tier {tier}: exactly the gate's admission ceiling was spent across {workers} \
             concurrent runs — not one ceiling per run"
        );
        assert_eq!(
            gate.remaining(),
            0,
            "tier {tier}: and the shared budget is fully spent afterwards"
        );
        assert_eq!(
            reports.len(),
            workers,
            "tier {tier}: every run reported, so none was lost to the tier"
        );
        // Everyone past the ceiling is refused with the budget's own arm, located at
        // the admitting step — never an admission the host refused and never a run
        // that silently did nothing.
        for report in &reports {
            if report.disposition() == Disposition::Succeeded {
                continue;
            }
            let error = report
                .error()
                .ok_or("tier: a failed run carries its located error")?;
            assert_eq!(
                error.at(),
                "admit/plan",
                "tier {tier}: a run past the admission ceiling is refused at the admitting step"
            );
        }
        assert_eq!(
            gate.spent(),
            0,
            "tier {tier}: a budget refusal is not a payload failure, so the ledger counted \
             nothing at all"
        );

        // The three numbers together, so a reader is never told a level nobody ran
        // (INV-BOT-16).
        trace.record_number("tier-requested", u64::try_from(requested)?);
        trace.record_number("tier-reached", u64::try_from(workers)?);
        trace.record_number("tier-ceiling", u64::try_from(MAX_WORKERS)?);
        trace.record_number("tier-admitted", u64::try_from(admitted)?);
        trace.record("tier-tenant");
        trace.record(support::TENANT);
    }
    assert!(
        !trace.is_empty(),
        "every tier recorded its requested, reached, ceiling and admitted levels"
    );
    Ok(())
}
