//! T16 across two real processes: an old worker's receipt cannot authorize a
//! stale dispatch or overwrite the current owner.
//!
//! The in-process half of this row lives in `durable_dispatch.rs` and in the
//! crate's own ledger tests, and both hand the stale worker its key, its
//! authority and its journal from the same process that took the takeover. What
//! neither can observe is the thing the row is about: a takeover that crosses a
//! process boundary while the old worker is genuinely alive, holding a receipt
//! and a warrant that were both minted before it.
//!
//! Two fences are named, and this file reaches both of them through public
//! surfaces rather than letting one stand in for the other.
//!
//! - The **writer fence** ([`FileJournal::open`]'s exclusive advisory lock) stops
//!   a second *writer*. It is observed here for what it is: a held fence is
//!   refused, the holder dies, the kernel hands it back, and the new owner
//!   replays the dead worker's acknowledged appends exactly once.
//! - The **owner epoch** ([`Broker::replace`]) stops a second *claimer* from
//!   acting on a receipt minted before it. A process can hold an open
//!   descriptor for a journal it no longer owns — nothing revokes an open
//!   descriptor — so the old worker here is deliberately represented the way
//!   a returning worker actually is: **released**, not killed, with its stale
//!   warrant still in hand and its own view of the file still open.
//!
//! What the release makes possible is exactly the hazard. The released worker
//! still holds a `FileJournal` over the same path, so its appends race the
//! current owner's; which one wins is a property of the journal's
//! compare-and-append fence, and *both* answers are refused effects. Either the
//! worker's append is rejected as `TailMismatch` — the fence held — or it is
//! accepted and the current owner's next append is then rejected against the
//! stale tail the old worker installed. Both are refusals, neither replays an
//! effect, and the row asserts the current owner's own committed history and the
//! bytes on the disk are exactly what it left. What is deliberately not claimed:
//! a fork, because a single writer with a live fence cannot produce one.
//!
//! The honest limits are in the module's own words: this is one process pair on
//! one host over one local filesystem. The advisory lock is not containment
//! against a writer that never asks for it, and cross-host fencing needs a
//! lease, which this crate does not claim to provide.

// The scratch-path, kill-guard and pause fixtures this file shares with the
// journal liveness, scale, fence and crash-observation families.
//
// One definition of "a unique scratch path", of "kill and reap a child", and of
// the wait a plain process may take without an executor, so this observation
// cannot drift into asserting a different cleanup discipline than the families
// that also run real children.
use crate::journal_fixtures as shared;

use shared::{ENV, FLOW_HEX, ProbeGuard, RUN, TempGuard, pause, scratch_dir};

