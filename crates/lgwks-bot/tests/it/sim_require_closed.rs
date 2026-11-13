//! Simulation family: `Scope::require` fails closed (#909 step 1).
//!
//! R9 (`docs/requirements/d-correctness.md`): no effect fires without a grant
//! minted by the one issuer, carried on the call and checked on the call. The
//! defect this pins: `require` read the run's authority from a task-local. A
//! step polled on a spawned task, or under no host at all, read "none", and
//! "none" passed as "covered". MITRE names that pattern fail-open (CWE-636).
//!
//! Real `Host`, real `Task`, real `join_all_bounded`. The seed draws each
//! tenant's grant, the requirement (with a fat-fingered repeat) and where the
//! step runs. The oracle is set difference over the declared vocabulary, never
//! the code's own answer.
//!
//! | Family | Property it pins |
//! |---|---|
//! | `a_spawned_step_under_an_empty_grant_is_blocked` | the issue's falsifier: a spawned step under `GrantSet::empty()` is `Blocked` |
//! | `a_scope_no_host_admitted_covers_nothing` | a hand-built root scope is refused every capability it asks for |
//! | `every_placement_agrees_with_the_set_model` | seeded: body, spawned and unhosted steps, two tenants, each shortfall is exactly the model's, and the trace replays |
//! | `a_wide_fan_out_keeps_the_hosts_authority` | 100, 1,000 and 10,000 spawned steps: each one is checked, granted or not |

#![cfg(all(feature = "script", feature = "rt", feature = "sync"))]

use std::error::Error;

use lgwks_bot::Runtime;
use lgwks_bot::cap::{Cap, Deficit};
use lgwks_bot::gate::GrantSet;
use lgwks_bot::rt::task::join_all_bounded;
use lgwks_bot::script::{FlowError, Scope, Tenant};
use lgwks_bot::task::{Disposition, Host, Report, task};

use crate::sim::{self, Band, Rng};

/// What every fallible test here returns.
type TestResult = Result<(), Box<dyn Error>>;

/// Two tenants: the smallest number at which "isolated" is a claim.
const TENANTS: [&str; 2] = ["acme", "globex"];

/// How many spawned steps run at once, the fan-out's own bound.
const FAN_LIMIT: usize = 16;

/// The fan-out widths the scale family drives.
const TIERS: [usize; 3] = [100, 1_000, 10_000];

/// Where a step's `require` runs.
#[derive(Clone, Copy, Debug)]
enum Place {
    /// On the run's own task, inside the host's body.
    Body,
    /// On tasks `join_all_bounded` spawns from the body, `width` of them.
    Spawned(usize),
    /// On a root scope no host admitted.
    Unhosted,
}

/// The capabilities the seed draws from: the shipped four and one custom name.
fn vocabulary() -> Vec<Cap> {
    vec![
        Cap::net(),
        Cap::fs(),
        Cap::sys(),
        Cap::notify(),
        Cap::new("vendor.payments.charge"),
    ]
}

/// The members of `vocabulary()` whose bit is set in `mask`, in order.
fn subset(mask: u32) -> Vec<Cap> {
    (0_u32..)
        .zip(vocabulary())
        .filter(|&(bit, _)| (mask >> bit) & 1 == 1)
        .map(|(_, cap)| cap)
        .collect()
}

/// The grant set holding exactly `caps`.
fn grant_of(caps: &[Cap]) -> GrantSet {
    caps.iter()
        .cloned()
        .fold(GrantSet::empty(), GrantSet::grant)
}

/// The model: each capability in `required` that `granted` lacks, once, in
/// the order first asked. `None` for `granted` is a step no host admitted.
fn expected(required: &[Cap], granted: Option<&[Cap]>) -> Vec<String> {
    let mut short: Vec<String> = Vec::new();
    for cap in required {
        let covered = match granted {
            Some(held) => held.contains(cap),
            None => false,
        };
        if !covered && !short.iter().any(|name| name == cap.as_str()) {
            short.push(cap.as_str().to_owned());
        }
    }
    short
}

/// The names a deficit carries, in order.
fn names_of(deficit: &Deficit) -> Vec<String> {
    deficit
        .shortages()
        .map(|shortage| shortage.required().as_str().to_owned())
        .collect()
}

/// The shortfall a `require` result reports: empty for `Ok`.
fn shortfall_of(result: &Result<(), FlowError>) -> Result<Vec<String>, String> {
    match *result {
        Ok(()) => Ok(Vec::new()),
        Err(FlowError::Blocked { ref deficit, .. }) => Ok(names_of(deficit)),
        Err(ref other) => Err(format!("require failed for another reason: {other:?}")),
    }
}

/// The shortfall a host's report carries: empty for `Succeeded`.
fn shortfall_of_report(report: &Report<()>) -> Result<Vec<String>, String> {
    match (report.disposition(), report.needs()) {
        (Disposition::Succeeded, None) => Ok(Vec::new()),
        (Disposition::Blocked, Some(deficit)) => Ok(names_of(deficit)),
        (disposition, _) => Err(format!(
            "the run ended {disposition:?}, neither succeeded nor blocked: {:?}",
            report.error()
        )),
    }
}

