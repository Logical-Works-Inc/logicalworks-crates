//! lgwks_bot against baselines and competitor engines.
//!
//! # What this measures, and what it refuses to
//!
//! Every timing this rig reports is gated on a **fairness check**: the bot and
//! the baseline must produce the identical sequence of effect counts *and*
//! perform the identical number of condition evaluations, tick for tick, over
//! the whole run. If either differs, the two engines are not doing the same
//! work, the ratio is meaningless, and the rig fails loudly rather than
//! printing a favourable number.
//!
//! The evaluation counter is not belt-and-braces. An earlier revision of this
//! file gated on effects alone, and the fan-out scenario reported a 2937x ratio
//! because the baseline returned `entries` as an integer instead of looping
//! over them: the same effect count, none of the work. Gating on work as well
//! as on results is what makes the comparison mean something.
//!
//! # The inputs are deterministic by construction
//!
//! Each source's value is `tick / period`, a pure function of the tick index.
//! There is no RNG in the workload, so the expected effect count is not merely
//! reproducible, it is *computable in closed form* and asserted. A benchmark
//! whose workload is random can only be checked for self-consistency; this one
//! can be checked against arithmetic.
//!
//! # What is deliberately excluded
//!
//! Raw-`bevy_ecs` and CEL/zen-engine comparators are not here. A raw-bevy
//! comparator would be *this rig's* reimplementation of the bot's schedule, so
//! it would measure the author's fluency with bevy rather than the bot's cost.
//! The honest comparators are the null hypothesis (a hand-rolled loop doing
//! exactly the same work, with the work counted) and the cost decomposition
//! (`poll-only-*` scenarios), and both are present. See `README.md`.
//!
//! # Layout
//!
//! This file is the measurement and the report. The workload both engines run,
//! with the builders and tick loops every measurement shares, is `workload`;
//! the one error every fallible step returns is `error`.

mod error;
mod fit;
mod probes;
mod profile;
mod scenarios;
mod stats;
mod workload;

use std::cell::Cell;
use std::fmt::Write as _;
use std::rc::Rc;
use std::time::{Duration, Instant};

use lgwks_bot::Bot;
use lgwks_bot::journal::MAX_JOURNAL_EVENTS;
use lgwks_std::trace::info;

use error::BenchError;
use scenarios::{SCENARIOS, Scenario};
use workload::{Clock, build_bot, build_handrolled, run_base_ticks, run_bot_ticks};

// ── The allocation counter ───────────────────────────────────────────────────
//
// Timing alone says a tick is slow; it does not say what the tick is spending
// its time on, and "reduce allocations" is then a guess. This counts them, so
// the hot path's allocation model is a measured number rather than an inference
// from reading `Box::new` in a source file.
//
// Two counters, because they answer different questions: `ALLOCS` is the count
// the tick pays, and `BYTES` says whether those allocations are small handovers
// or large buffers. A tick that is allocation-bound shows a count in the
// hundreds per tick with a small mean size.
//
// Counting is a relaxed atomic add and a subtraction on the free path. That is
// not free, and it inflates the measured wall time of the timed loops below.
// The two are therefore run separately: `--alloc-report` counts, the timing
// scenarios do not, and no ratio in `results.json` is taken from a counting run.

/// The allocation counter, shared with `bench/async` by path inclusion.
///
/// It lives in its own file because a second copy of an instrument that
/// produces published numbers is a second instrument, and a reader comparing the
/// two rigs would be comparing two different measurements under one name.
#[path = "alloc_count.rs"]
mod alloc_count;

// ── Measurement ──────────────────────────────────────────────────────────────

/// One scenario's paired result.
struct Measurement {
    name: &'static str,
    intent: &'static str,
    /// Per-round bot duration, seconds.
    bot: Vec<f64>,
    /// Per-round baseline duration, seconds, paired with `bot` by index.
    base: Vec<f64>,
    ticks_per_round: u64,
    effects_bot: u64,
    effects_base: u64,
    evals_bot: u64,
    evals_base: u64,
    build_bot: Duration,
    /// Events the bot's own journal retained after the busiest round, and the
    /// crate's declared ceiling on them.
    ///
    /// Reported for every row rather than once in a footnote, because a bot
    /// whose journal is near its ceiling is a bot whose next effect is refused
    /// (INV-BOT-16: a scale measurement records the requested level, the level
    /// reached, and the ceiling together, so no reader is told a number nobody
    /// ran).
    journal_events: usize,
    journal_ceiling: usize,
}