use lgwks_bot::broker::{Broker, BrokerError};
use lgwks_bot::effect::{
    ActionDigest, ActionId, AttemptId, EffectKey, EnvironmentEpoch, EnvironmentId, FlowRevision,
    Id128, RunId,
};
use lgwks_bot::journal::{
    EffectEvent, EffectEvidence, EffectJournal, EventKind, FileJournal, JournalError, Verification,
    VerificationResult,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Environment variable that turns this test binary into the old worker.
const OWNER_ENV: &str = "LGWKS_OWNER_PROBE";
/// Which half of the old worker this process is.
const OWNER_MODE: &str = "LGWKS_OWNER_MODE";
/// The value of [`OWNER_MODE`] that selects the return half.
const RETURNING: &str = "returning";
/// Environment variables carrying the old worker's orders and its two reports.
const OWNER_JOURNAL: &str = "LGWKS_OWNER_JOURNAL";
const OWNER_RELEASE: &str = "LGWKS_OWNER_RELEASE";
const OWNER_SETTLED: &str = "LGWKS_OWNER_SETTLED";
const OWNER_PARKED: &str = "LGWKS_OWNER_PARKED";

/// The generation the old worker prepares its attempt under.
const FIRST_GENERATION: &str = "1";
/// The generation the current owner takes over at.
const SECOND_GENERATION: &str = "2";
/// The action the two workers fight over.
const TAKEOVER_ACTION: &str = "3132333435363738393a3b3c3d3e3f40";
/// The predicate the settled attempt is verified under.
const TAKEOVER_PREDICATE: &str = "5152535455565758595a5b5c5d5e5f60";
/// The digest the takeover attempt binds to.
const DIGEST_HEX: &str = "f0f1f2f3f4f5f6f7f8f9fafbfcfdfeffe0e1e2e3e4e5e6e7e8e9eaebecedeeef";
/// The predicate the settled attempt is verified under.
const PREDICATE: &str = "5152535455565758595a5b5c5d5e5f60";

/// The key for one attempt of the takeover action at `epoch`.
///
/// One constructor for both processes, because the old worker's key and the new
/// owner's key have to be the *same* value for a stale receipt to be about the
/// current work: two spellings would let this row pass by constructing two
/// different attempts.
fn takeover_key(attempt: &str, epoch: &str) -> Result<EffectKey, Box<dyn std::error::Error>> {
    let run = RunId::from_hex(RUN)?;
    let action = ActionId::from_hex(TAKEOVER_ACTION)?;
    let attempt = AttemptId::from_decimal(attempt)?;
    let flow = FlowRevision::from_tagged("blake3_256", FLOW_HEX)?;
    let digest = ActionDigest::from_tagged("blake3_256", DIGEST_HEX)?;
    let environment = EnvironmentId::from_hex(ENV)?;
    let epoch = EnvironmentEpoch::from_decimal(epoch)?;
    Ok(
        lgwks_bot::effect::EffectIdentity::new(run, environment, flow)
            .key(action, attempt, digest, epoch),
    )
}

/// A broker that has already registered the shared environment at its first
/// generation, which is the state a worker finds on a fresh start.
fn broker_at_first_generation() -> Result<Broker, Box<dyn std::error::Error>> {
    let mut broker = Broker::new();
    broker.register(EnvironmentId::from_hex(ENV)?)?;
    Ok(broker)
}

/// The environment the two workers share, once.
fn environment() -> Result<EnvironmentId, Box<dyn std::error::Error>> {
    Ok(EnvironmentId::from_hex(ENV)?)
}

// ── The old worker ──────────────────────────────────────────────────────────

/// What the old worker managed to do, written for the parent to read.
///
/// A report file rather than the parent's own derivation, because a position the
/// parent computed and a position the worker's append acknowledged are different
/// claims and only the second is evidence.
fn old_worker_body() -> TestResult {
    let orders = Orders::from_env("the old worker")?;

    let broker = broker_at_first_generation()?;
    let key = takeover_key("1", FIRST_GENERATION)?;

    // The worker's own handle, opened under generation 1 and kept open for the
    // whole of its life. This is the shape the row is about: a descriptor on the
    // journal file survives the takeover, because nothing revokes an open
    // descriptor.
    let mut journal = FileJournal::open(&orders.journal)?;
    let authority = broker.authorize(key)?;

    // Probe phase: the dispatch the worker was in the middle of. Both rungs are
    // `fsync`-ed before their acknowledgments, and the preparation is authorized
    // against the generation this worker believes is current.
    journal.compare_and_append(journal.tail(), &EffectEvent::IntentAdmitted { key })?;
    let prepared =
        journal.compare_and_append(journal.tail(), &EffectEvent::DispatchPrepared { key })?;

    // The receipt a worker holds between "prepared" and "settled": the exact
    // position its own acknowledgment named, read back through positioned
    // readback rather than assumed.
    let receipt = journal
        .committed_entry(prepared.position())?
        .ok_or("the acknowledged preparation must be readable at its own position")?;
    if receipt.event().kind() != EventKind::DispatchPrepared {
        let refusal = Err("the receipt names a position that does not hold the preparation".into());
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "old_worker_body: returning an error to the caller");
        return refusal;
    }

    std::fs::write(&orders.parked, b"parked")?;

    // Park until the parent releases us. Bounded, so a parent that never
    // releases cannot leave a stray process behind.
    for _ in 0..600 {
        if orders.release.exists() {
            break;
        }
        pause(100);
    }
    if !orders.release.exists() {
        let refusal =
            Err("the old worker parked for its whole bound and was never released".into());
        lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "old_worker_body: returning an error to the caller");
        return refusal;
    }

    // Everything below happens *after* the takeover, and this is the row: the old
    // worker returns, and nothing it holds is allowed to act. Note that it
    // appends against *its own* view of the file, which the parent's writes have
    // already moved past — that is what a returning worker does, and what the
    // tail fence is there to answer.
    report_return(&mut journal, &broker, &authority, key, &orders.settled)
}

