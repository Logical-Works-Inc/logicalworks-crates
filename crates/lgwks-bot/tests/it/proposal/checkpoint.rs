//! The T27 family: a context reset preserves completed work, corrections, Unknown effects and evidence, and truncated data is never full coverage.

use super::*;

// ── T27: a context reset preserves what the run already learned ──────────────

/// A context reset — a brand new task instance resuming the same run — recovers
/// completed work, user corrections, `Unknown` effects and evidence references.
// A host with a run store mints its run identity from `lgwks_std::random`, which
// the `ephemeral` feature provides; without it every such run is refused.
#[cfg(feature = "ephemeral")]
#[test]
fn a_context_reset_preserves_completed_work_corrections_unknowns_and_evidence_t27() -> TestResult {
    // The one scratch directory the durable families share: named by random
    // bytes rather than a reused pid, and removed when the guard drops.
    let scratch = crate::scratch::Scratch::new("proposal-t27-reset")?;

    let report = drive(first_instance(scratch.path()))?;
    assert_eq!(
        report.disposition(),
        lgwks_bot::task::Disposition::Succeeded,
        "the first instance built and recorded its checkpoint: {:?}",
        report.error()
    );
    let run = report
        .run_id()
        .ok_or("a host with a store names the run its records are keyed by")?;

    // The reset: a fresh Host, a fresh task, the same run id. Nothing of the
    // first instance survives except what the run store kept.
    let host = Host::builder(TENANT)?.run_store(scratch.path())?.build()?;
    let work = checkpoint_task()?;
    let resumed = drive(host.resume(run, &work, ()));
    assert_eq!(
        resumed.disposition(),
        lgwks_bot::task::Disposition::Succeeded,
        "the new instance resumed the run: {:?}",
        resumed.error()
    );
    let recovered = resumed
        .output()
        .ok_or("a resumed run that read its checkpoint produced it")?;

    assert!(
        recovered.completed("fetch"),
        "the completed step survived the reset, so it is not re-run"
    );
    assert_eq!(
        recovered.steps(),
        &["fetch".to_owned(), "analyse".to_owned()],
        "every completed step survived, in order"
    );
    assert_eq!(
        recovered.corrections().len(),
        2,
        "both user corrections survived, not just the most recent one"
    );
    assert_eq!(
        recovered.corrections().first().map(|found| found.kind()),
        Some(lgwks_bot::proposal::CorrectionKind::Refusal),
        "a refusal stays a refusal across the reset, rather than being re-read as an override"
    );
    assert_eq!(
        recovered.corrections().get(1).map(|found| found.text()),
        Some("publish as draft"),
        "the user's own words survived verbatim"
    );
    assert_eq!(
        recovered.unknowns(),
        1,
        "an effect whose outcome is Unknown is still Unknown after the reset"
    );
    assert!(
        recovered
            .effects()
            .iter()
            .any(|note| note.kind().is_unknown()),
        "and it is still an Unknown effect rather than an absent one"
    );
    assert_eq!(
        recovered.evidence(),
        &["report-1".to_owned()],
        "the evidence references survived, so a completion claim can rest on them"
    );

    // The reset is honest about what it does *not* preserve: an artifact the
    // first instance wrote is gone with it, which is why the evidence references
    // are the durable part.
    drop(host);
    Ok(())
}

/// The one task type the two instances share, so the resumed run is of the same
/// body as the run that wrote the checkpoint.
///
/// The body is a named `fn` coerced to a pointer, which is what lets the same
/// `Task` type be built twice: a closure would give each call its own anonymous
/// type, and the two instances would not be the same task.
#[cfg(feature = "ephemeral")]
type CheckpointBody =
    fn(Scope, ()) -> lgwks_bot::verb::BoxFuture<'static, Result<Checkpoint, FlowError>>;

/// The task type both instances are built at.
#[cfg(feature = "ephemeral")]
type CheckpointTask = lgwks_bot::task::Task<CheckpointBody>;

/// The task whose whole body is a durable checkpoint, so the run store is what
/// carries it across the reset.
#[cfg(feature = "ephemeral")]
fn checkpoint_task() -> Result<CheckpointTask, Box<dyn Error>> {
    // The coercion is written at the call rather than left to inference: `task`
    // is generic over its body, and without this the `?` resolves against the
    // `fn` *item* type and the annotation on the binding never applies.
    let body: CheckpointBody = checkpoint_body;
    let declared: CheckpointTask = task("checkpoint", body)?;
    Ok(declared)
}

/// The body behind [`checkpoint_task`].
///
/// It takes `scope` by value and moves it into the future, so the future owns
/// what it borrows and the returned boxed future is `'static` — which is what a
/// task body needs, since the run drives it after the body has returned.
#[cfg(feature = "ephemeral")]
fn checkpoint_body(
    scope: Scope,
    (): (),
) -> lgwks_bot::verb::BoxFuture<'static, Result<Checkpoint, FlowError>> {
    Box::pin(
        async move { remember(&scope, "ctx", || std::future::ready(build_checkpoint())).await },
    )
}

