//! Regressions for process ownership across leader exit and task cancellation.

#![cfg(all(
    unix,
    feature = "rt",
    feature = "time",
    feature = "sync",
    feature = "process"
))]

use crate::scratch::Scratch;

use std::path::Path;
use std::time::Duration;

use lgwks_bot::Runtime;
use lgwks_bot::rt::process::ProcessSpec;
use lgwks_bot::rt::supervise::{Supervisor, SupervisorCancelled, TaskOutcome};
use lgwks_bot::rt::time::{Instant, sleep};

const BUDGET: Duration = Duration::from_secs(10);

fn is_alive(pid: &str) -> bool {
    let mut alive = false;
    if let Ok(status) = std::process::Command::new("kill")
        .arg("-0")
        .arg(pid)
        .status()
    {
        alive = status.success();
    }
    alive
}

async fn pid(path: &Path) -> Option<String> {
    let deadline = Instant::now().checked_add(BUDGET)?;
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(String::from(text));
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        sleep(Duration::from_millis(5)).await;
    }
}

async fn outcome(supervisor: &mut Supervisor) -> Result<TaskOutcome, std::io::Error> {
    let deadline = Instant::now()
        .checked_add(BUDGET)
        .ok_or_else(|| std::io::Error::other("clock deadline overflowed"))?;
    loop {
        supervisor.reap();
        if let Some(outcome) = supervisor.next_report() {
            return Ok(outcome);
        }
        if Instant::now() >= deadline {
            let refusal = Err(std::io::Error::other("process outcome did not arrive"));
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "outcome: returning an error to the caller");
            return refusal;
        }
        sleep(Duration::from_millis(5)).await;
    }
}

async fn gone(pid: &str) -> bool {
    let deadline = Instant::now().checked_add(BUDGET);
    while deadline.is_some_and(|value| Instant::now() < value) {
        if !is_alive(pid) {
            return true;
        }
        sleep(Duration::from_millis(5)).await;
    }
    !is_alive(pid)
}

#[test]
fn cancel_before_spawn_process_refuses_without_starting_the_command()
-> Result<(), Box<dyn std::error::Error>> {
    // The fence sits between the permit and the fork. A supervisor that
    // checked cancellation only after admission would return an error *and*
    // have started the command — the marker below would exist even though the
    // call failed. The refusal must come first: no process, no first
    // instruction, and the error names the cancellation rather than looking
    // like a platform failure.
    let dir = Scratch::new("fenced")?;
    let marker = dir.path().join("marker");
    let script = format!("echo started > {}", marker.display());
    let runtime = Runtime::new()?;
    runtime.block_on(async {
        let mut supervisor = Supervisor::default();
        supervisor.cancel();
        let mut spec = ProcessSpec::new("sh");
        spec.arg("-c").arg(&script);
        let refused = supervisor.spawn_process(&spec).await;
        assert!(
            refused.is_err_and(|error| {
                error
                    .get_ref()
                    .is_some_and(|payload| payload.is::<SupervisorCancelled>())
            }),
            "the refusal must be the typed cancellation, not a platform failure"
        );
        assert!(
            !marker.exists(),
            "a refused command must not have run its first instruction"
        );
    });
    Ok(())
}

/// Start a shell on `supervisor` that forks one `sleep 60` descendant, records
/// its pid in `child_file`, and then runs `tail`.
///
/// The descendant is what every tree-cleanup test here is about: it is the
/// process a cleanup that only reaped the shell would leave running.
async fn spawn_with_descendant(
    supervisor: &mut Supervisor,
    child_file: &Path,
    tail: &str,
) -> std::io::Result<()> {
    let script = format!("sleep 60 & echo $! > {}; {tail}", child_file.display());
    let mut spec = ProcessSpec::new("sh");
    spec.arg("-c").arg(&script);
    supervisor.spawn_process(&spec).await?;
    Ok(())
}

/// [`spawn_with_descendant`], then wait for the descendant's recorded pid.
async fn spawned_descendant(
    supervisor: &mut Supervisor,
    child_file: &Path,
    tail: &str,
) -> std::io::Result<String> {
    spawn_with_descendant(supervisor, child_file, tail).await?;
    pid(child_file)
        .await
        .ok_or_else(|| std::io::Error::other("descendant pid was not recorded"))
}

