//! Seeded sims of the process census against a model process tree that acts as
//! the kernel does for a subreaper (ADR 0082, #1128 G2b).
//!
//! The oracle is the drawn tree, never the census's answer: every process alive
//! at the start is named once and gone at the end. Fools and the bug each is
//! for: the pulled plug, a parent killed under its children, which must be
//! adopted and swept in a later round; the saboteur, a SIGKILL the host refuses,
//! which must fail the test by name rather than read as clean; the monkey, a
//! fork bomb that adopts a fresh child at every listing, which must end at the
//! round cap; and a model that refuses to block, which catches a reap that
//! waits on a child nobody killed.

use crate::process::orphans::{Host, MAX_ROUNDS, Orphans, sweep};
use crate::seeded::Seeded;
use std::collections::{BTreeMap, BTreeSet};
use std::io;

/// What a sim reports when a precondition did not hold.
type SimResult = Result<(), Box<dyn std::error::Error>>;

/// Seeds the family runs.
const SEEDS: u64 = 4_096;

/// The wrapper's own pid in the model: the subreaper every orphan goes to.
const WRAPPER: i32 = 1;

/// One process of the model.
#[derive(Debug, Clone, Copy)]
struct Proc {
    /// Its parent's pid.
    parent: i32,
    /// Alive, or ended and not yet reaped.
    alive: bool,
}

/// A process tree under the wrapper.
#[derive(Debug)]
struct World {
    /// Every process not yet reaped, by pid.
    procs: BTreeMap<i32, Proc>,
    /// A pid whose SIGKILL the host refuses.
    refuses: Option<i32>,
    /// Whether every listing finds a fresh adoptee.
    bomb: bool,
    /// The next pid a fork takes.
    next: i32,
}

/// A drawn number below `bound`, as an I/O error when the stream refuses.
fn below(draw: &mut Seeded, bound: u64) -> io::Result<u64> {
    draw.below(bound).map_err(io::Error::other)
}

/// A drawn element of `items`, as an I/O error when the stream refuses.
fn pick(draw: &mut Seeded, items: &[i32]) -> io::Result<i32> {
    let at = draw.index(items.len()).map_err(io::Error::other)?;
    items
        .get(at)
        .copied()
        .ok_or_else(|| io::Error::other("a drawn index is past the list"))
}

impl World {
    /// A drawn tree of up to 12 processes, some ended, one maybe unkillable.
    fn draw(seed: u64) -> io::Result<Self> {
        let mut draw = Seeded::from_seed(seed);
        let mut procs = BTreeMap::new();
        let mut pids = vec![WRAPPER];
        for number in 0..below(&mut draw, 12)? {
            let pid = 100_i32.saturating_add(i32::try_from(number).map_err(io::Error::other)?);
            let chosen = pick(&mut draw, &pids)?;
            // A process whose parent ended was adopted when the parent did.
            let parent = match procs.get(&chosen) {
                Some(&Proc { alive: false, .. }) => WRAPPER,
                _ => chosen,
            };
            let alive = below(&mut draw, 4)? != 0;
            let _replaced = procs.insert(pid, Proc { parent, alive });
            pids.push(pid);
        }
        let live: Vec<i32> = alive_set(&procs).into_iter().collect();
        let refuses = if below(&mut draw, 8)? == 0 && !live.is_empty() {
            Some(pick(&mut draw, &live)?)
        } else {
            None
        };
        Ok(Self {
            procs,
            refuses,
            bomb: below(&mut draw, 16)? == 0,
            next: 1_000,
        })
    }
}

/// The pids alive in `procs`.
fn alive_set(procs: &BTreeMap<i32, Proc>) -> BTreeSet<i32> {
    procs
        .iter()
        .filter(|&(_, proc)| proc.alive)
        .map(|(pid, _)| *pid)
        .collect()
}