/// What a returning worker manages to do with what it still holds, written out
/// for the parent to read.
///
/// One reporter for both bodies, because "what the old worker was allowed to do"
/// is one question and two spellings of it would let a row pass by answering it
/// differently on the two paths.
fn report_return(
    journal: &mut FileJournal,
    broker: &Broker,
    authority: &lgwks_bot::broker::Authority,
    key: EffectKey,
    settled_path: &std::path::Path,
) -> TestResult {
    let dispatch = match broker.revalidate(authority) {
        Ok(()) => "authorized".to_owned(),
        Err(error) => format!("{error}"),
    };
    let settle = match journal.compare_and_append(
        journal.tail(),
        &EffectEvent::OutcomeObserved {
            key,
            evidence: EffectEvidence::Applied,
        },
    ) {
        Ok(ack) => format!("appended at sequence {}", ack.position().sequence()),
        Err(error) => format!("{error}"),
    };
    let verify = match journal.compare_and_append(
        journal.tail(),
        &EffectEvent::Verified {
            key,
            verification: Verification::new(
                Id128::from_hex(PREDICATE)?,
                1,
                lgwks_std::hash::blake3(b"the takeover attempt's own predicate"),
                VerificationResult::Satisfied,
            ),
        },
    ) {
        Ok(ack) => format!("appended at sequence {}", ack.position().sequence()),
        Err(error) => format!("{error}"),
    };
    let minted = match broker.authorize(key) {
        Ok(_) => "minted".to_owned(),
        Err(error) => format!("{error}"),
    };
    std::fs::write(
        settled_path,
        format!("dispatch={dispatch}\nsettle={settle}\nverify={verify}\nminted={minted}\n"),
    )?;
    Ok(())
}

/// The four paths the parent hands a worker, read from its environment.
///
/// One reader for both worker bodies. Spelled as four separate reads in each, the
/// two drift, and a path one of them forgot to read is a worker that parks
/// forever on a marker nobody writes — which fails as a hang rather than as the
/// missing variable it is.
struct Orders {
    /// Where the journal lives.
    journal: std::path::PathBuf,
    /// The file the parent creates to let the worker act.
    release: std::path::PathBuf,
    /// Where the worker reports what it was allowed to do.
    settled: std::path::PathBuf,
    /// Where the worker reports that it is alive and holding its handle.
    parked: std::path::PathBuf,
}

impl Orders {
    /// Read all four, naming `who` in the refusal so a missing variable says which
    /// process wanted it and which one it was.
    fn from_env(who: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let journal = worker_path(OWNER_JOURNAL, who, "journal")?;
        let release = worker_path(OWNER_RELEASE, who, "release")?;
        let settled = worker_path(OWNER_SETTLED, who, "settled")?;
        let parked = worker_path(OWNER_PARKED, who, "parked")?;
        Ok(Self {
            journal,
            release,
            settled,
            parked,
        })
    }
}

/// One of a worker's paths, read from the environment the parent set.
fn worker_path(
    variable: &str,
    who: &str,
    which: &str,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    Ok(std::path::PathBuf::from(
        std::env::var_os(variable)
            .ok_or_else(|| format!("{who} was started without its {which} path ({variable})"))?,
    ))
}

