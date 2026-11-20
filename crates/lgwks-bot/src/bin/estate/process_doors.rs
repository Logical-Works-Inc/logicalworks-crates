//! The supervisor's process doors, proved on the shipped artifact rather than
//! trusted from the tests: record-then-release gating, the release handshake,
//! and every deny-builder refusal arm.
//!
//! A gated lane that ran before its identity was recorded, an empty denial
//! the builders accepted, or a profile a control byte slipped into would fail
//! here rather than in a coordinator. Every line below is asserted, so a
//! door that stops working fails the smoke.

#![cfg(unix)]

use std::time::Duration;

use lgwks_bot::rt::process::{
    CapturedStream, Confinement, DenyBeneath, ProcessRun, ProcessSpec, SandboxProfile,
};
use lgwks_bot::rt::supervise::{GatedIdentifiedSpawn, Supervisor, TaskOutcome};
use lgwks_bot::rt::time::{Instant, sleep};

use super::smoke::{make_dir, remove_dir};
use super::{record, say, unique_hex};

/// The `run_process` result door, read as a downstream caller reads it.
async fn read_process_result(supervisor: &mut Supervisor) -> Option<String> {
    let run: ProcessRun = match supervisor
        .run_process(&ProcessSpec::new("/usr/bin/true"))
        .await
    {
        Ok(run) => run,
        Err(err) => return Some(format!("run_process refused a plain program: {err}")),
    };
    let stdout: &CapturedStream = run.stdout();
    let frames = stdout.frames(64);
    lgwks_std::trace::debug!(frames = ?frames, "estate smoke: the stable frames door answered");
    // One check per fact, so a door that breaks names itself and no fact can
    // hide behind another in a chain.
    let checks = [
        (run.exit_code() == Some(0), "it exits zero"),
        (run.signal().is_none(), "no signal ends it"),
        (!run.deadline_fired(), "its deadline does not fire"),
        (stdout.total_bytes() == 0, "it writes no bytes"),
        (stdout.lossy_text().is_empty(), "it renders no text"),
        (stdout.retained_capacity() == 0, "it retains no buffer"),
    ];
    checks
        .iter()
        .find(|&&(held, _)| !held)
        .map(|&(_, fact)| format!("a plain program must exit clean, but not: {fact}"))
}

/// The deny builders refuse what they cannot confine, before any fork.
///
/// A deny variant that did not build is recorded as the mismatch, and the
/// later doors still run, so one smoke reports every door that broke.
fn deny_builders(mismatch: &mut Option<String>) {
    if DenyBeneath::new().validate().is_ok() {
        record(
            mismatch,
            "an empty denial list must not validate".to_owned(),
        );
    }
    let mut many = DenyBeneath::new();
    for _ in 0..=DenyBeneath::MAX_PATHS {
        many.deny("/srv/denied");
    }
    if many.validate().is_ok() {
        record(
            mismatch,
            format!(
                "a denial past {} paths must not validate",
                DenyBeneath::MAX_PATHS
            ),
        );
    }
    let mut overlong = DenyBeneath::new();
    overlong.deny(format!("/{}", "x".repeat(DenyBeneath::MAX_PATH_BYTES)));
    if overlong.validate().is_ok() {
        record(
            mismatch,
            "a denial past the path bound must not validate".to_owned(),
        );
    }
    let mut one = DenyBeneath::new();
    one.deny("/srv/denied");
    if one.validate().is_err() {
        record(mismatch, "one absolute denial must validate".to_owned());
    }
    match Confinement::deny_beneath(["/srv/denied"]) {
        Confinement::DenyBeneath(denied) => {
            if denied.validate().is_err() {
                record(mismatch, "the built denial must validate".to_owned());
            }
        }
        _ => record(
            mismatch,
            "deny_beneath must build the deny variant".to_owned(),
        ),
    }
}

