//! Seeded check of the round bound `end_group_on` takes (#1164, ADR 0092): the
//! production bound and each small one the Kani proof (`group_end_*`) uses.
//!
//! Source: larry#131, the macOS `EPERM` shield the rounds answer. Requirement:
//! INV-STD-COMMAND-1, a group that ends within the bound ends, and a fork bomb
//! past it is refused by name, at every bound.

#[cfg(test)]
use super::tests::{PGID, forker};
use super::{GROUP_ROUNDS, end_group_on};

/// What every test here returns.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A shielded forker ends exactly when its kills and one empty listing fit the
/// bound; past it the refusal names the rounds, and no bound is overrun.
#[test]
fn sim_child_group_end_holds_every_round_bound() -> TestResult {
    for rounds in (1..=8).chain([GROUP_ROUNDS]) {
        for forks in 0..rounds.saturating_add(2) {
            let mut model = forker(forks)?;
            let outcome = end_group_on(&mut model, PGID, rounds);
            let ends = forks.saturating_add(2) <= rounds;
            assert_eq!(
                outcome.is_ok(),
                ends,
                "{forks} forks in {rounds}: {outcome:?}"
            );
            assert!(
                model.listings <= rounds,
                "{forks} forks: {} listings",
                model.listings
            );
            if let Err(error) = outcome {
                assert!(error.to_string().contains("kill round"), "{error}");
            }
        }
    }
    Ok(())
}