/// Run one scenario paired: within a round, the bot is timed and then the
/// baseline is timed on the same input window, so machine drift lands on both
/// legs and cancels in the ratio.
///
/// **Each round builds its own bot and its own baseline, and warms both before
/// the timer starts.** Two reasons, and both are about what a round measures:
///
/// - Every bot this rig admits dispatches an external effect, so every fired
///   effect writes to the journal behind it. One bot for the whole run retains
///   every event it ever wrote, and `MAX_JOURNAL_EVENTS` (100 000) refuses the
///   next append rather than dropping evidence — so a workload that fires
///   millions of effects cannot be measured against one journal at all. A round
///   bounds what is retained, and `Scenario::ticks_per_round` is sized to stay
///   inside that ceiling.
/// - Admission is then outside the timed window on both legs, which is what the
///   ratio is supposed to be about: the cost of a steady-state tick, not of the
///   first tick a freshly built bot pays.
///
/// The input window keeps advancing across rounds, because a source's value is a
/// function of the tick index and a workload that restarted would be measuring a
/// different one.
fn measure(scenario: &Scenario) -> Result<Measurement, BenchError> {
    let clock = Clock::new();
    let tally = Rc::new(Cell::new(0u64));
    let evals_bot_cell = Rc::new(Cell::new(0u64));
    let evals_base_cell = Cell::new(0u64);

    let build_start = Instant::now();
    let admission = build_bot(scenario, &clock, &tally, &evals_bot_cell)?;
    let admission_cost = build_start.elapsed();
    drop(admission);

    let mut bot_tick = 0u64;
    let mut base_tick = 0u64;
    let mut effects_bot = 0u64;
    let mut effects_base = 0u64;
    let mut bot_times = Vec::with_capacity(scenario.rounds);
    let mut base_times = Vec::with_capacity(scenario.rounds);
    let mut journal_events = 0usize;

    for _ in 0..scenario.rounds {
        let mut bot = build_bot(scenario, &clock, &tally, &evals_bot_cell)?;
        let mut chains = build_handrolled(scenario);

        let warm = run_bot_ticks(&mut bot, &clock, &mut bot_tick, scenario.warmup_ticks)?;
        effects_bot = effects_bot.saturating_add(warm);
        let warm = run_base_ticks(
            &mut chains,
            &evals_base_cell,
            &mut base_tick,
            scenario.warmup_ticks,
        );
        effects_base = effects_base.saturating_add(warm);

        let start = Instant::now();
        let fired = run_bot_ticks(&mut bot, &clock, &mut bot_tick, scenario.ticks_per_round)?;
        let bot_elapsed = start.elapsed();
        effects_bot = effects_bot.saturating_add(fired);

        let start = Instant::now();
        let fired = run_base_ticks(
            &mut chains,
            &evals_base_cell,
            &mut base_tick,
            scenario.ticks_per_round,
        );
        let base_elapsed = start.elapsed();
        effects_base = effects_base.saturating_add(fired);

        journal_events = journal_events.max(retained_events(bot));
        bot_times.push(bot_elapsed.as_secs_f64());
        base_times.push(base_elapsed.as_secs_f64());
    }

    Ok(Measurement {
        name: scenario.name,
        intent: scenario.intent,
        bot: bot_times,
        base: base_times,
        ticks_per_round: scenario.ticks_per_round,
        effects_bot,
        effects_base,
        evals_bot: evals_bot_cell.get(),
        evals_base: evals_base_cell.get(),
        build_bot: admission_cost,
        journal_events,
        journal_ceiling: MAX_JOURNAL_EVENTS,
    })
}

/// How many events the bot's own journal retained, read back through the door
/// the crate provides for it.
///
/// `into_journal` rather than an accessor on the bot: the journal is the host's
/// record, and a measurement instrument is not a reason to add a getter to the
/// thing being measured. A journal that cannot answer retains nothing this rig
/// can report, which is a zero rather than a claim.
fn retained_events(bot: Bot) -> usize {
    match bot.into_journal() {
        Some(journal) => match journal.committed() {
            Ok(events) => events.len(),
            Err(error) => {
                lgwks_std::trace::debug!(
                    error = ?error,
                    "retained_events: the journal refused its committed events, so this round retains no row"
                );
                0
            }
        },
        None => 0,
    }
}