/// Park until the parent releases us, bounded.
fn wait_for_release(release_path: &std::path::Path) -> TestResult {
    for _ in 0..600 {
        if release_path.exists() {
            return Ok(());
        }
        pause(100);
    }
    Err("the worker parked for its whole bound and was never released".into())
}

/// Run only the return half of the old worker: open the journal under generation
/// 1, park with a warrant in hand, then re-present it and settle.
///
/// The shape the row needs and a second process can be alive through. The probe
/// half of [`old_worker_body`] is what a *first* process does and is what puts
/// the two acknowledged rungs on the disk; a returning worker comes back to a
/// journal that already holds them, and re-admitting its own attempt there would
/// be refused as out-of-order — which is a refusal of a *duplicate* identity,
/// not of a stale one, and would answer the wrong question.
fn returning_worker_body() -> TestResult {
    let orders = Orders::from_env("the returning worker")?;

    let key = takeover_key("1", FIRST_GENERATION)?;
    let mut journal = FileJournal::open(&orders.journal)?;
    // Parked *before* it asks anything, so the parent observes a returning worker
    // that is alive and holding a handle before it has learned what the disk now
    // says — which is the moment the row is about.
    std::fs::write(&orders.parked, b"parked")?;
    wait_for_release(&orders.release)?;

    // The broker is rebuilt from what is on the disk, not from a constant. This
    // is the whole of a *returning* worker: it comes back to a journal another
    // process has been writing, and the only honest answer to "which generation
    // am I" is the one the journal's own history was written at. A broker built
    // by `register` here would mint warrants for generation 1 — the generation
    // the previous owner was *replaced* past — and every fence in the row would
    // pass while fencing nothing.
    let mut broker = Broker::new();
    broker.adopt(EnvironmentId::from_hex(ENV)?, &journal)?;
    let authority = match broker.authorize(key) {
        Ok(authority) => authority,
        Err(error) => {
            // A returning worker that cannot even mint a warrant for its own
            // stale attempt reports that and stops: there is nothing left for it
            // to try, and the report is the observation.
            return std::fs::write(
                &orders.settled,
                format!("dispatch={error}\nsettle=skipped\nverify=skipped\nminted={error}\n"),
            )
            .map_err(Into::into);
        }
    };
    report_return(&mut journal, &broker, &authority, key, &orders.settled)
}

/// The old worker's report for one of the four lines it writes.
///
/// A report without the line is refused, naming the line: an empty answer would
/// reach the caller's assertion as "not replaced" and blame the broker for what
/// is the worker's missing output.
fn reported(report: &str, field: &str) -> Result<String, Box<dyn std::error::Error>> {
    report
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .map(str::to_owned)
        .ok_or_else(|| format!("the old worker reported no `{field}` line: {report}").into())
}

/// This test binary re-invoked as the named test, in probe mode.
fn spawn_old_worker(
    test_name: &str,
    mode: &str,
    journal_path: &std::path::Path,
    release_path: &std::path::Path,
    settled_path: &std::path::Path,
    parked_path: &std::path::Path,
) -> Result<ProbeGuard, Box<dyn std::error::Error>> {
    Ok(ProbeGuard(Some(
        crate::probe_command(&crate::probe_test(module_path!(), test_name))?
            .env(OWNER_ENV, "1")
            .env(OWNER_MODE, mode)
            .env(OWNER_JOURNAL, journal_path)
            .env(OWNER_RELEASE, release_path)
            .env(OWNER_SETTLED, settled_path)
            .env(OWNER_PARKED, parked_path)
            .spawn()?,
    )))
}

/// Wait until `marker` exists, bounded, so a child that never reaches it fails
/// the observation rather than hanging it.
fn await_marker(marker: &std::path::Path) -> TestResult {
    for _ in 0..2_000 {
        if marker.exists() {
            return Ok(());
        }
        pause(5);
    }
    Err(format!("the old worker never reached {}", marker.display()).into())
}