/// The checkpoint the first instance records, built fresh each time it runs.
///
/// A closure rather than a constant so the resume genuinely *reads the record*
/// instead of re-deriving the same value — which is the only way the test can
/// tell a recovered checkpoint from a recomputed one.
fn build_checkpoint() -> Result<Checkpoint, FlowError> {
    let mut checkpoint = Checkpoint::new();
    checkpoint.complete("fetch")?;
    checkpoint.complete("analyse")?;
    checkpoint.correct(
        lgwks_bot::proposal::CorrectionKind::Refusal,
        "do not publish",
    )?;
    checkpoint.correct(
        lgwks_bot::proposal::CorrectionKind::Override,
        "publish as draft",
    )?;
    checkpoint.observe_effect("merge-7", EffectNoteKind::Unknown)?;
    checkpoint.observe_effect("tag-3", EffectNoteKind::Applied)?;
    checkpoint.record_evidence("report-1")?;
    Ok(checkpoint)
}

/// The first instance: builds the checkpoint through a real host, with a real
/// store, over the real `remember` path.
// A host with a run store mints its run identity from `lgwks_std::random`, which
// the `ephemeral` feature provides; without it every such run is refused.
#[cfg(feature = "ephemeral")]
async fn first_instance(
    scratch: &std::path::Path,
) -> Result<lgwks_bot::task::Report<Checkpoint>, Box<dyn Error>> {
    let host = Host::builder(TENANT)?.run_store(scratch)?.build()?;
    let work = checkpoint_task()?;
    Ok(host.run(&work, ()).await)
}

/// Truncated data never becomes a full-coverage claim: the payload is refused,
/// and even a payload that survives as `Partial`.
#[test]
fn a_truncated_payload_never_becomes_a_full_coverage_claim() -> TestResult {
    let surface = surface()?;
    let decoder = decoder();

    // Cut short mid-document: refused as malformed or empty, never admitted.
    let cut = decoder.decode(&surface, &truncated(), Source::Model);
    assert!(
        !cut.is_admitted(),
        "a payload cut off part-way through a value is not a proposal: {cut}"
    );

    // The stronger half: a payload that *survives* and declares full coverage
    // still gets `Partial`. `Coverage::from_claim` maps every claim the decoder
    // does not recognize — including `complete` — onto the conservative value,
    // because nothing has established that the evidence is present.
    let claimed = decoder.decode(&surface, &overclaims(), Source::Model);
    let plan = claimed.plan().ok_or("a well-formed payload is admitted")?;
    assert_eq!(
        plan.coverage(),
        Coverage::Partial {
            covered: 1,
            asked: 1
        },
        "a plan may claim completeness, but never carries it: a completion claim is the only \
         thing that can establish one"
    );
    assert!(
        !plan.coverage().is_complete(),
        "a decoder cannot be talked into full coverage however many lines it carries"
    );
    assert!(
        !Coverage::from_claim(Some("complete")).is_complete(),
        "even the exact spelling `complete` maps onto Partial at the decoder"
    );
    assert_eq!(
        Coverage::from_claim(Some("totally-complete")).counts(),
        (0, 1),
        "and an unrecognized claim is the conservative value, not the convenient one"
    );
    Ok(())
}

/// A checkpoint round-trips through its archive, and a truncated archive is
/// refused rather than decoded into a partial checkpoint.
#[test]
fn a_checkpoint_round_trips_through_the_run_store() -> TestResult {
    let checkpoint = build_checkpoint()?;
    let bytes = Checkpoint::to_record(&checkpoint)?;
    let recovered = Checkpoint::from_record(&bytes)?;

    assert_eq!(
        recovered, checkpoint,
        "an archived checkpoint is the checkpoint it was archived from"
    );
    assert_eq!(
        recovered.corrections().first().map(|found| found.kind()),
        Some(lgwks_bot::proposal::CorrectionKind::Refusal),
        "a correction's kind survives the archive, not just its text"
    );

    // A truncated archive is refused, never decoded into a checkpoint that looks
    // complete: the same rule the row names about truncated data, applied to the
    // checkpoint itself.
    let cut = bytes
        .get(..bytes.len().saturating_div(2))
        .ok_or("half of the archive is inside it")?;
    assert!(
        Checkpoint::from_record(cut).is_err(),
        "a truncated archive is refused rather than decoded into a partial checkpoint"
    );
    Ok(())
}

/// A checkpoint that would grow past its ceiling is refused by name, and the
/// refusal leaves what it already held intact.
#[test]
fn a_checkpoint_refuses_to_grow_past_its_ceiling() -> TestResult {
    let mut checkpoint = Checkpoint::new();
    for index in 0..lgwks_bot::proposal::MAX_CHECKPOINT_STEPS {
        checkpoint.complete(&format!("step-{index}"))?;
    }
    assert!(
        checkpoint.complete("one-too-many").is_err(),
        "a checkpoint past its step ceiling refuses the append"
    );
    assert_eq!(
        checkpoint.steps().len(),
        lgwks_bot::proposal::MAX_CHECKPOINT_STEPS,
        "and what it already held is intact: a refusal is not a truncation"
    );

    // Recording the same step twice is a no-op, so a resumed step re-announcing
    // itself cannot inflate the count or trip the ceiling.
    assert!(
        checkpoint.complete("step-0").is_ok(),
        "re-recording a completed step is accepted"
    );
    assert_eq!(
        checkpoint.steps().len(),
        lgwks_bot::proposal::MAX_CHECKPOINT_STEPS,
        "and it did not add a second entry"
    );
    Ok(())
}
