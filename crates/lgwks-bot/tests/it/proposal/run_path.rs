//! The wired path: `script::admit` called from a real `Host::run`, and a resumed run reading back what the first run refused.

use super::*;

// ── The wired path: `script::admit` called from a real `Host::run` ───────────
//
// Everything above drives `Decoder::decode` and the ledger directly, which proves
// the boundary but not that anything calls it. These drive a real `Host::run` over
// a real store, and every assertion is about what a caller would read off the
// returned `Report` or back out of a reopened store.

// ── T26: a refusal on the run path is located, typed and attributable ────────

/// A tool-output instruction injection refused inside a real task run is located
/// at the admitting step and carries the provenance of the exact bytes, and the
/// run reports `Failed` rather than succeeding.
///
/// The whole row on the path that matters: the refusal a caller sees from a
/// `Report` is the same typed arm the decoder produced, at the same step, with the
/// same digest — not a string a `Display` had to be parsed back into.
#[test]
fn an_injected_instruction_is_refused_at_its_step_on_a_real_run() -> TestResult {
    let host = Host::builder(TENANT)?.build()?;
    let work = support::admitting_task()?;
    let payload = injection();

    let report = drive(host.run(&work, (support::gate(TENANT)?, payload.clone())));

    assert_eq!(
        report.disposition(),
        lgwks_bot::task::Disposition::Failed,
        "a refused payload fails its run rather than succeeding"
    );
    let error = report
        .error()
        .ok_or("a failed run carries its located error")?;
    assert_eq!(
        error.at(),
        "admit/plan",
        "the refusal is located at the admitting step, under the task's own step"
    );
    let refusal =
        lgwks_bot::script::refusal_of(error).ok_or("the failure is the typed refusal arm")?;
    assert!(
        matches!(refusal, Refusal::SandboxEscape { .. }),
        "an injection aimed at another tenant stays an observable SandboxEscape, not a \
         generic parse failure: {refusal}"
    );

    // The provenance travels with the refusal: the same bytes, the same digest.
    let provenance =
        lgwks_bot::script::provenance_of(error).ok_or("a refusal carries its provenance")?;
    assert_eq!(
        provenance.source(),
        Source::Model,
        "the bytes are attributed to the untrusted producer they came from"
    );
    assert_eq!(
        provenance.tenant(),
        TENANT,
        "and to the tenant whose run refused them"
    );
    assert_eq!(
        provenance.digest_hex(),
        lgwks_std::hash::blake3(&payload).to_hex(),
        "and to the exact bytes admitted, so the same refusal reproduces from the same payload"
    );
    Ok(())
}

/// A run whose body admits a well-formed proposal succeeds and its report carries
/// the step, so the refusal path above is not the only thing `admit` does.
///
/// Without this, a falsifier that only ever saw refusals could not tell a wired
/// step from one that refuses everything.
#[test]
fn a_well_formed_proposal_is_admitted_on_a_real_run() -> TestResult {
    let host = Host::builder(TENANT)?.build()?;
    let work = support::admitting_task()?;

    let report = drive(host.run(&work, (support::gate(TENANT)?, well_formed())));

    assert_eq!(
        report.disposition(),
        lgwks_bot::task::Disposition::Succeeded,
        "a proposal naming only registered operations is admitted: {:?}",
        report.error()
    );
    assert_eq!(
        report.output().map(String::as_str),
        Some("read-report [Partial]"),
        "the run's output is the plan itself, and its coverage is partial however the \
         payload spelled its claim"
    );
    assert!(
        report.steps().iter().any(|step| &**step == "admit/plan"),
        "and the admitting step is in the run's trail: {:?}",
        report.steps()
    );
    Ok(())
}

/// A capability the run does not hold is refused by name, on the run path, through
/// the same typed arm.
#[test]
fn a_capability_the_run_does_not_hold_is_refused_by_name_on_a_real_run() -> TestResult {
    let host = Host::builder(TENANT)?.build()?;
    let work = support::admitting_task()?;

    let report = drive(host.run(&work, (support::poor_gate(TENANT)?, well_formed())));

    let error = report
        .error()
        .ok_or("a run that cannot hold the capability fails")?;
    assert!(
        matches!(
            lgwks_bot::script::refusal_of(error),
            Some(Refusal::CapabilityNotHeld { .. })
        ),
        "the refusal names the capability the run lacks, so the repair is a deliberate \
         grant rather than a widened decoder: {error}"
    );
    Ok(())
}