// ── The allocation model ─────────────────────────────────────────────────────

/// Rounds the allocation window counts, each on its own bot.
///
/// A round per window rather than one long window, because the journal is
/// bounded per bot: a single bot over 576 ticks of `churn-64x1` asks for 110,592
/// events against a ceiling of 100,000, and the report aborted there before
/// #640 instead of reaching the rows it exists for. Each round's warm-up stays
/// outside the counted window, as it stays outside the timed one.
const ALLOC_ROUNDS: usize = 8;

/// Count one scenario's steady-state allocations and append its row.
fn alloc_row(out: &mut String, scenario: &Scenario) -> Result<(), BenchError> {
    let clock = Clock::new();
    let tally = Rc::new(Cell::new(0u64));
    let evals_bot_cell = Rc::new(Cell::new(0u64));
    let evals_base_cell = Cell::new(0u64);

    let mut bot_allocs = 0u64;
    let mut bot_bytes = 0u64;
    let mut base_allocs = 0u64;
    let mut bot_tick = 0u64;
    let mut base_tick = 0u64;

    for _ in 0..ALLOC_ROUNDS {
        let mut bot = build_bot(scenario, &clock, &tally, &evals_bot_cell)?;
        let mut chains = build_handrolled(scenario);
        run_bot_ticks(&mut bot, &clock, &mut bot_tick, scenario.warmup_ticks)?;
        run_base_ticks(
            &mut chains,
            &evals_base_cell,
            &mut base_tick,
            scenario.warmup_ticks,
        );

        alloc_count::reset();
        alloc_count::start();
        run_bot_ticks(&mut bot, &clock, &mut bot_tick, scenario.ticks_per_round)?;
        alloc_count::stop();
        let (allocs, bytes) = alloc_count::snapshot();
        bot_allocs = bot_allocs.saturating_add(allocs);
        bot_bytes = bot_bytes.saturating_add(bytes);

        alloc_count::reset();
        alloc_count::start();
        run_base_ticks(
            &mut chains,
            &evals_base_cell,
            &mut base_tick,
            scenario.ticks_per_round,
        );
        alloc_count::stop();
        let (allocs, _) = alloc_count::snapshot();
        base_allocs = base_allocs.saturating_add(allocs);
    }

    let ticks = scenario.ticks_per_round as f64 * ALLOC_ROUNDS as f64;
    writeln!(
        out,
        "  {:<20} {:>10.1}   {:>10.1}   {:>10.1}   {:>19.1}",
        scenario.name,
        bot_allocs as f64 / ticks,
        bot_bytes as f64 / ticks,
        bot_bytes as f64 / bot_allocs.max(1) as f64,
        base_allocs as f64 / ticks,
    )?;
    Ok(())
}

/// Count the heap allocations one tick costs, per scenario.
///
/// The bot is built and warmed *outside* the counted window, so what is counted
/// is the steady-state tick and not the admission cost. The baseline is counted
/// through the same window, which is the control: a hand-rolled loop over the
/// same workload should allocate nothing, and a non-zero count there would mean
/// the counter is picking up something other than the bot.
fn alloc_report(only: Option<&str>) -> Result<String, BenchError> {
    let mut out = String::new();
    writeln!(
        out,
        "\nallocation model -- steady-state tick, built and warmed outside the counted window\n\
         =================================================================================="
    )?;
    writeln!(
        out,
        "  scenario                allocs/tick   bytes/tick   mean bytes   baseline allocs/tick"
    )?;

    for scenario in selected(only)? {
        alloc_row(&mut out, scenario)?;
    }
    // Per-tick trace for one scenario. A mean over 512 ticks hides the shape of
    // the cost: a bot that allocates nothing on a steady tick and heavily on a
    // moving one has the same mean as one that allocates the same amount every
    // tick, and only the trace tells them apart. This is what says whether the
    // observation path still allocates on a tick where nothing moved.
    if only.is_some() {
        // A filtered report is one row, and the trace belongs to `steady-64x100`
        // whatever was asked for: printing it beside another row would attach one
        // scenario's shape to another scenario's mean.
        return Ok(out);
    }
    let scenario = &SCENARIOS[1];
    let clock = Clock::new();
    let tally = Rc::new(Cell::new(0u64));
    let evals = Rc::new(Cell::new(0u64));
    let mut bot = build_bot(scenario, &clock, &tally, &evals)?;
    for tick in 0..64u64 {
        clock.set(tick);
        bot.tick()?;
    }
    write!(
        out,
        "\n  per-tick allocations, {} (a value moves every {} ticks)\n    ",
        scenario.name, scenario.period
    )?;
    for tick in 64..124u64 {
        clock.set(tick);
        alloc_count::reset();
        alloc_count::start();
        bot.tick()?;
        alloc_count::stop();
        let (tick_allocs, _) = alloc_count::snapshot();
        write!(out, "{tick}:{tick_allocs} ")?;
    }
    writeln!(out, "\n")?;
    Ok(out)
}

