//! The orphan reap: a recorded leader is signalled only while it is still the
//! process the record names, and every descendant below it stops.

use super::*;

/// The process census's sims (#1128 G2b).
#[path = "orphans_sim.rs"]
mod orphans_sim;

/// How many table reads [`all_stop_running`] makes before it gives up.
#[cfg(unix)]
const SETTLE_READS: usize = 100_000;

/// Whether every pid in `pids` stops running within [`SETTLE_READS`] reads.
///
/// The tree's pipe ends once the last process has closed its descriptors,
/// which is a few instructions before the table marks it a zombie, so this
/// reads the table until it agrees, yielding the processor between reads and
/// counting reads, not time. Running, not present: a killed process whose
/// parent has not reaped it yet is a zombie, which has stopped and is not a
/// survivor.
#[cfg(unix)]
fn all_stop_running(pids: &[i32]) -> Result<bool, Box<dyn std::error::Error>> {
    for _ in 0..SETTLE_READS {
        if running_processes(pids)?.is_empty() {
            return Ok(true);
        }
        std::thread::yield_now();
    }
    Ok(false)
}

#[test]
#[cfg(unix)]
fn a_matching_leader_has_its_group_and_every_descendant_stopped()
-> Result<(), Box<dyn std::error::Error>> {
    let mut tree = Tree::grow(3)?;
    let leader = identify_process(tree.root)?.ok_or("the live leader has no identity")?;
    let OrphanReap::Signalled { descendants } = reap_orphaned_group(&leader)? else {
        return Err("a leader that still holds its pid must be signalled".into());
    };
    let mut expected = tree.descendants.clone();
    expected.sort_unstable();
    assert_eq!(
        descendants.pids(),
        expected.as_slice(),
        "the reap captures exactly the tree below the leader"
    );
    let status = tree.leader.wait()?;
    assert!(!status.success(), "the leader ends by the reap's signal");
    tree.wait_for_exit()?;
    assert!(
        all_stop_running(&expected)?,
        "every descendant of the reaped group stops running: {expected:?}"
    );
    Ok(())
}

#[test]
#[cfg(unix)]
fn a_forged_start_on_a_live_pid_is_never_signalled() -> Result<(), Box<dyn std::error::Error>> {
    let mut tree = Tree::grow(1)?;
    let real = identify_process(tree.root)?.ok_or("the live leader has no identity")?;
    let forged = ProcessIdentity::new(real.pid(), &format!("{}0", real.started()))?;
    assert_eq!(
        reap_orphaned_group(&forged)?,
        OrphanReap::LeaderReused { holder: real },
        "a pid whose start differs from the record is another process"
    );
    assert!(
        tree.leader.try_wait()?.is_none(),
        "the process holding the number was not signalled"
    );
    assert_eq!(
        running_processes(&tree.descendants)?
            .into_iter()
            .collect::<Vec<i32>>(),
        tree.descendants,
        "nor was anything below it"
    );
    Ok(())
}

#[test]
#[cfg(unix)]
fn a_leader_that_is_gone_is_left_alone() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = std::process::Command::new("true")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let pid = i32::try_from(child.id())?;
    let recorded = identify_process(pid)?.ok_or("the child has no identity")?;
    child.wait()?;
    let reaped = reap_orphaned_group(&recorded)?;
    assert!(
        !matches!(reaped, OrphanReap::Signalled { .. }),
        "a reaped leader's record signals nothing, got {reaped:?}"
    );
    Ok(())
}
