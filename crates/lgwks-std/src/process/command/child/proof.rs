//! Kani proofs of the group-end rounds (#1164, ADR 0091, ADR 0092, ADR 0093):
//! for every group of up to [`MEMBERS`] members, every state, race, refusal
//! and fork, not only the seeds a sim draws.
//!
//! `cargo kani -p lgwks_std --no-default-features --features process
//! --harness group_end_`. Kani proves `end_group_on` itself at a bound of
//! [`ROUNDS`] rounds; the round logic is the same at the production bound, and
//! `sim_child_group_end_holds_every_round_bound` drives that one. The same
//! [`check`] also runs under `cargo test` over every case the finite domain
//! holds, so a mutation of the host or the oracle fails a test as well as the
//! proof.

use std::io;

use super::{GroupHost, end_group_on};

/// The members a group starts with.
const MEMBERS: usize = 2;

/// The group id, which is its leader's pid.
const PGID: i32 = 100;

/// The pid of each slot a group can fill, a fork included.
const PIDS: [i32; 3] = [PGID, 101, 102];

/// The kill rounds the proof allows.
const ROUNDS: usize = 3;

/// A member's state.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// Running.
    Live,
    /// Ended and not reaped.
    Zombie,
    /// Gone from the table.
    Gone,
}

/// One member slot.
#[derive(Clone, Copy)]
struct Slot {
    /// Its state.
    state: State,
    /// It ends on its own right after a listing.
    races: bool,
    /// Its kill is refused with `EPERM`.
    refuses: bool,
    /// Killing it starts one live replacement.
    forks: bool,
}

/// An empty slot.
const EMPTY: Slot = Slot {
    state: State::Gone,
    races: false,
    refuses: false,
    forks: false,
};

/// The member a fork starts: live, and it neither races, refuses nor forks.
const FORKED: Slot = Slot {
    state: State::Live,
    races: false,
    refuses: false,
    forks: false,
};

/// What the group kill does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kill {
    /// Every live member dies.
    Kills,
    /// `EPERM`, and no member is signalled.
    Shielded,
    /// `ESRCH`.
    Gone,
    /// `EACCES`.
    Refused,
}

/// Every group-kill answer.
const KILLS: [Kill; 4] = [Kill::Kills, Kill::Shielded, Kill::Gone, Kill::Refused];

/// The host Kani drives.
struct Host {
    /// The members, the first `used` of them real.
    slots: [Slot; 3],
    /// How many slots hold a member.
    used: usize,
    /// What the group kill does.
    kill: Kill,
    /// Whether a single kill reached a zombie or a pid outside the group.
    misfired: bool,
    /// How many listings were taken.
    listings: usize,
}

/// An OS error of `errno`.
fn os(errno: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(errno.raw_os_error())
}

impl GroupHost for Host {
    fn kill_group(&mut self, _pgid: i32) -> io::Result<()> {
        match self.kill {
            Kill::Kills => {
                for slot in &mut self.slots {
                    if slot.state == State::Live {
                        slot.state = State::Gone;
                    }
                }
                Ok(())
            }
            Kill::Shielded => Err(os(rustix::io::Errno::PERM)),
            Kill::Gone => {
                for slot in &mut self.slots {
                    slot.state = State::Gone;
                }
                Err(os(rustix::io::Errno::SRCH))
            }
            Kill::Refused => Err(os(rustix::io::Errno::ACCESS)),
        }
    }

    type Members = std::iter::Flatten<std::array::IntoIter<Option<(i32, bool)>, 3>>;

    fn members(&mut self, _pgid: i32) -> io::Result<Self::Members> {
        self.listings = self.listings.saturating_add(1);
        let mut listed = [None; 3];
        for ((slot, pid), entry) in self.slots.iter_mut().zip(PIDS).zip(&mut listed) {
            if slot.state != State::Gone {
                *entry = Some((pid, slot.state == State::Zombie));
            }
            if slot.races && slot.state == State::Live {
                slot.state = State::Gone;
            }
        }
        Ok(listed.into_iter().flatten())
    }

    fn kill_pid(&mut self, pid: i32) -> io::Result<()> {
        let used = self.used;
        let found = PIDS
            .iter()
            .position(|known| *known == pid)
            .filter(|index| *index < used);
        // The kernel's answer, as the model gives it: a stranger or a zombie
        // is a misfire, and only a live member's death can fork.
        let answer = match found.and_then(|index| self.slots.get_mut(index)) {
            None => Err(rustix::io::Errno::SRCH),
            Some(slot) => match slot.state {
                State::Gone => Err(rustix::io::Errno::SRCH),
                State::Zombie => Ok(None),
                State::Live if slot.refuses => Err(rustix::io::Errno::PERM),
                State::Live => {
                    slot.state = State::Gone;
                    Ok(Some(slot.forks))
                }
            },
        };
        self.misfired |= found.is_none() || matches!(answer, Ok(None));
        let forks = matches!(answer.map_err(os)?, Some(true));
        if forks && let Some(next) = self.slots.get_mut(used) {
            *next = FORKED;
            self.used = used.saturating_add(1);
        }
        Ok(())
    }
}