// ── Reporting ────────────────────────────────────────────────────────────────

/// The scenarios this invocation measures: the one `--scenario=` names, or every
/// row in the table.
///
/// A name the table does not hold is refused rather than ignored: a mistyped
/// scenario that silently ran the whole rig would print numbers for a run nobody
/// asked for, and a mistyped one that ran nothing would report success.
fn selected(only: Option<&str>) -> Result<Vec<&'static Scenario>, BenchError> {
    let Some(name) = only else {
        return Ok(SCENARIOS.iter().collect());
    };
    match scenarios::named(name) {
        Some(scenario) => Ok(vec![scenario]),
        None => {
            let known = SCENARIOS
                .iter()
                .map(|scenario| scenario.name)
                .collect::<Vec<_>>()
                .join(", ");
            let refusal = BenchError::UnknownScenario {
                name: name.to_owned(),
                known,
            };
            lgwks_std::trace::debug!(error = ?refusal, "selected: refusing an unknown scenario name");
            Err(refusal)
        }
    }
}

/// Measure one scenario, refuse an unfair comparison, and append its human and JSON rows.
fn scenario_report(
    scenario: &Scenario,
    human: &mut String,
    json: &mut String,
) -> Result<Measurement, BenchError> {
    let m = measure(scenario)?;

    // THE FAIRNESS GATE, on both axes. A ratio between two engines doing
    // different amounts of work is not a measurement, so refuse to report
    // one. Effects catch a baseline that produces different results;
    // evaluations catch a baseline that reaches the same results by
    // skipping the work.
    if m.effects_bot != m.effects_base {
        let refusal = BenchError::Unfair {
            scenario: m.name,
            what: "effects",
            bot: m.effects_bot,
            base: m.effects_base,
        };
        lgwks_std::trace::debug!(error = ?refusal, "main: returning an error to the caller");
        return Err(refusal);
    }
    if m.evals_bot != m.evals_base {
        let refusal = BenchError::Unfair {
            scenario: m.name,
            what: "evaluations",
            bot: m.evals_bot,
            base: m.evals_base,
        };
        lgwks_std::trace::debug!(error = ?refusal, "main: returning an error to the caller");
        return Err(refusal);
    }

    let mut ratios: Vec<f64> = m
        .bot
        .iter()
        .zip(m.base.iter())
        .map(|(b, r)| if *r > 0.0 { b / r } else { f64::NAN })
        .collect();

    let ticks = m.ticks_per_round as f64;
    let mut bot_tps: Vec<f64> = m.bot.iter().map(|d| ticks / d).collect();
    let mut base_tps: Vec<f64> = m.base.iter().map(|d| ticks / d).collect();

    let ratio_median = stats::median(&mut ratios);
    let (ci_lo, ci_hi) = stats::bootstrap_median_ci(&ratios, 10_000, 0.95, 0x5EED_1234);
    let bot_median_tps = stats::median(&mut bot_tps);
    let base_median_tps = stats::median(&mut base_tps);

    writeln!(human, "scenario  {}", m.name)?;
    writeln!(human, "  intent  {}", m.intent)?;
    writeln!(
        human,
        "  workload  {} sources x {} entries, {} ticks/round x {} rounds",
        scenario.sources, scenario.entries, m.ticks_per_round, scenario.rounds
    )?;
    writeln!(
        human,
        "  fairness  {} effects and {} condition evaluations, identical in both engines",
        m.effects_bot, m.evals_bot
    )?;
    writeln!(
        human,
        "  bot       {:.0} ticks/s   ({:.1} ns/tick)",
        bot_median_tps,
        1e9 / bot_median_tps
    )?;
    writeln!(
        human,
        "  baseline  {:.0} ticks/s   ({:.1} ns/tick)",
        base_median_tps,
        1e9 / base_median_tps
    )?;
    writeln!(
        human,
        "  ratio     {:.2}x  bot / baseline   95% CI [{:.2}, {:.2}]  {}",
        ratio_median,
        ci_lo,
        ci_hi,
        if stats::distinguishes_parity(ci_lo, ci_hi) {
            "(excludes parity)"
        } else {
            "(INCLUDES parity -- not a distinguishable difference)"
        }
    )?;
    writeln!(
        human,
        "  journal   {} events retained after the busiest round, ceiling {}",
        m.journal_events, m.journal_ceiling
    )?;
    writeln!(
        human,
        "  build     {} chains admitted in {:.3} ms\n",
        scenario.sources,
        m.build_bot.as_secs_f64() * 1e3
    )?;

    writeln!(
        json,
        "  \"{}\": {{ \"sources\": {}, \"entries\": {}, \"ticks_per_round\": {}, \
         \"rounds\": {}, \"effects\": {}, \"evaluations\": {}, \
         \"bot_ticks_per_sec\": {:.1}, \"baseline_ticks_per_sec\": {:.1}, \
         \"ratio_median\": {:.4}, \"ratio_ci95\": [{:.4}, {:.4}], \
         \"journal_events\": {}, \"journal_ceiling\": {}, \"build_ms\": {:.4} }},",
        m.name,
        scenario.sources,
        scenario.entries,
        m.ticks_per_round,
        scenario.rounds,
        m.effects_bot,
        m.evals_bot,
        bot_median_tps,
        base_median_tps,
        ratio_median,
        ci_lo,
        ci_hi,
        m.journal_events,
        m.journal_ceiling,
        m.build_bot.as_secs_f64() * 1e3
    )?;
    Ok(m)
}