/// While the worker holds the fence a second writer is refused, and then the
/// worker is killed: the first half of the takeover, the fence handed back.
fn kill_the_fenced_worker(journal_path: &std::path::Path, probe: &mut ProbeGuard) -> TestResult {
    match FileJournal::open(journal_path) {
        Err(JournalError::Locked { .. }) => {}
        Err(other) => {
            lgwks_std::trace::debug!("kill_the_fenced_worker: returning an error to the caller");
            return Err(format!("expected a lock refusal against the child, got {other}").into());
        }
        Ok(_) => {
            lgwks_std::trace::debug!("kill_the_fenced_worker: returning an error to the caller");
            return Err("a second process opened a journal another process holds".into());
        }
    }
    let Some(mut held) = probe.take() else {
        lgwks_std::trace::debug!("kill_the_fenced_worker: returning an error to the caller");
        return Err("the old worker was gone before the takeover".into());
    };
    if let Err(error) = held.kill() {
        lgwks_std::trace::debug!(error = %error, "kill_the_fenced_worker: the worker had already gone");
    }
    let status = held.wait()?;
    assert!(
        !status.success(),
        "the old worker must have died from the kill, not exited on its own: {status}"
    );
    Ok(())
}

/// The takeover proper: the new owner adopts the generation the dead worker's
/// history was written at, claims the one after it, authorizes its own key and
/// is refused the stale one. Returns the current key and the two generations.
fn adopt_and_fence(
    owner: &FileJournal,
) -> Result<(EffectKey, EnvironmentEpoch, EnvironmentEpoch), Box<dyn std::error::Error>> {
    let stale = takeover_key("1", FIRST_GENERATION)?;
    let current = takeover_key("1", SECOND_GENERATION)?;
    assert_ne!(
        stale, current,
        "the same attempt at a new generation is a different identity"
    );
    // `adopt` is what makes the generation a fact on the disk rather than a
    // constant each process chooses for itself.
    let mut broker = Broker::new();
    let after = broker.adopt(environment()?, owner)?;
    let before = EnvironmentEpoch::from_decimal(FIRST_GENERATION)?;
    assert_eq!(
        after,
        EnvironmentEpoch::from_decimal(SECOND_GENERATION)?,
        "adopting a journal written entirely at generation 1 claims generation 2"
    );
    assert_ne!(
        before, after,
        "a takeover must move the generation, or nothing below is fenced"
    );
    broker.authorize(current)?;
    match broker.authorize(stale) {
        Err(BrokerError::Superseded {
            presented,
            current: held,
            ..
        }) => {
            assert_eq!(
                presented, before,
                "the refusal names the generation presented"
            );
            assert_eq!(held, after, "and the one the broker holds now");
        }
        Err(other) => {
            lgwks_std::trace::debug!("adopt_and_fence: returning an error to the caller");
            return Err(format!("a stale key must be Superseded, got {other}").into());
        }
        Ok(_) => {
            lgwks_std::trace::debug!("adopt_and_fence: returning an error to the caller");
            return Err("the broker authorized a generation it had replaced".into());
        }
    }
    Ok((current, before, after))
}

/// The history the current owner had acknowledged is extended at most by the
/// returning worker's two refusals, and its own attempt was never settled by
/// the returning worker's receipt.
fn assert_history_stands(
    observed: &[EffectEvent],
    acknowledged: &[EffectEvent],
    current: EffectKey,
) {
    assert!(
        observed.starts_with(acknowledged),
        "the returning worker changed the history the current owner had already \
         acknowledged: {observed:?} does not extend {acknowledged:?}"
    );
    assert!(
        observed.len() <= acknowledged.len().saturating_add(2),
        "the returning worker added {} events to a four-rung attempt that was \
         already complete: {observed:?}",
        observed.len().saturating_sub(acknowledged.len()),
    );
    assert!(
        !observed.contains(&EffectEvent::OutcomeObserved {
            key: current,
            evidence: EffectEvidence::Applied,
        }),
        "the current owner's own attempt was settled by the returning worker's receipt"
    );
}

