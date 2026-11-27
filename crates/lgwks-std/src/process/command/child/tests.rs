//! Seeded simulation of `end_group_on`, the kill rounds behind every group end
//! on macOS (#1164, ADR 0092).
//!
//! Source: larry#131, where macOS `killpg` refused with `EPERM` for a group
//! holding a zombie in 7 of 24 parallel runs and signalled no member, and
//! #1160, where children outlived their owners. Requirement:
//! INV-STD-COMMAND-1, a group end leaves no live member, kills no pid outside
//! the group and no zombie, and names a group it could not end.
//!
//! The kernel's shield only shows under load, so a model stands in for it: a
//! group of live and zombie members, a kill of the group that the shield
//! refuses, members that die on their own between the listing and the kill, a
//! member whose kill is refused, and a forker that starts a new member each
//! time it is killed. The oracle is built from the draw, not from the code.

#[cfg(test)]
use std::io;

use super::{GROUP_ROUNDS, GroupHost, end_group_on};
use crate::seeded::Seeded;

/// What every test here returns.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Seeds the family draws, each one whole group.
const SEEDS: u64 = 4_096;

/// The most members one group starts with.
const MAX_MEMBERS: u64 = 6;

/// The group id every model uses.
pub(super) const PGID: i32 = 4_000;

/// A model member's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Running.
    Live,
    /// Ended and not reaped: listed, never killable.
    Zombie,
    /// Gone from the table.
    Gone,
}

/// One member of the model group.
#[derive(Clone, Debug)]
struct Member {
    /// Its pid.
    pid: i32,
    /// Its state.
    state: State,
    /// It ends on its own right after it is listed, so its kill finds nothing.
    races: bool,
    /// Its kill is refused with `EPERM`, as another user's process would be.
    refuses: bool,
    /// Killing it starts a replacement forker while this many remain.
    forks: u32,
}

/// What the model's kill of the whole group does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroupKill {
    /// Every live member is killed.
    Kills,
    /// `EPERM`, and no member is signalled: the zombie shield.
    Shielded,
    /// `ESRCH`: the group is already gone.
    Gone,
    /// `EACCES`: a refusal that is not the shield.
    Refused,
}

/// The model host.
pub(super) struct Model {
    /// Every member ever in the group.
    members: Vec<Member>,
    /// What a group kill does.
    group_kill: GroupKill,
    /// Each pid a single kill was sent to.
    killed: Vec<i32>,
    /// The next pid a fork takes.
    next_pid: i32,
    /// How many listings were taken.
    pub(super) listings: usize,
}

/// An OS error carrying `errno`.
fn os(errno: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(errno.raw_os_error())
}

impl GroupHost for Model {
    fn kill_group(&mut self, pgid: i32) -> io::Result<()> {
        assert_eq!(pgid, PGID, "the kill names the group");
        match self.group_kill {
            GroupKill::Kills => {
                for member in &mut self.members {
                    if member.state == State::Live {
                        member.state = State::Gone;
                    }
                }
                Ok(())
            }
            GroupKill::Shielded => Err(os(rustix::io::Errno::PERM)),
            GroupKill::Gone => {
                // A group the kernel no longer has holds no member at all.
                for member in &mut self.members {
                    member.state = State::Gone;
                }
                Err(os(rustix::io::Errno::SRCH))
            }
            GroupKill::Refused => Err(os(rustix::io::Errno::ACCESS)),
        }
    }

    type Members = Vec<(i32, bool)>;

    fn members(&mut self, pgid: i32) -> io::Result<Self::Members> {
        assert_eq!(pgid, PGID, "the listing names the group");
        self.listings = self.listings.saturating_add(1);
        let listed = self
            .members
            .iter()
            .filter(|member| member.state != State::Gone)
            .map(|member| (member.pid, member.state == State::Zombie))
            .collect();
        for member in &mut self.members {
            if member.races && member.state == State::Live {
                member.state = State::Gone;
            }
        }
        Ok(listed)
    }

    fn kill_pid(&mut self, pid: i32) -> io::Result<()> {
        self.killed.push(pid);
        let member = self
            .members
            .iter_mut()
            .find(|member| member.pid == pid)
            .ok_or_else(|| io::Error::other(format!("pid {pid} is a stranger")))?;
        // The kernel's answer, as the model gives it: a zombie takes the
        // signal and stays listed, and only a live member's death can fork.
        let forks = match member.state {
            State::Gone => Err(os(rustix::io::Errno::SRCH)),
            State::Zombie => Ok(0),
            State::Live if member.refuses => Err(os(rustix::io::Errno::PERM)),
            State::Live => {
                member.state = State::Gone;
                Ok(member.forks)
            }
        }?;
        if forks > 0 {
            let forks = forks.saturating_sub(1);
            let pid = self.next_pid;
            self.next_pid = self.next_pid.saturating_add(1);
            self.members.push(Member {
                pid,
                state: State::Live,
                races: false,
                refuses: false,
                forks,
            });
        }
        Ok(())
    }
}