/// One measured row's ns/tick, the figure the slope is fitted on.
fn ns_per_tick(m: &Measurement) -> f64 {
    let mut rounds = m.bot.clone();
    let median = stats::median(&mut rounds);
    if m.ticks_per_round == 0 {
        return f64::NAN;
    }
    (median / m.ticks_per_round as f64) * 1e9
}

/// The slope of ns/tick against source count over the `wide-*` rows in `rows`.
///
/// Appended to the human report, so the sentence a reader quotes from §4.9 is a
/// line through the medians printed directly above it. A run that measured fewer
/// than two wide rows has no line to fit and says so rather than printing a
/// slope from one point.
fn wide_scaling(rows: &[Measurement], human: &mut String) -> Result<(), BenchError> {
    let points: Vec<(f64, f64)> = rows
        .iter()
        .filter(|row| row.name.starts_with("wide-"))
        .filter_map(|row| {
            scenarios::named(row.name).map(|scenario| (scenario.sources as f64, ns_per_tick(row)))
        })
        .collect();
    if points.len() < 2 {
        return Ok(());
    }
    match fit::loglog(&points) {
        Some((slope, _)) => {
            writeln!(
                human,
                "wide scaling -- least squares of ln(ns/tick) on ln(sources), {count} rows\n  \
                 slope {slope:.3} (1.000 is linear in the source count, 2.000 quadratic); \
                 {factor:.2}x cost per doubling",
                count = points.len(),
                factor = 2f64.powf(slope),
            )?;
        }
        None => {
            writeln!(
                human,
                "wide scaling -- no fit: the wide rows carry a non-positive value"
            )?;
        }
    }
    Ok(())
}