impl Host for World {
    fn children(&mut self) -> io::Result<Vec<i32>> {
        if self.bomb {
            let pid = self.next;
            self.next = self.next.saturating_add(1);
            let fresh = Proc {
                parent: WRAPPER,
                alive: true,
            };
            let _replaced = self.procs.insert(pid, fresh);
        }
        Ok(self
            .procs
            .iter()
            .filter(|&(_, proc)| proc.parent == WRAPPER)
            .map(|(pid, _)| *pid)
            .collect())
    }

    fn live_name(&mut self, pid: i32) -> Option<String> {
        self.procs
            .get(&pid)
            .filter(|proc| proc.alive)
            .map(|_| format!("p{pid}"))
    }

    fn kill(&mut self, pid: i32) -> io::Result<()> {
        if self.refuses == Some(pid) {
            #[cfg(feature = "trace")]
            crate::trace::debug!(pid, "the model refuses this kill, as its saboteur");
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        if let Some(proc) = self.procs.get_mut(&pid) {
            proc.alive = false;
        }
        // The dead parent's children, live or ended, go to the subreaper.
        for proc in self.procs.values_mut() {
            if proc.parent == pid {
                proc.parent = WRAPPER;
            }
        }
        Ok(())
    }

    fn reap_any(&mut self) -> io::Result<Option<i32>> {
        let ended = self
            .procs
            .iter()
            .find(|&(_, proc)| proc.parent == WRAPPER && !proc.alive)
            .map(|(pid, _)| *pid);
        if let Some(pid) = ended {
            let _reaped = self.procs.remove(&pid);
            return Ok(Some(pid));
        }
        if self.procs.values().any(|proc| proc.parent == WRAPPER) {
            #[cfg(feature = "trace")]
            crate::trace::debug!("the model would block on a live child nobody killed");
            return Err(io::Error::other(
                "the model would block: a live child nobody killed",
            ));
        }
        Ok(None)
    }
}

/// One seed's sweep, checked against what the drawn tree owed.
fn judge(seed: u64) -> io::Result<Orphans> {
    let mut world = World::draw(seed)?;
    let owed = alive_set(&world.procs);
    let (refuses, bomb) = (world.refuses, world.bomb);
    let found = sweep(&mut world);
    let named: Vec<i32> = found
        .survivors()
        .iter()
        .map(|survivor| survivor.pid())
        .collect();
    let distinct: BTreeSet<i32> = named.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        named.len(),
        "seed {seed}: named twice: {named:?}"
    );
    if let Some(pid) = refuses {
        let why = found
            .unstopped()
            .ok_or_else(|| io::Error::other("a refused kill read as clean"))?;
        assert!(why.contains(&pid.to_string()), "seed {seed}: {why}");
    } else if bomb {
        let why = found
            .unstopped()
            .ok_or_else(|| io::Error::other("a fork bomb read as stopped"))?;
        assert!(why.contains(&MAX_ROUNDS.to_string()), "seed {seed}: {why}");
    } else {
        assert_eq!(found.unstopped(), None, "seed {seed}");
        assert_eq!(
            distinct, owed,
            "seed {seed}: named against alive at the start"
        );
        assert!(
            world.procs.is_empty(),
            "seed {seed}: left {:?}",
            world.procs
        );
        assert_eq!(found.is_clean(), owed.is_empty(), "seed {seed}");
    }
    Ok(found)
}

/// Catches a census that misses an adoptee, names one twice, reads a refused
/// kill or a fork bomb as clean, or waits on a child it never killed.
#[test]
fn sim_orphan_census_matches_the_model() -> SimResult {
    let mut swept = 0_u32;
    for seed in 0..SEEDS {
        let found = judge(seed)?;
        assert_eq!(found, judge(seed)?, "seed {seed}: one seed swept two ways");
        swept = swept.saturating_add(u32::from(!found.is_clean()));
    }
    assert!(swept > 0, "no seed left a survivor");
    Ok(())
}

