//! T25: a native bot and a materialized spec leave one trace.

use lgwks_bot::domain::eval::{Above, Changed};
use lgwks_bot::spec::Bot;
use lgwks_bot::{Cap, GrantSet};

use super::{
    Counter, Page, TestResult, both_runs, both_runs_with, materialize, native_one,
    native_two_chains, native_two_entries, scope, tick_and_trace,
};

#[test]
fn one_chain_traces_match_t25() -> TestResult<()> {
    let native = native_one(3, |_: &u16| true, Page::new("a"))?;
    let (native_run, made_run) = both_runs(
        native,
        r#"{"version":1,"name":"made","chains":[
            {"source":"test::counter","target":"3",
             "on":[["always",{"domain":"test::page","target":"a"}]]}]}"#,
    )?;
    assert_eq!(
        native_run, made_run,
        "a materialized bot must poll and execute in the same order as a native one"
    );
    assert_eq!(made_run.0, 1, "the one chain fires once");
    Ok(())
}

#[test]
fn two_entries_in_one_chain_trace_in_declaration_order() -> TestResult<()> {
    let native = native_two_entries(Page::new("a"), Page::new("b"))?;
    let (native_run, made_run) = both_runs(
        native,
        r#"{"version":1,"name":"made","chains":[
            {"source":"test::counter","target":"3","on":[
                ["always",{"domain":"test::page","target":"a"}],
                ["always",{"domain":"test::page","target":"b"}]]}]}"#,
    )?;
    assert_eq!(
        native_run, made_run,
        "entry order must survive materialization"
    );
    assert_eq!(made_run.0, 2, "both entries fire");
    Ok(())
}

#[test]
fn two_chains_poll_in_declaration_order() -> TestResult<()> {
    let native = native_two_chains(Page::new("a"), Page::new("b"))?;
    let (native_run, made_run) = both_runs(
        native,
        r#"{"version":1,"name":"made","chains":[
            {"source":"test::counter","target":"3",
             "on":[["always",{"domain":"test::page","target":"a"}]]},
            {"source":"test::counter","target":"4",
             "on":[["always",{"domain":"test::page","target":"b"}]]}]}"#,
    )?;
    assert_eq!(
        native_run, made_run,
        "two chains must poll and fire in the same order on both paths"
    );
    assert_eq!(made_run.0, 2, "each chain fires its one entry");
    Ok(())
}

#[test]
fn changed_condition_materializes_identically() -> TestResult<()> {
    let json = r#"{"version":1,"name":"made","chains":[
        {"source":"test::counter","target":"3",
         "on":[["changed",{"domain":"test::page","target":"a"}]]}]}"#;

    let mut native = native_one(3, Changed::<u16>::new(), Page::new("a"))?;
    let native_first = tick_and_trace(&mut native)?;
    let native_second = tick_and_trace(&mut native)?;
    let mut made = materialize(json, &GrantSet::empty())?;
    let made_first = tick_and_trace(&mut made)?;
    let made_second = tick_and_trace(&mut made)?;

    assert_eq!(
        native_first, made_first,
        "the first tick of a `changed` chain must match"
    );
    assert_eq!(
        native_second, made_second,
        "a `changed` condition must hold its state the same way on both paths"
    );
    assert_eq!(native_first.0, 1, "the first tick fires once");
    assert_eq!(
        native_second.0, 0,
        "a source that holds still fires nothing on the second tick"
    );
    Ok(())
}

#[test]
fn threshold_condition_materializes_identically() -> TestResult<()> {
    let native = native_one(7, Above::<u16>::new(5), Page::new("a"))?;
    let (native_run, made_run) = both_runs(
        native,
        r#"{"version":1,"name":"made","chains":[
            {"source":"test::counter","target":"7",
             "on":[["threshold::above(5)",{"domain":"test::page","target":"a"}]]}]}"#,
    )?;
    assert_eq!(
        native_run, made_run,
        "a parameterized threshold must materialize to the same predicate"
    );
    assert_eq!(made_run.0, 1, "7 is above 5, so the entry fires");
    Ok(())
}

#[test]
fn capability_requiring_chain_materializes_under_the_same_grants() -> TestResult<()> {
    let grants = GrantSet::empty().grant(Cap::net());
    let native = Bot::builder("native")
        .observe(Counter::with_caps(3, vec![Cap::net()]))
        .on(|_: &u16| true, Page::new("a"))
        .with_effects(scope()?)
        .build(&grants)?;
    let (native_run, made_run) = both_runs_with(
        native,
        r#"{"version":1,"name":"made","chains":[
            {"source":"test::net_counter","target":"3",
             "on":[["always",{"domain":"test::page","target":"a"}]]}]}"#,
        &grants,
    )?;
    assert_eq!(
        native_run, made_run,
        "authority comes from the grant set on both paths, so the traces agree"
    );
    Ok(())
}