fn main() -> Result<(), BenchError> {
    // The report goes through the estate's tracing, not `println!`: levelled,
    // filterable through `LGWKS_LOG`, and never a panic on a closed pipe.
    lgwks_std::trace::install_default("lgwks-bench")?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let run_timings = !args.iter().any(|a| a == "--capcheck-only");
    // Both spellings, `--scenario NAME` and `--scenario=NAME`: the issue and
    // `bench/README.md` write the first, and a command that silently ignored its
    // filter would report every row without saying so.
    let named_scenario = match args.iter().position(|a| a == "--scenario") {
        Some(index) => match args.get(index.saturating_add(1)) {
            Some(value) => Some(value.clone()),
            None => {
                let refusal = BenchError::MissingArg;
                lgwks_std::trace::warn!(error = ?refusal, "main: refusing a filter with no name");
                return Err(refusal);
            }
        },
        None => args
            .iter()
            .find_map(|a| a.strip_prefix("--scenario="))
            .map(str::to_owned),
    };
    let only = named_scenario.as_deref();

    // Both doors are asked for together in the issue's verification command, and
    // a command that silently ran one of the two would report half a measurement
    // under a name that names both.
    let want_alloc = args.iter().any(|a| a == "--alloc-report");
    let want_profile = args.iter().any(|a| a == "--profile");
    if want_alloc || want_profile {
        if want_alloc {
            info!("{}", alloc_report(only)?);
        }
        if want_profile {
            info!("{}", profile::stage_report(only)?);
        }
        return Ok(());
    }

    let mut json = String::from("{\n");
    let mut human = String::new();

    writeln!(
        human,
        "\nlgwks_bot benchmark rig -- paired design, ratios are bot / hand-rolled baseline\n\
         ==============================================================================\n"
    )?;

    let mut rows: Vec<Measurement> = Vec::new();
    if run_timings {
        for scenario in selected(only)? {
            rows.push(scenario_report(scenario, &mut human, &mut json)?);
        }
    }

    // Capability-check scaling.
    let scaling = probes::cap_check_scaling()?;
    writeln!(
        human,
        "capability-check scaling -- Auth::check as required-capability count grows"
    )?;
    writeln!(human, "  required   ns/call    ns per required cap")?;
    for (count, ns) in &scaling {
        writeln!(
            human,
            "  {:>8}   {:>7.2}    {:>18.3}",
            count,
            ns,
            ns / (*count as f64)
        )?;
    }
    // A scaling line needs both ends; a run that produced no points has no growth
    // to report, and inventing one (a 1.0 or a NaN) would print a number nobody
    // measured.
    let (Some(&(first_caps, first_ns)), Some(&(last_caps, last_ns))) =
        (scaling.first(), scaling.last())
    else {
        let refusal = BenchError::NoScalingPoints;
        lgwks_std::trace::warn!(error = ?refusal, "capability scaling: no points to report");
        return Err(refusal);
    };
    let growth = last_caps as f64 / first_caps as f64;
    writeln!(
        human,
        "  growth    {:.1}x cost for {:.0}x capabilities (quadratic would be {:.0}x)\n",
        last_ns / first_ns,
        growth,
        growth * growth
    )?;

    // Build cost.
    let build_1 = probes::build_cost(1)?;
    let build_64 = probes::build_cost(64)?;
    writeln!(
        human,
        "admission (build) cost\n  1 source: {:.3} ms\n  64 sources: {:.3} ms\n",
        build_1.as_secs_f64() * 1e3,
        build_64.as_secs_f64() * 1e3
    )?;

    // Determinism.
    let (deterministic, fired) = probes::determinism_probe(&SCENARIOS[1], 500)?;
    writeln!(
        human,
        "determinism probe (500 ticks, replayed)\n  identical per-tick effect sequence: {}\n  total effects: {}\n",
        if deterministic { "YES" } else { "NO" },
        fired
    )?;

    // The slope over the wide rows, after the rows it is fitted on.
    wide_scaling(&rows, &mut human)?;

    json.push_str(&format!(
        "  \"cap_check_scaling\": [{}],\n  \"determinism\": {},\n  \"build_ms_1\": {:.4},\n  \"build_ms_64\": {:.4}\n}}\n",
        scaling
            .iter()
            .map(|(c, ns)| format!("{{\"caps\": {c}, \"ns_per_call\": {ns:.3}}}"))
            .collect::<Vec<_>>()
            .join(", "),
        deterministic,
        build_1.as_secs_f64() * 1e3,
        build_64.as_secs_f64() * 1e3
    ));

    info!("{human}");

    if let Some(path) = args.iter().find(|a| a.starts_with("--json=")) {
        let path = path.trim_start_matches("--json=");
        std::fs::write(path, &json).map_err(|cause| BenchError::Io {
            path: std::path::PathBuf::from(path),
            cause,
        })?;
        info!("wrote {path}");
    }

    Ok(())
}
