//! Independent materializations running at once.

#![cfg(feature = "rt")]

use lgwks_bot::GrantSet;
use lgwks_bot::spec::{Bot, BotSpec};
use lgwks_std::task::try_spawn_blocking;

use super::{SPEC_DOMAINS, TestResult, scope_for_run};

/// How many materializations run at once: well under the blocking pool's queue
/// bound of 16,384, so every one is admitted and none is shed.
const JOBS: u32 = 1000;

/// The spec every concurrent materialization builds.
const CONCURRENT_SPEC: &str = r#"{"version":1,"name":"concurrent","chains":[
    {"source":"test::counter","target":"3",
     "on":[["always",{"domain":"test::page","target":"a"}]]}]}"#;

#[test]
fn a_thousand_materializations_across_tenants_are_independent() -> TestResult<()> {
    let runtime = lgwks_bot::Runtime::new()?;

    // Each job owns its run identity and its scope, and builds, ticks and drops
    // its bot entirely on its own thread. Nothing is shared, so nothing can
    // collide; the assertion is that every one of a thousand independent runs
    // reaches the same answer.
    let jobs = (0..JOBS)
        .map(|index| {
            try_spawn_blocking(move || -> Result<usize, String> {
                let spec =
                    BotSpec::from_json(CONCURRENT_SPEC).map_err(|error| error.to_string())?;
                // One-based: a zero run identifier is refused, and every job
                // must still get its own distinct identity.
                let run_hex = format!("{:032x}", index.saturating_add(1));
                let effects = scope_for_run(&run_hex).map_err(|error| error.to_string())?;
                let mut bot = Bot::from_spec(&spec, &SPEC_DOMAINS, &GrantSet::empty(), effects)
                    .map_err(|error| error.to_string())?;
                assert_eq!(bot.name(), "concurrent", "the name must not be shared");
                assert_eq!(
                    bot.source_domains().len(),
                    1,
                    "each bot keeps its own single chain"
                );
                bot.tick().map_err(|error| error.to_string())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let fired = runtime.block_on(async {
        let mut counts = Vec::with_capacity(jobs.len());
        for job in jobs {
            counts.push(job.await);
        }
        counts
    });

    assert_eq!(fired.len(), 1000, "every job must be collected");
    for (index, outcome) in fired.iter().enumerate() {
        assert_eq!(
            outcome.as_ref().ok(),
            Some(&1_usize),
            "materialization {index} was not independent of the others: {outcome:?}"
        );
    }
    Ok(())
}