/// The profile builders refuse what could not confine: empty sources and
/// control bytes, eagerly at construction and again at validation, and a
/// denial names the resolved directory, quoted for the profile.
///
/// The smoke mints a fresh directory with the estate's own entropy source
/// rather than naming a fixed path.
fn profile_builders(mismatch: &mut Option<String>) -> std::io::Result<()> {
    if SandboxProfile::try_new(String::new()).is_ok() {
        record(mismatch, "an empty profile must not validate".to_owned());
    }
    if SandboxProfile::try_new("a\x01b").is_ok() {
        record(
            mismatch,
            "a profile with a control byte must not validate".to_owned(),
        );
    }
    if let Err(error) = SandboxProfile::try_new("(version 1)(allow default)") {
        record(mismatch, format!("a plain profile must validate: {error}"));
    }
    let mut profile = SandboxProfile::allow_default();
    if profile.validate().is_err() {
        record(mismatch, "the default profile must validate".to_owned());
    }
    let deny_dir = std::env::temp_dir().join(format!("lgwks-estate-smoke-deny-{}", unique_hex()?));
    let denied = deny_dir.join("denied");
    make_dir(&denied)
        .map_err(|error| std::io::Error::other(format!("the smoke denied dir failed: {error}")))?;
    let denied_read = profile.deny_read_beneath(&denied);
    remove_dir(&deny_dir);
    if let Err(error) = denied_read {
        record(
            mismatch,
            format!("a real directory must be deniable: {error}"),
        );
    } else if !profile.source().contains("(deny file-read*") {
        record(
            mismatch,
            format!(
                "the denial is missing from the profile: {:?}",
                profile.source()
            ),
        );
    }
    Ok(())
}

/// A gated lane holds its payload until the identity is recorded: the
/// release below stands in for the durable write, and the assertions
/// around it are the record-then-release order the coordinator keeps.
async fn gated_lane(mismatch: &mut Option<String>) -> std::io::Result<()> {
    let mut supervisor = Supervisor::new(1);
    let mut spec = ProcessSpec::new("/usr/bin/true");
    if let Ok((reader, _writer)) = std::io::pipe() {
        spec.inherit_fd(std::os::fd::OwnedFd::from(reader));
        if spec.inherited_fd_count() != 1 {
            record(
                mismatch,
                "one inherited descriptor must count as one".to_owned(),
            );
        }
        if spec.inherited_raw_fds().len() != 1 {
            record(
                mismatch,
                "one inherited descriptor must list as one raw fd".to_owned(),
            );
        }
    }
    let mut spawned: GatedIdentifiedSpawn =
        match supervisor.spawn_process_identified_gated(&spec).await {
            Ok(spawned) => spawned,
            Err(error) => {
                record(mismatch, format!("the gated spawn was refused: {error}"));
                return Ok(());
            }
        };
    if spawned.is_released() {
        record(mismatch, "a new gate must read as unreleased".to_owned());
    }
    if !spawned.leader().pid().is_positive() {
        record(
            mismatch,
            "the identity must be readable while the payload waits".to_owned(),
        );
    }
    if let Err(error) = spawned.release() {
        record(
            mismatch,
            format!("the gate must release once the record is durable: {error}"),
        );
        return Ok(());
    }
    if !spawned.is_released() {
        record(mismatch, "a released gate must read as released".to_owned());
    }
    if let Err(error) = spawned.release() {
        record(
            mismatch,
            format!("a second release must be a no-op, not {error}"),
        );
    }
    let task = spawned.task();
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or_else(|| std::io::Error::other("the gate drain deadline overflowed the clock"))?;
    let outcome = loop {
        supervisor.reap();
        if let Some(outcome) = supervisor.next_report() {
            break outcome;
        }
        if deadline.checked_duration_since(Instant::now()).is_none() {
            record(mismatch, "the released payload never reported".to_owned());
            return Ok(());
        }
        sleep(Duration::from_millis(1)).await;
    };
    if !matches!(outcome, TaskOutcome::Completed { .. }) {
        record(
            mismatch,
            format!("the released payload must complete, got {outcome:?}"),
        );
    }
    if outcome.task() != task {
        record(
            mismatch,
            format!("the released payload must report under its own task, got {outcome:?}"),
        );
    }
    if let Some(problem) = read_process_result(&mut supervisor).await {
        record(mismatch, problem);
    }
    say(&format!(
        "process doors: gated lane completed under task {}",
        task.get()
    ))
    .map_err(|error| std::io::Error::other(format!("the process doors line failed: {error}")))?;
    Ok(())
}

/// Every process door in order: the deny builders, the profile builders, then the gated lane.
pub(super) async fn run(mismatch: &mut Option<String>) -> std::io::Result<()> {
    deny_builders(mismatch);
    profile_builders(mismatch)?;
    gated_lane(mismatch).await
}

#[cfg(test)]
mod tests {
    use lgwks_bot::rt::estate::Estate;

    /// Every process door answers as the smoke asserts, run under this module so
    /// a change to a door is judged by its own test rather than the whole crate.
    #[test]
    fn process_doors_answer_as_the_smoke_asserts() -> Result<(), Box<dyn std::error::Error>> {
        let estate = Estate::supervised("acme", 1)?;
        let driver = estate
            .handle()
            .ok_or("a supervised estate carries the process driver")?;
        let mut mismatch = None;
        driver.block_on(super::run(&mut mismatch))?;
        assert_eq!(mismatch, None, "every process door answers as asserted");
        Ok(())
    }
}