#[test]
fn zero_exit_does_not_fabricate_tree_cleanup_t19() -> Result<(), Box<dyn std::error::Error>> {
    let dir = Scratch::new("zero")?;
    let child_file = dir.path().join("child.pid");
    let runtime = Runtime::new()?;
    let (outcome, child) = runtime.block_on(async {
        let mut supervisor = Supervisor::default();
        let child = spawned_descendant(&mut supervisor, &child_file, "exit 0").await?;
        let outcome = outcome(&mut supervisor).await?;
        Ok::<_, std::io::Error>((outcome, child))
    })?;
    assert!(outcome.cleanup().is_some());
    assert!(runtime.block_on(gone(&child)));
    Ok(())
}

#[test]
fn nonzero_exit_does_not_fabricate_tree_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    let dir = Scratch::new("nonzero")?;
    let child_file = dir.path().join("child.pid");
    let runtime = Runtime::new()?;
    let (outcome, child) = runtime.block_on(async {
        let mut supervisor = Supervisor::default();
        spawn_with_descendant(&mut supervisor, &child_file, "exit 7").await?;
        // Block this executor thread after native spawn without yielding, so
        // the manager future cannot receive its first poll before shutdown is
        // requested. The shell still runs: it forks the descendant and records
        // the pid. Wait for that record with a deadline rather than a fixed
        // pause, because a one-shot read after 25ms loses the race on a loaded
        // machine and reports a missing descendant that is merely late.
        let deadline = std::time::Instant::now() + BUDGET;
        let child = loop {
            if let Ok(text) = std::fs::read_to_string(&child_file) {
                let text = text.trim();
                if !text.is_empty() {
                    break String::from(text);
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(std::io::Error::other("descendant pid was not recorded"));
            }
            std::hint::spin_loop();
        };
        let outcome = outcome(&mut supervisor).await?;
        Ok::<_, std::io::Error>((outcome, child))
    })?;
    assert!(outcome.is_process_failure());
    assert!(outcome.cleanup().is_some());
    assert!(runtime.block_on(gone(&child)));
    Ok(())
}

#[test]
fn cancellation_before_manager_task_poll_keeps_descendant_owned_t20()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = Scratch::new("pre-poll")?;
    let child_file = dir.path().join("child.pid");
    let runtime = Runtime::new()?;
    let child = runtime.block_on(async {
        let mut supervisor = Supervisor::default();
        let child = spawned_descendant(&mut supervisor, &child_file, "wait").await?;
        let _report = supervisor.shutdown().await;
        Ok::<_, std::io::Error>(child)
    })?;
    assert!(runtime.block_on(gone(&child)));
    Ok(())
}

/// Test that inherit_fd passes a pipe end into a child and sibling cannot access that fd.
#[test]
fn inherited_fd_passes_pipe_and_isolates_sibling() -> Result<(), Box<dyn std::error::Error>> {
    let (pipe_reader, pipe_writer) = std::io::pipe()?;
    let writer_fd = std::os::fd::OwnedFd::from(pipe_writer);
    let writer_raw = std::os::fd::AsRawFd::as_raw_fd(&writer_fd);

    let runtime = Runtime::new()?;
    runtime.block_on(async {
        let mut supervisor = Supervisor::default();

        // Child writes "inherited-passed\n" directly to inherited fd.
        let child_script = format!("echo inherited-passed >&{writer_raw}");
        let mut child_spec = ProcessSpec::new("sh");
        child_spec.arg("-c").arg(&child_script);
        child_spec.inherit_fd(writer_fd);

        let _task_id = supervisor.spawn_process(&child_spec).await?;
        // Drop child_spec in parent so child holds the only remaining write end of the pipe.
        drop(child_spec);

        let mut read_file = std::fs::File::from(pipe_reader);
        use std::io::Read as _;
        let mut text_buffer = String::new();
        read_file.read_to_string(&mut text_buffer)?;

        if text_buffer.trim() != "inherited-passed" {
            return Err(
                std::io::Error::other("unexpected buffer content from inherited fd").into(),
            );
        }

        // Sibling spawned without inherit_fd must not be able to write to that fd number (EBADF).
        let sibling_script = format!("echo sibling-bad >&{writer_raw} || exit 42");
        let mut sibling_spec = ProcessSpec::new("sh");
        sibling_spec.arg("-c").arg(&sibling_script);

        let _sibling_task = supervisor.spawn_process(&sibling_spec).await?;
        let sibling_outcome = outcome(&mut supervisor).await?;
        if !sibling_outcome.is_process_failure() {
            return Err(std::io::Error::other("sibling without inherited_fd should fail").into());
        }

        Ok(())
    })
}