/// Run one `require` of `needs` at `place` for `tenant` under `granted`, and
/// return the shortfall it reported.
fn reach(
    runtime: &Runtime,
    tenant: &str,
    granted: &[Cap],
    needs: Vec<Cap>,
    place: Place,
) -> Result<Vec<String>, Box<dyn Error>> {
    let width = match place {
        Place::Unhosted => {
            let scope = Scope::root(Tenant::new(tenant)?);
            return Ok(shortfall_of(&scope.require(&needs))?);
        }
        Place::Body => None,
        Place::Spawned(width) => Some(width),
    };
    let host = Host::builder(tenant)?.grants(grant_of(granted)).build()?;
    let probe = task(
        "probe",
        |scope: Scope, (needs, width): (Vec<Cap>, Option<usize>)| async move {
            let Some(width) = width else {
                return scope.enter("reach")?.require(&needs);
            };
            let steps = (0..width).map(|index| {
                let scope = scope.clone();
                let needs = needs.clone();
                async move { scope.item("spawned", index)?.require(&needs) }
            });
            join_all_bounded(FAN_LIMIT, steps)
                .await
                .into_iter()
                .collect::<Result<Vec<()>, FlowError>>()
                .map(drop)
        },
    )?;
    let report = runtime.block_on(host.run(&probe, (needs, width)));
    Ok(shortfall_of_report(&report)?)
}

/// One draw of the requirement: a subset of the vocabulary, its first member
/// sometimes repeated, the way a careless declaration lists a cap twice.
fn draw_needs(rng: &mut Rng) -> Vec<Cap> {
    let mut needs = subset(rng.below(32));
    if rng.below(2) == 1
        && let Some(first) = needs.first().cloned()
    {
        needs.push(first);
    }
    needs
}

/// One draw of where the step runs.
fn draw_place(rng: &mut Rng) -> Result<Place, Box<dyn Error>> {
    Ok(match rng.below(3) {
        0 => Place::Body,
        1 => Place::Spawned(usize::try_from(rng.between(1, 48))?),
        _ => Place::Unhosted,
    })
}

#[test]
fn a_spawned_step_under_an_empty_grant_is_blocked() -> TestResult {
    let runtime = Runtime::new()?;
    let short = reach(
        &runtime,
        TENANTS[0],
        &[],
        vec![Cap::net()],
        Place::Spawned(4),
    )?;
    assert_eq!(
        short,
        vec!["bot.net"],
        "a step on a spawned task under a host that granted nothing is Blocked on bot.net"
    );
    Ok(())
}

#[test]
fn a_scope_no_host_admitted_covers_nothing() -> TestResult {
    let runtime = Runtime::new()?;
    let every = vocabulary();
    let short = reach(&runtime, TENANTS[0], &every, every.clone(), Place::Unhosted)?;
    assert_eq!(
        short,
        expected(&every, None),
        "no host means no authority, whatever grant the caller had in mind"
    );
    let none = reach(&runtime, TENANTS[0], &[], Vec::new(), Place::Unhosted)?;
    assert!(none.is_empty(), "an empty requirement asks for nothing");
    Ok(())
}

#[test]
fn every_placement_agrees_with_the_set_model() -> TestResult {
    let runtime = Runtime::new()?;
    sim::assert_replays(Band::new(0, 48), |sim| {
        let rng = sim.rng();
        let mut facts: Vec<String> = Vec::new();
        for tenant in TENANTS {
            let granted = subset(rng.below(32));
            let needs = draw_needs(rng);
            let place = draw_place(rng)?;
            let hosted = match place {
                Place::Unhosted => None,
                Place::Body | Place::Spawned(_) => Some(granted.as_slice()),
            };
            let want = expected(&needs, hosted);
            let got = reach(&runtime, tenant, &granted, needs.clone(), place)?;
            assert_eq!(
                got, want,
                "{tenant} {place:?}: granted {granted:?}, needed {needs:?}"
            );
            facts.push(format!("{tenant} {place:?} short={got:?}"));
        }
        for fact in facts {
            sim.record(&fact);
        }
        Ok(())
    })
}

#[test]
fn a_wide_fan_out_keeps_the_hosts_authority() -> TestResult {
    let runtime = Runtime::new()?;
    let every = vocabulary();
    for width in TIERS {
        let granted = reach(
            &runtime,
            TENANTS[0],
            &every,
            every.clone(),
            Place::Spawned(width),
        )?;
        assert!(
            granted.is_empty(),
            "width {width}: every spawned step holds the host's full grant"
        );
        let refused = reach(
            &runtime,
            TENANTS[1],
            &[],
            every.clone(),
            Place::Spawned(width),
        )?;
        assert_eq!(
            refused,
            expected(&every, Some(&[])),
            "width {width}: under an empty grant no spawned step slips through"
        );
    }
    Ok(())
}