/// The verdict a user reads counts the survivors and names each one.
#[test]
fn a_census_verdict_counts_and_names_each_survivor() {
    let mut world = World {
        procs: BTreeMap::from([
            (
                7,
                Proc {
                    parent: WRAPPER,
                    alive: true,
                },
            ),
            (
                8,
                Proc {
                    parent: 7,
                    alive: true,
                },
            ),
        ]),
        refuses: None,
        bomb: false,
        next: 1_000,
    };
    let verdict = sweep(&mut world).to_string();
    assert!(
        verdict.contains("the test left 2 live processes"),
        "{verdict}"
    );
    assert!(
        verdict.contains("7 (p7)") && verdict.contains("8 (p8)"),
        "{verdict}"
    );
}

/// The wrong-order fool for `reap_until`: children end in a drawn order, the
/// one waited for among them or not at all. The oracle is the draw: its own
/// status comes back after exactly the ends before it were reaped, and a pid
/// that never ends is `NotFound` once no child is left. Catches a wait that
/// returns the first child to end, or stops before the one it waits for.
#[test]
fn sim_a_wait_returns_its_own_childs_status_and_reaps_the_rest() -> SimResult {
    use crate::process::orphans::wait_for;
    for seed in 0..SEEDS {
        let mut draw = Seeded::from_seed(seed);
        let count = below(&mut draw, 8)?;
        let ends: Vec<i32> = (0..count)
            .map(|number| i32::try_from(number).map(|n| n.saturating_mul(3).saturating_add(2)))
            .collect::<Result<_, _>>()?;
        let present = below(&mut draw, 4)? > 0 && !ends.is_empty();
        let target = if present { pick(&mut draw, &ends)? } else { 1 };
        let mut reaped = 0_usize;
        let waited = wait_for(target, || {
            let next = ends.get(reaped).map(|&pid| (pid, pid.saturating_mul(10)));
            reaped = reaped.saturating_add(usize::from(next.is_some()));
            Ok(next)
        });
        match ends.iter().position(|&pid| pid == target) {
            Some(at) => {
                assert_eq!(
                    waited?,
                    target.saturating_mul(10),
                    "seed {seed}: another child's status"
                );
                assert_eq!(
                    reaped,
                    at.saturating_add(1),
                    "seed {seed}: reaped past its child"
                );
            }
            None => {
                let refused = waited.err().map(|error| error.kind());
                assert_eq!(refused, Some(io::ErrorKind::NotFound), "seed {seed}");
                assert_eq!(
                    reaped,
                    ends.len(),
                    "seed {seed}: stopped before the last end"
                );
            }
        }
    }
    Ok(())
}

/// A census names a process only while it runs user code: a zombie, an exiting
/// process and one with `SIGKILL` pending are already ending. Catches a census
/// that fails a test for a child it killed itself just before it exited.
#[cfg(unix)]
#[test]
fn a_process_with_a_pending_kill_or_none_left_to_run_is_not_named() {
    use crate::process::orphans::name_if_live;
    let running = "42 (sleep x) S 1 42 42 0 -1 4194304 0";
    assert_eq!(name_if_live(running, false).as_deref(), Some("sleep x"));
    assert_eq!(
        name_if_live(running, true),
        None,
        "a pending SIGKILL is an end"
    );
    assert_eq!(
        name_if_live("42 (sh) Z 1 42 42 0 -1 4194304 0", false),
        None,
        "a zombie"
    );
    // PF_EXITING (0x4) in the flags field: part way through its exit.
    assert_eq!(
        name_if_live("42 (sh) R 1 42 42 0 -1 4 0", false),
        None,
        "an exiting process"
    );
}

/// A real `/proc/<pid>/status` holds the pending masks past a dozen other
/// lines; a reader that stopped at the first other line would miss the kill.
#[cfg(unix)]
#[test]
fn a_pending_kill_is_read_past_the_status_lines_before_it() {
    use crate::process::capture::table::status_has_kill_pending;
    let status = "Name:\tsleep\nState:\tS (sleeping)\nSigPnd:\t0000000000000000\nShdPnd:\t0000000000000100\n";
    assert!(
        status_has_kill_pending(status),
        "SIGKILL is bit 8 of ShdPnd"
    );
    assert!(!status_has_kill_pending(
        "Name:\tsleep\nShdPnd:\t0000000000000000\n"
    ));
}