/// Whether a draw below `bound` lands on 0: a one-in-`bound` choice.
fn one_in(rng: &mut Seeded, bound: u64) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(rng.below(bound)? == 0)
}

/// One drawn group and the outcome the oracle expects: `Some(true)` an end
/// that leaves no live member, `Some(false)` a refusal, `None` either.
fn draw(seed: u64) -> Result<(Model, bool), Box<dyn std::error::Error>> {
    let mut rng = Seeded::from_seed(seed);
    let group_kill = match rng.below(8)? {
        0 => GroupKill::Kills,
        1 => GroupKill::Gone,
        2 => GroupKill::Refused,
        _ => GroupKill::Shielded,
    };
    let mut members = vec![Member {
        pid: PGID,
        state: State::Zombie,
        races: false,
        refuses: false,
        forks: 0,
    }];
    let count = rng.below(MAX_MEMBERS)?;
    let mut refused = false;
    let mut forks_needed = 0_u32;
    for index in 0..count {
        let state = if one_in(&mut rng, 4)? {
            State::Zombie
        } else {
            State::Live
        };
        let refuses = state == State::Live && one_in(&mut rng, 16)?;
        let races = state == State::Live && !refuses && one_in(&mut rng, 4)?;
        let forks = if state == State::Live && !refuses && !races && one_in(&mut rng, 3)? {
            u32::try_from(rng.below(80)?)?
        } else {
            0
        };
        refused |= refuses;
        forks_needed = forks_needed.max(forks);
        members.push(Member {
            pid: PGID.saturating_add(1).saturating_add(i32::try_from(index)?),
            state,
            races,
            refuses,
            forks,
        });
    }
    // A forker of `f` needs `f + 1` killing rounds and one more that finds the
    // group empty.
    let rounds = usize::try_from(forks_needed)?.saturating_add(2);
    let ends = match group_kill {
        GroupKill::Kills | GroupKill::Gone => true,
        GroupKill::Refused => false,
        GroupKill::Shielded => !refused && rounds <= GROUP_ROUNDS,
    };
    let model = Model {
        members,
        group_kill,
        killed: Vec::new(),
        next_pid: PGID.saturating_add(1_000),
        listings: 0,
    };
    Ok((model, ends))
}

/// Every drawn group ends exactly when the oracle says; an end leaves no live
/// member; no kill reaches a stranger or a zombie; a refusal that is not the
/// shield lists nothing; and the family reaches every outcome.
#[test]
fn sim_child_group_end_matches_the_model() -> TestResult {
    let (mut ended, mut refused, mut by_pid) = (0_u32, 0_u32, 0_u32);
    for seed in 0..SEEDS {
        let (mut model, ends) = draw(seed)?;
        let outcome = end_group_on(&mut model, PGID, GROUP_ROUNDS);
        assert_eq!(
            outcome.is_ok(),
            ends,
            "seed {seed}: {outcome:?} {:?}",
            model.members
        );
        let live: Vec<i32> = model
            .members
            .iter()
            .filter(|member| member.state == State::Live)
            .map(|member| member.pid)
            .collect();
        if ends {
            assert!(live.is_empty(), "seed {seed}: {live:?} still live");
            ended = ended.saturating_add(1);
        } else {
            refused = refused.saturating_add(1);
        }
        for pid in &model.killed {
            let member = model.members.iter().find(|member| member.pid == *pid);
            assert!(member.is_some(), "seed {seed}: pid {pid} is a stranger");
            assert!(
                member.is_some_and(|member| member.state != State::Zombie),
                "seed {seed}: zombie {pid} was sent a kill"
            );
        }
        if model.group_kill != GroupKill::Shielded {
            assert_eq!(
                model.listings, 0,
                "seed {seed}: only the shield lists the group"
            );
            assert!(
                model.killed.is_empty(),
                "seed {seed}: only the shield kills by pid"
            );
        } else if !model.killed.is_empty() {
            by_pid = by_pid.saturating_add(1);
        }
    }
    assert!(
        ended > 0 && refused > 0 && by_pid > 0,
        "ended {ended}, refused {refused}, by pid {by_pid}"
    );
    Ok(())
}

/// A shielded group of one live forker that starts `forks` replacements; the
/// round-bound sims (`sim_rounds`) end it under each bound.
pub(super) fn forker(forks: usize) -> Result<Model, std::num::TryFromIntError> {
    Ok(Model {
        members: vec![Member {
            pid: PGID,
            state: State::Live,
            races: false,
            refuses: false,
            forks: u32::try_from(forks)?,
        }],
        group_kill: GroupKill::Shielded,
        killed: Vec::new(),
        next_pid: PGID.saturating_add(1),
        listings: 0,
    })
}
