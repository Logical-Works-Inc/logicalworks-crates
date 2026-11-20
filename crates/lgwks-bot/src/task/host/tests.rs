//! The plan arms and the derived definition identity, checked from inside the
//! host module.

#[cfg(test)]
use super::Plan;
use crate::effect::RunId;
use crate::task::{Host, name::TaskName};

/// What this module's tests report.
///
/// A `Result` rather than a panic because the workspace forbids the
/// panicking path everywhere, test code included: a unit test that unwound
/// would report a line number inside a macro rather than which of the facts
/// it was checking had moved.
type TestResult = Result<(), String>;

/// A run identity built from a literal, not from the entropy source.
///
/// [`RunId::mint`] is behind the `ephemeral` feature and `ephemeral` is not a
/// default feature, so a unit test that minted one compiled under
/// `--all-features` and failed the workspace-default build — which is the
/// build `cargo nextest list --workspace` performs, and therefore a test that
/// could not be run at all on the configuration CI measures coverage over. What
/// these tests need is *a* run id and *two* distinct ones; none of them
/// asserts anything about how a run id is minted, which is
/// `effect::RunId::mint`'s own contract and is exercised where that feature is
/// on. Parsing a fixed literal is therefore the honest fixture: it needs no
/// feature and no randomness, so the same two identities are the same two
/// identities on every run of the suite.
fn run_id(nibble: char) -> Result<RunId, String> {
    // A 32-character lowercase hex literal, which is the one spelling
    // `RunId::from_hex` accepts, and one distinct nibble per call so two
    // draws cannot collide and quietly make a "distinct identities" assertion
    // pass on a single value.
    let literal = format!("{nibble}{}", "0".repeat(31));
    RunId::from_hex(&literal).map_err(|error| error.to_string())
}

/// Every plan arm reports the run it adopts, and only the resume arms report one.
///
/// The property the drift check's placement rests on: a fresh plan has no run
/// to be incompatible with, and both resume arms have one whether or not they
/// declared an identity.
#[test]
fn only_the_resume_arms_adopt_a_run() -> TestResult {
    let host = host()?;
    let task = TaskName::new("probe").map_err(|error| error.to_string())?;
    let run = run_id('1')?;
    let identity = host.definition_for(task.as_str(), None);
    assert!(Plan::Fresh.run().is_none(), "a fresh plan adopts nothing");
    assert!(
        Plan::Declared(identity.clone()).run().is_none(),
        "a declared fresh plan adopts nothing either"
    );
    assert_eq!(
        Plan::Resume(run).run(),
        Some(run),
        "a resume adopts its run"
    );
    assert_eq!(
        Plan::ResumeUnder(run, identity).run(),
        Some(run),
        "a resume under a declared identity adopts the same run"
    );
    Ok(())
}

/// The derived identity is stable for one run and different for another.
///
/// What makes it usable as a default at all: a derived identity that moved
/// between two attempts at the same run would make every existing durable
/// step refuse its own resume, and one that was the same for every run would
/// make two runs claim one definition.
#[test]
fn a_derived_identity_is_stable_per_run_and_distinct_across_runs() -> TestResult {
    let host = host()?;
    let first = run_id('1')?;
    let second = run_id('2')?;
    assert_eq!(
        host.definition_for("probe", Some(&first)).digest(),
        host.definition_for("probe", Some(&first)).digest(),
        "one run's derived identity must not move between two readings"
    );
    assert_ne!(
        host.definition_for("probe", Some(&first)).digest(),
        host.definition_for("probe", Some(&second)).digest(),
        "two runs must not share a derived identity"
    );
    assert_ne!(
        host.definition_for("probe", None).digest(),
        host.definition_for("probe", Some(&first)).digest(),
        "a run that has not adopted an id is not the run that has"
    );
    Ok(())
}

/// A host this module's tests can always build.
fn host() -> Result<Host, String> {
    Host::builder("acme")
        .map_err(|error| error.to_string())?
        .build()
        .map_err(|error| error.to_string())
}
