//! The smoke's human-approval section (#784), written as `await approval`: the
//! first run parks, a decision releases the payload, a denial fails it typed,
//! and the gated act the default policy guards runs only on a matching
//! approval — while an explicit policy runs the stub without asking.

use std::sync::atomic::{AtomicUsize, Ordering};

use lgwks_bot::cap::Cap;
use lgwks_bot::rt::runtime::Handle;
use lgwks_bot::script::{
    ApprovalDecision, Approved, DefaultPolicy, EffectDisposition, EffectPolicy, FlowError, Scope,
    with_effect_policy,
};
use lgwks_bot::task::{Disposition, Host, task};
use lgwks_bot::{Action, Auth, BotError, EffectLifetime, Execute};

use super::super::say;
use super::{Section, VocabularyError, refuse};

lgwks_bot::script! {
    flow vocab_ship(payload: Vec<u8>) -> Vec<u8>:
        let approved = await approval ship within 30s:
            payload
        give back approved.payload().to_vec()

    flow vocab_guarded(payload: Vec<u8>) -> Vec<u8>:
        let approved = await approval ship within 30s:
            payload
        act vocab::ship on "dock" with approved.payload().to_vec()
        give back decider(&approved)
}

/// Who decided the approval a gated run resumed on, as the bytes it gives back.
fn decider(approved: &Approved) -> Vec<u8> {
    approved.approved_by().as_bytes().to_vec()
}

/// A stub action whose effect leaves the process: the smoke's one gated act.
///
/// Its input archives as the payload's wire record, so an approval parked over
/// the same bytes releases it, and anything else does not.
struct ShipAction;

impl ShipAction {
    /// Build from any target: the dock is named, not validated.
    fn from_target(_target: &str) -> Result<Action, BotError> {
        Ok(Action::new(Self))
    }
}

/// How many times the smoke's gated action ran.
static SHIPPED: AtomicUsize = AtomicUsize::new(0);

impl Execute for ShipAction {
    /// The payload, as the approval parked it.
    type Input = Vec<u8>;
    /// It answers nothing.
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::External
    }

    fn archive_input(&self, input: &Vec<u8>) -> Option<Vec<u8>> {
        // The wire record, the same form an approval digests its payload in:
        // raw bytes would never match the approved digest, so every gated act
        // would be refused as `NeedsApproval`.
        lgwks_bot::script::Durable::to_record(input).ok()
    }

    async fn execute_action(&self, call: (Auth, &Vec<u8>)) -> Result<(), BotError> {
        call.0.check(&[])?;
        SHIPPED.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "vocab::ship"
    }
}

/// The smoke's explicit policy: the stub actions above are test doubles, so
/// an allow runs them without parking a decision.
struct AllowVocab;

impl EffectPolicy for AllowVocab {
    /// Every action is reversible: the smoke owns its doubles.
    fn disposition(&self, _lifetime: EffectLifetime) -> EffectDisposition {
        EffectDisposition::Reversible
    }
}

lgwks_bot::domains! {
    /// `vocab::ship` bound to the stub the gated act runs.
    pub(crate) SHIP {
        observe {}
        execute {
            "vocab::ship" => ShipAction::from_target,
        }
    }
}