// ── T27: a resumed run reads back what the first run refused ─────────────────

/// A run that refuses a payload records the refusal through its run store, and a
/// *new* instance resuming the same run id on a *fresh* host over the same store
/// reads it back — so the reset instance knows what was refused without ever
/// having seen the bytes.
///
/// The real context reset: a different `Host`, a different task process's worth of
/// state, the same run id. The record is read back from the *reopened* store, not
/// from the handle that wrote it.
// A host with a run store mints its run identity from `lgwks_std::random`, which
// the `ephemeral` feature provides; without it every such run is refused.
#[cfg(feature = "ephemeral")]
#[test]
fn a_resumed_run_reads_back_the_refusal_the_first_run_recorded() -> TestResult {
    let scratch = crate::scratch::Scratch::new("proposal-t27-admit")?;
    let payload = installs();

    // First instance: a host with a store, a real run, a real refusal.
    let first = Host::builder(TENANT)?.run_store(scratch.path())?.build()?;
    let work = support::admitting_task()?;
    let gate = support::gate(TENANT)?;
    let report = drive(first.run(&work, (gate, payload.clone())));
    assert_eq!(
        report.disposition(),
        lgwks_bot::task::Disposition::Failed,
        "the first instance refused the payload: {:?}",
        report.error()
    );
    let run = report
        .run_id()
        .ok_or("a host with a store names the run its records are keyed by")?;

    // The refusal is durable: it went through `remember` under the run, with an
    // `fsync`, before the run returned the refusal.
    let store = first.run_store().ok_or("a store host reports its store")?;
    assert!(
        store.record_count(run) > 0,
        "the refusal was recorded against the run before the run returned it"
    );

    // Drop the first host entirely: the reset instance shares nothing but the file.
    drop(first);

    // Second instance: a fresh `Host`, a fresh gate, the *same* run id. Nothing of
    // the first instance survives except what the run store kept.
    let second = Host::builder(TENANT)?.run_store(scratch.path())?.build()?;
    let resumed = drive(second.resume(run, &work, (support::gate(TENANT)?, well_formed())));
    assert_eq!(
        resumed.disposition(),
        lgwks_bot::task::Disposition::Succeeded,
        "the resumed run admitted a well-formed payload: {:?}",
        resumed.error()
    );

    // Reopen the store from scratch and read the recorded refusal back through the
    // public read door. This is the T27 observation: the new instance can say what
    // the first one refused, and for which bytes.
    drop(second);
    let reopened =
        lgwks_bot::task::RunStore::open(scratch.path().join(format!("{TENANT}.runstore")))?;
    let recorded = read_one_refusal(&reopened, TENANT, run)?;
    assert_eq!(
        recorded.arm(),
        Refusal::install_tool("ripgrep").label(),
        "the resumed store names the arm the first run refused, exactly as the live \
         `Refusal::label` spells it"
    );
    assert_eq!(
        recorded.digest(),
        payload_digest(&payload).to_hex(),
        "and the exact bytes it refused, so the record is attributable rather than merely \
         counted"
    );
    assert_eq!(
        recorded.source(),
        Source::Model.label(),
        "and the untrusted producer they came from"
    );
    Ok(())
}

/// Read back one recorded refusal from a reopened store, through the public door.
///
/// The step key is reconstructed from the same scope path the refusing step
/// entered — `<task>/plan/refusal` — rather than the test knowing its own key: the
/// reader is a caller, and a caller reconstructs the path it can name.
#[cfg(feature = "ephemeral")]
fn read_one_refusal(
    store: &lgwks_bot::task::RunStore,
    tenant: &str,
    run: lgwks_bot::effect::RunId,
) -> Result<lgwks_bot::script::RefusalRecord, Box<dyn Error>> {
    let scope = lgwks_bot::script::Scope::root(lgwks_bot::script::Tenant::new(tenant)?);
    let step = scope.enter("admit")?.enter("plan")?.enter("refusal")?;
    let bytes = store
        .lookup(tenant, run, step.key())?
        .ok_or("the reopened store holds no record for the refusal step")?;
    Ok(lgwks_bot::script::read_refusal(&bytes)?)
}