/// One group and one group-kill answer.
#[derive(Clone, Copy)]
struct Case {
    /// The first member, the leader.
    first: Slot,
    /// The second member, or [`EMPTY`] for a group of one.
    second: Slot,
    /// Whether the second slot holds a member.
    two: bool,
    /// What the group kill does.
    kill: Kill,
}

/// Run `end_group_on` on `case` and hold it to the oracle: no kill reaches a
/// zombie or a stranger; an end leaves no live member; only the shield lists
/// the group; a group kill that works or finds the group gone ends it; another
/// refusal is returned; and under the shield the group ends exactly when no
/// live member refuses its kill.
fn check(case: Case) {
    let Case {
        first,
        second,
        two,
        kill,
    } = case;
    let mut host = Host {
        slots: [first, second, EMPTY],
        used: if two { MEMBERS } else { 1 },
        kill,
        misfired: false,
        listings: 0,
    };
    let outcome = end_group_on(&mut host, PGID, ROUNDS);
    assert!(
        !host.misfired,
        "a kill reached a zombie or a pid outside the group"
    );
    if outcome.is_ok() {
        assert!(
            host.slots.iter().all(|slot| slot.state != State::Live),
            "an end left a live member"
        );
    }
    if kill != Kill::Shielded {
        assert!(host.listings == 0, "a group was listed without the shield");
    }
    let refuses = |slot: Slot| slot.state == State::Live && !slot.races && slot.refuses;
    match kill {
        Kill::Kills | Kill::Gone => assert!(outcome.is_ok(), "a killed or gone group was refused"),
        Kill::Refused => assert!(outcome.is_err(), "a refused group kill was called an end"),
        // A fork chain of one needs two killing rounds and one empty listing,
        // which [`ROUNDS`] holds.
        Kill::Shielded => assert!(
            outcome.is_ok() == !(refuses(first) || refuses(second)),
            "the shield's rounds ended a group they should not, or kept one they should end"
        ),
    }
}

/// Whether `case` is one the proof covers: at most one fork, so it has a slot
/// to land in, and an empty second slot in a group of one.
fn covered(case: Case) -> bool {
    !(case.first.forks && case.second.forks) && (case.two || case.second.state == State::Gone)
}

/// A member drawn by Kani: live or a zombie, with any race, refusal and fork.
#[cfg(kani)]
fn any_member() -> Slot {
    Slot {
        state: if kani::any::<bool>() {
            State::Live
        } else {
            State::Zombie
        },
        races: kani::any::<bool>(),
        refuses: kani::any::<bool>(),
        forks: kani::any::<bool>(),
    }
}

/// [`check`] for every case Kani can draw.
#[cfg(kani)]
#[kani::proof]
#[kani::unwind(4)]
fn group_end_kills_every_live_member_and_nothing_else() {
    let two = kani::any::<bool>();
    let case = Case {
        first: any_member(),
        second: if two { any_member() } else { EMPTY },
        two,
        kill: KILLS[usize::from(kani::any::<u8>() % 4)],
    };
    kani::assume(covered(case));
    check(case);
}

/// Each member the proof draws: live or a zombie, with any race, refusal and
/// fork.
#[cfg(test)]
fn every_member() -> impl Iterator<Item = Slot> {
    (0_u8..16).map(|bits| Slot {
        state: if bits & 1 == 0 {
            State::Live
        } else {
            State::Zombie
        },
        races: bits & 2 != 0,
        refuses: bits & 4 != 0,
        forks: bits & 8 != 0,
    })
}

/// [`check`] for every case the proof covers, enumerated: 832 cases.
#[cfg(test)]
#[test]
fn sim_group_end_proof_holds_for_every_case() {
    let mut cases = 0_u32;
    for kill in KILLS {
        for first in every_member() {
            let seconds =
                std::iter::once((false, EMPTY)).chain(every_member().map(|second| (true, second)));
            for (two, second) in seconds {
                let case = Case {
                    first,
                    second,
                    two,
                    kill,
                };
                if covered(case) {
                    check(case);
                    cases = cases.saturating_add(1);
                }
            }
        }
    }
    assert_eq!(cases, 832, "the enumeration covers the whole domain");
}