/// Drive the approval section over a fresh store of its own.
///
/// # Errors
///
/// The first failed check, or the runtime error the run returned.
pub(super) fn run(driver: &Handle) -> Result<(), VocabularyError> {
    let unique = lgwks_std::random::bytes::<8>().map_err(std::io::Error::other)?;
    let hex: String = unique.iter().map(|byte| format!("{byte:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("lgwks-estate-vocab-{hex}"));
    std::fs::create_dir_all(&dir)?;
    let _keep = super::super::Scratch(dir.clone());
    let host = Host::builder("vocab")?
        .run_store(&dir)?
        .domains(&SHIP)
        .build()?;
    let shipping = task("vocab-ship", |scope: Scope, payload: Vec<u8>| async move {
        vocab_ship(&scope, payload).await
    })?;
    let payload = b"release-42".to_vec();
    let parked = driver.block_on(host.run(&shipping, payload.clone()));
    if !matches!(parked.disposition(), Disposition::Parked) {
        return refuse(Section::Approval, "the first run parks");
    }
    let Some(parked_error) = parked.error() else {
        return refuse(Section::Approval, "the park names its id");
    };
    let id = match *parked_error {
        FlowError::Parked { ref id, .. } => id.clone(),
        _ => return refuse(Section::Approval, "the park names its id"),
    };
    say(&format!("approval: parked as {id}"))?;
    let listed = host.pending(100)?;
    if listed.len() != 1 || listed[0].id() != id {
        return refuse(Section::Approval, "the park is listed once under its id");
    }
    let digest = *listed[0].digest();
    host.decide(&id, ApprovalDecision::approve("vocab-smoke", digest))?;
    let Some(run) = parked.run_id() else {
        return refuse(Section::Approval, "a parked run names its run");
    };
    let resumed = driver.block_on(host.resume(run, &shipping, payload.clone()));
    if !matches!(resumed.disposition(), Disposition::Succeeded)
        || resumed.output() != Some(&payload)
    {
        return refuse(Section::Approval, "the decision releases the payload");
    }
    // A denial fails the resume typed, and runs nothing after the park.
    let parked_denied = driver.block_on(host.run(&shipping, b"hold-7".to_vec()));
    let Some(denied_error) = parked_denied.error() else {
        return refuse(Section::Approval, "a denial starts from a park");
    };
    let denied_id = match *denied_error {
        FlowError::Parked { ref id, .. } => id.clone(),
        _ => return refuse(Section::Approval, "a denial starts from a park"),
    };
    let denied_digest = host
        .pending(100)?
        .into_iter()
        .find(|entry| entry.id() == denied_id)
        .map(|entry| *entry.digest())
        .ok_or(VocabularyError::Check {
            section: Section::Approval,
            check: "the denied park is listed",
        })?;
    host.decide(
        &denied_id,
        ApprovalDecision::deny("vocab-smoke", denied_digest),
    )?;
    let Some(denied_run) = parked_denied.run_id() else {
        return refuse(Section::Approval, "a denied park names its run");
    };
    match driver.block_on(host.resume(denied_run, &shipping, b"hold-7".to_vec())) {
        resumed
            if matches!(
                resumed.error(),
                Some(error) if matches!(*error, FlowError::ApprovalDenied { .. })
            ) => {}
        _ => return refuse(Section::Approval, "a denial fails the resume typed"),
    }
    // The gated act under the default policy: parked, decided, resumed, and
    // the action runs exactly once, on the approved bytes.
    let guarding = task(
        "vocab-guarded",
        |scope: Scope, payload: Vec<u8>| async move { vocab_guarded(&scope, payload).await },
    )?;
    let parked_guarded = driver.block_on(host.run(&guarding, b"ship-it".to_vec()));
    let Some(guarded_error) = parked_guarded.error() else {
        return refuse(Section::Approval, "a gated run starts from a park");
    };
    let guarded_id = match *guarded_error {
        FlowError::Parked { ref id, .. } => id.clone(),
        _ => return refuse(Section::Approval, "a gated run starts from a park"),
    };
    let guarded_digest = host
        .pending(100)?
        .into_iter()
        .find(|entry| entry.id() == guarded_id)
        .map(|entry| *entry.digest())
        .ok_or(VocabularyError::Check {
            section: Section::Approval,
            check: "the gated park is listed",
        })?;
    host.decide(
        &guarded_id,
        ApprovalDecision::approve("vocab-smoke", guarded_digest),
    )?;
    let Some(guarded_run) = parked_guarded.run_id() else {
        return refuse(Section::Approval, "a gated park names its run");
    };
    // Named rather than left implicit: the policy every host runs without one.
    let resumed_guarded = driver.block_on(with_effect_policy(
        DefaultPolicy::default(),
        host.resume(guarded_run, &guarding, b"ship-it".to_vec()),
    ));
    if !matches!(resumed_guarded.disposition(), Disposition::Succeeded)
        || resumed_guarded.output().map(Vec::as_slice) != Some(b"vocab-smoke".as_slice())
    {
        // The smoke names what the run said, so a refusal here is diagnosable
        // from its output alone.
        say(&format!(
            "approval: the gated resume ended {:?}: {:?}",
            resumed_guarded.disposition(),
            resumed_guarded.error()
        ))?;
        return refuse(Section::Approval, "the approved run succeeds");
    }
    if SHIPPED.load(Ordering::SeqCst) != 1 {
        return refuse(
            Section::Approval,
            "the gated action runs exactly once on the approved bytes",
        );
    }
    // The same run under an explicit policy: the stub is a test double, so no
    // decision is owed.
    let parked_open = driver.block_on(host.run(&guarding, b"ship-open".to_vec()));
    let Some(open_error) = parked_open.error() else {
        return refuse(Section::Approval, "an allowed run starts from a park");
    };
    let open_id = match *open_error {
        FlowError::Parked { ref id, .. } => id.clone(),
        _ => return refuse(Section::Approval, "an allowed run starts from a park"),
    };
    let open_digest = host
        .pending(100)?
        .into_iter()
        .find(|entry| entry.id() == open_id)
        .map(|entry| *entry.digest())
        .ok_or(VocabularyError::Check {
            section: Section::Approval,
            check: "the allowed park is listed",
        })?;
    host.decide(
        &open_id,
        ApprovalDecision::approve("vocab-smoke", open_digest),
    )?;
    let Some(open_run) = parked_open.run_id() else {
        return refuse(Section::Approval, "an allowed park names its run");
    };
    let resumed_open = driver.block_on(with_effect_policy(
        AllowVocab,
        host.resume(open_run, &guarding, b"ship-open".to_vec()),
    ));
    if !matches!(resumed_open.disposition(), Disposition::Succeeded)
        || SHIPPED.load(Ordering::SeqCst) != 2
    {
        return refuse(
            Section::Approval,
            "an explicit policy runs the stub without asking",
        );
    }
    say(&format!(
        "approval: parked, decided, resumed; gated action ran {}",
        SHIPPED.load(Ordering::SeqCst)
    ))?;
    Ok(())
}