/// Test that ProcessSpec::configure validates MAX_INHERITED_FDS and duplicate fds.
#[test]
fn inherited_fd_validation_refuses_invalid_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Runtime::new()?;
    runtime.block_on(async {
        let mut supervisor = Supervisor::default();

        // 1. Test exceeding MAX_INHERITED_FDS (17 fds).
        let mut excess_spec = ProcessSpec::new("sh");
        excess_spec.arg("-c").arg("exit 0");
        for _round in 0..17 {
            let (_reader, writer) = std::io::pipe()?;
            excess_spec.inherit_fd(std::os::fd::OwnedFd::from(writer));
        }

        match supervisor.spawn_process(&excess_spec).await {
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {}
            Ok(_) => {
                return Err(std::io::Error::other(
                    "expected error for exceeding MAX_INHERITED_FDS",
                )
                .into());
            }
            Err(error) => {
                return Err(
                    std::io::Error::other(format!("unexpected error kind: {error}")).into(),
                );
            }
        }

        // 2. Test duplicate file descriptor.
        let (_dup_reader, dup_writer) = std::io::pipe()?;
        let mut dup_spec = ProcessSpec::new("sh");
        dup_spec.arg("-c").arg("exit 0");
        dup_spec.inherit_fd(std::os::fd::OwnedFd::from(dup_writer));
        dup_spec.duplicate_first_inherited_fd_for_test();

        match supervisor.spawn_process(&dup_spec).await {
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {}
            Ok(_) => {
                return Err(
                    std::io::Error::other("expected error for duplicate inherited fd").into(),
                );
            }
            Err(error) => {
                return Err(
                    std::io::Error::other(format!("unexpected error kind: {error}")).into(),
                );
            }
        }

        Ok(())
    })
}

fn gate_test_fixture(
    label: &str,
) -> Result<(Scratch, std::path::PathBuf, String), Box<dyn std::error::Error>> {
    let scratch = Scratch::new(label)?;
    let marker = scratch.path().join("payload-ran.marker");
    let script = format!("echo executed > {}; exit 0", marker.display());
    Ok((scratch, marker, script))
}

async fn spawn_gated_sh(
    supervisor: &mut Supervisor,
    script: &str,
) -> Result<GatedIdentifiedSpawn, Box<dyn std::error::Error>> {
    let mut spec = ProcessSpec::new("sh");
    spec.arg("-c").arg(script);
    let gated = supervisor.spawn_process_identified_gated(&spec).await?;
    Ok(gated)
}

/// Test that dropping GatedIdentifiedSpawn without release closes gate pipe and child exits 99.
#[test]
fn gated_identified_spawn_drop_closes_gate_with_exit_99() -> Result<(), Box<dyn std::error::Error>>
{
    let (_scratch_dir, marker_path, script) = gate_test_fixture("gate-drop-test")?;

    let runtime = Runtime::new()?;
    runtime.block_on(async {
        let mut supervisor = Supervisor::default();
        let gated_spawn = spawn_gated_sh(&mut supervisor, &script).await?;
        let _leader_identity = gated_spawn.leader().clone();

        // Drop the gated spawn without releasing it!
        drop(gated_spawn);

        // Wait for the child task outcome.
        let task_outcome = outcome(&mut supervisor).await?;
        if !task_outcome.is_process_failure() {
            return Err(
                std::io::Error::other("expected process failure on unreleased gate drop").into(),
            );
        }

        if let Some(status) = task_outcome.exit_status() {
            if status.code() != Some(99) {
                return Err(std::io::Error::other(format!(
                    "expected exit code 99, got {status:?}"
                ))
                .into());
            }
        }

        // Payload marker file must NOT exist.
        if marker_path.exists() {
            return Err(
                std::io::Error::other("payload must not have executed when gate dropped").into(),
            );
        }

        Ok(())
    })
}

/// Test that calling release() on GatedIdentifiedSpawn successfully executes the payload.
#[test]
fn gated_identified_spawn_release_executes_payload() -> Result<(), Box<dyn std::error::Error>> {
    let (_scratch_dir, marker_path, script) = gate_test_fixture("gate-release-test")?;

    let runtime = Runtime::new()?;
    runtime.block_on(async {
        let mut supervisor = Supervisor::default();
        let mut gated_spawn = spawn_gated_sh(&mut supervisor, &script).await?;
        gated_spawn.release().await?;

        let task_outcome = outcome(&mut supervisor).await?;
        if task_outcome.is_process_failure() {
            return Err(std::io::Error::other("expected process success on released gate").into());
        }

        if !marker_path.exists() {
            return Err(
                std::io::Error::other("payload marker file should exist after release").into(),
            );
        }

        Ok(())
    })
}