// ── The row ─────────────────────────────────────────────────────────────────

/// An old worker returning after an owner-epoch takeover cannot authorize a
/// stale dispatch, and its receipt cannot overwrite the current owner.
///
/// Two real processes against one real file. The old worker opens the journal,
/// prepares one attempt under generation 1, reads its own receipt back by
/// position, and parks — alive, holding a warrant and a position. The parent
/// kills it, which is half of the takeover: the writer fence comes back from the
/// kernel. The parent reopens as the new owner, reads every event the dead
/// worker acknowledged, and *replaces* the environment, which is the other half:
/// the generation moves from 1 to 2 and every command prepared before it is now
/// stale. The old worker is then started a second time — released, not killed —
/// and asked to re-present its warrant and to settle the attempt it prepared. The
/// parent reads what it was allowed to do, and then reads the journal again.
///
/// The last assertion is the row. Whatever the released worker was allowed to
/// do, the current owner's committed history is exactly what it left, and the
/// released worker cannot commit anything the current owner does not see.
#[test]
fn an_old_worker_returning_after_a_takeover_cannot_settle_or_authorize_t16() -> TestResult {
    if std::env::var_os(OWNER_ENV).is_some() {
        return match std::env::var(OWNER_MODE) {
            Ok(mode) if mode == RETURNING => returning_worker_body(),
            Ok(_) | Err(std::env::VarError::NotPresent) => old_worker_body(),
            Err(error) => Err(error.into()),
        };
    }

    let dir = scratch_dir("owner-takeover")?;
    let _guard = TempGuard(dir.clone());
    let journal_path = dir.join("journal.log");
    let release_path = dir.join("release");
    let settled_path = dir.join("settled");
    let parked_path = dir.join("parked");

    let row = "an_old_worker_returning_after_a_takeover_cannot_settle_or_authorize_t16";
    let mut probe = spawn_old_worker(
        row,
        "probe",
        &journal_path,
        &release_path,
        &settled_path,
        &parked_path,
    )?;
    await_marker(&parked_path)?;

    // While the worker holds the fence, a second writer is refused — before it
    // reads a byte. This is asserted rather than waited for: the refusal is the
    // fence working and an acceptance is a fence that does not exist.
    kill_the_fenced_worker(&journal_path, &mut probe)?;

    let mut owner = loop_reopen(&journal_path)?;
    let inherited = owner.committed()?;
    assert_eq!(
        inherited.len(),
        2,
        "the new owner inherits exactly the dead worker's two acknowledged rungs"
    );
    assert!(
        inherited
            .iter()
            .any(|event| matches!(event, EffectEvent::DispatchPrepared { .. })),
        "and one of them is the prepared attempt whose receipt is now stale"
    );

    // The takeover, part two: the generation moves. This is what makes the dead
    // worker's warrant stale rather than merely abandoned.
    let (current, before, after) = adopt_and_fence(&owner)?;

    // The current owner writes its own attempt at the new generation, so the
    // journal holds work that a stale settlement could overwrite.
    owner.compare_and_append(owner.tail(), &EffectEvent::IntentAdmitted { key: current })?;
    let after_owner_write = owner.committed()?;

    // The fence is released, which is what the takeover means from the old
    // worker's point of view: it is no longer the owner, and it comes back.
    drop(owner);

    // The owner's bytes, read once its fence is gone. `File::try_lock` is `flock`
    // on Unix and `LockFileEx` on Windows, and a region `LockFileEx` holds refuses
    // a read through any other handle, this process's own included, with os error
    // 33. Nothing writes between the append and here, so these are its bytes.
    let bytes_after_owner_write = std::fs::read(&journal_path)?;

    // Release a *second* process — the returning worker, with its stale warrant
    // and its own open view of the file. It was never killed, so this is a real
    // return rather than a restart.
    std::fs::write(&release_path, b"go")?;
    let _returning = spawn_old_worker(
        row,
        RETURNING,
        &journal_path,
        &dir.join("release-2"),
        &dir.join("settled-2"),
        &dir.join("parked-2"),
    )?;
    await_marker(&dir.join("parked-2"))?;
    std::fs::write(dir.join("release-2"), b"go")?;
    let report = wait_for_report(&dir.join("settled-2"))?;
    // The returning worker is reaped by its guard's `Drop`, which is the backstop
    // the harness itself can forget.

    let dispatch = reported(&report, "dispatch=")?;
    assert!(
        dispatch.contains("replaced"),
        "the returning worker re-presented a warrant from generation {before} after \
         the broker moved to {after}: {report}"
    );
    assert!(
        reported(&report, "minted=")?.contains("replaced"),
        "and a fresh attempt under the old generation is refused the same way: {report}"
    );

    // The returning worker appended its settlement or was fenced out of it; both
    // are refusals of an effect, and the current owner is what decides either
    // way. Nothing it wrote may be invisible to the current owner, and nothing
    // it did may have changed what the current owner had committed.
    //
    // Both arms of the race are refusals, and which one this run took is
    // recorded rather than assumed: with the fence released, the returning
    // worker held a stale tail, so it either lost the compare-and-append
    // (`TailMismatch`, the fence held) or won it and installed a tail the
    // current owner then has to reconcile. Neither replays an effect, and
    // neither lets the returning worker's receipt stand as the current owner's.
    let reopened = loop_reopen(&journal_path)?;
    let observed = reopened.committed()?;
    // What the returning worker was allowed to do, kept next to the file it was
    // allowed to do it to, so a reader can see both halves without re-running.
    std::fs::write(
        dir.join("observed.txt"),
        format!(
            "takeover {before} -> {after}\n{report}observed {:?}\n",
            observed
        ),
    )?;
    assert_history_stands(&observed, &after_owner_write, current);
    assert!(
        lgwks_bot::journal::verify_chain(&reopened.committed_entries()?).is_ok(),
        "whatever the returning worker appended is committed into the one chain, not \
         beside it"
    );

    // The byte-level half of "does not overwrite". A writer that rewrote the
    // current owner's history would leave the owner's bytes as a *shorter*
    // prefix, not a longer one; an append-only fence can only extend them. Read
    // once the reopened journal's fence is gone, for the reason given above.
    drop(reopened);
    let observed_bytes = std::fs::read(&journal_path)?;
    assert!(
        observed_bytes.starts_with(&bytes_after_owner_write),
        "the journal's first {} bytes must be exactly what the current owner \
         acknowledged, so a returning worker can only append and never overwrite",
        bytes_after_owner_write.len()
    );
    Ok(())
}

/// Reopen the journal after the fence's holder is gone, waiting for the kernel
/// to hand it back.
///
/// Bounded rather than open-ended: a lock a dead descriptor pinned forever would
/// be a defect worth failing on, not worth hanging on.
fn loop_reopen(path: &std::path::Path) -> Result<FileJournal, Box<dyn std::error::Error>> {
    for _ in 0..200 {
        match FileJournal::open(path) {
            Ok(journal) => return Ok(journal),
            Err(JournalError::Locked { .. }) => pause(25),
            Err(other) => {
                let refusal = Err(format!("reopen after the kill failed: {other}").into());
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "loop_reopen: returning an error to the caller");
                return refusal;
            }
        }
    }
    Err("the writer fence never came back after its holder was killed".into())
}

/// Wait for the returning worker's report, bounded, and hand back what it wrote.
fn wait_for_report(path: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
    for _ in 0..2_000 {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.is_empty()
        {
            return Ok(text);
        }
        pause(5);
    }
    Err("the returning worker never reported what it was allowed to do".into())
}

mod in_process;
