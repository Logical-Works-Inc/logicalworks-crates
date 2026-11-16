//! Simulations of the two-row table and of the flags that ask for it.
//!
//! The table is one struct that prints three ways. The oracle is the struct's own
//! numbers, formatted a second way here: text, Markdown and JSON must agree with
//! them and with each other, in the order the rows were asked for.

use std::ffi::OsString;

use lgwks_std::json::Value;

use crate::args::{Mode, Row, parse};
use crate::fleet::{TENANTS, WORKERS};
use crate::report::{
    Durability, Envelope, FlowDoc, Recovery, RowDoc, TableDoc, Workload, json, table_markdown,
    table_text,
};
use crate::sim_kit::{Dice, TestResult, Trace, replays};
use crate::stats::Summary;
use crate::world::Latencies;

/// The number of measure lines a table carries.
const MEASURES: usize = 13;

/// A distribution drawn from seeded samples.
fn summary(dice: &mut Dice) -> Result<Summary, String> {
    let mut samples: Vec<u64> = (0..=dice.pick(50)?)
        .map(|_| dice.roll(900_000))
        .collect::<Result<_, _>>()?;
    Ok(Summary::of(&mut samples))
}

/// A row with seeded numbers, named `name`.
fn row(dice: &mut Dice, name: &'static str, recovering: bool) -> Result<RowDoc, String> {
    let latency_us = summary(dice)?;
    let recovery = if recovering {
        let rounds = 1 + dice.pick(5)?;
        Some(Recovery {
            rounds,
            parked_runs_per_round: vec![dice.small(900)?; rounds],
            kill_to_done_us: summary(dice)?,
            per_round_us: vec![dice.roll(9_000_000)?; rounds],
            resumed_runs: dice.roll(9_000)?,
            steps_checked: dice.roll(90_000)?,
            duplicate_step_executions: 0,
        })
    } else {
        None
    };
    let engine = format!("{name} engine {}", dice.small(99)?);
    let tenancy = format!("{name} keeps tenants apart by {}", dice.small(9)?);
    let durability = format!("{name} flushes {}", dice.small(9)?);
    let runs = 1 + dice.small(5_000)?;
    let throughput = dice.roll(1_000_000)?;
    let peak_rss_bytes = dice.roll(1 << 32)?;
    let retries = dice.roll(900)?;
    let failures_injected = dice.roll(900)?;
    let throttled_runs = dice.roll(90)?;
    let exhausted_runs = dice.roll(90)?;
    let code_lines = dice.pick(400)?;
    Ok(RowDoc {
        row: name,
        engine,
        tenancy,
        durability,
        rss_covers: "the test's processes",
        runs,
        samples: latency_us.samples,
        latency_us,
        throughput_runs_per_s: throughput as f64 / 100.0,
        peak_rss_bytes,
        retries,
        failures_injected,
        throttled_runs,
        exhausted_runs,
        code_lines,
        cold_build: None,
        recovery,
    })
}

/// A table of seeded rows, in a seeded order, with seeded recovery.
fn table(dice: &mut Dice, seed: u64) -> Result<TableDoc, String> {
    let recovering = dice.pick(2)? == 0;
    let mut names = vec!["lgwks", "restate"];
    if dice.pick(2)? == 0 {
        names.reverse();
    }
    if dice.pick(4)? == 0 {
        names.truncate(1);
    }
    let rows = names
        .into_iter()
        .map(|name| row(dice, name, recovering))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TableDoc {
        envelope: Envelope::new("lgwks-bench-durable --rows x", seed, 100).map_err(|e| e.line())?,
        mode: "table",
        tenants: TENANTS.len(),
        workers: WORKERS,
        workload: Workload::of(Latencies::WORKLOAD),
        rows,
    })
}

/// Milliseconds with one decimal, by a second route.
fn tenths_of_ms(us: u64) -> String {
    let tenths = us / 100;
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// The cells of the line whose label is `label`.
fn cells<'a>(markdown: &'a str, label: &str) -> Result<Vec<&'a str>, String> {
    let prefix = format!("| {label} | ");
    let line = markdown
        .lines()
        .find(|line| line.starts_with(&prefix))
        .ok_or_else(|| format!("no line for {label}: {markdown}"))?;
    let body = line.strip_prefix(&prefix).and_then(|rest| rest.strip_suffix(" |"));
    Ok(body.ok_or("a line does not close")?.split(" | ").collect())
}

/// Text, Markdown and JSON carry the numbers of the struct, in the order the rows
/// were asked for, over seeded tables of one or two rows with and without
/// recovery.
#[test]
fn sim_table_text_markdown_and_json_agree_with_the_rows() -> TestResult {
    for seed in 1..=200_u64 {
        let mut dice = Dice::new(seed);
        let doc = table(&mut dice, seed)?;
        let markdown = table_markdown(&doc);
        let lines: Vec<&str> = markdown.lines().collect();
        assert_eq!(lines.len(), 2 + MEASURES, "seed {seed}");
        let pipes = doc.rows.len() + 2;
        for line in &lines {
            assert_eq!(line.matches('|').count(), pipes, "seed {seed}: {line}");
        }
        let value = lgwks_std::json::from_str::<Value>(&json(&doc).map_err(|e| e.line())?)
            .map_err(|e| e.to_string())?;
        let text = table_text(&doc);
        for (at, row) in doc.rows.iter().enumerate() {
            assert_eq!(cells(&markdown, "engine")?.get(at), Some(&row.engine.as_str()), "seed {seed}");
            assert_eq!(value["rows"][at]["row"].as_str(), Some(row.row));
            assert_eq!(value["rows"][at]["samples"].as_u64(), u64::try_from(row.samples).ok());
            assert_eq!(value["rows"][at]["latency_us"]["p99"].as_u64(), Some(row.latency_us.p99));
            assert_eq!(value["rows"][at]["retries"].as_u64(), Some(row.retries));
            let latency = cells(&markdown, "latency p50 / p95 / p99 (max)")?;
            let want = format!(
                "{} / {} / {} ms ({})",
                tenths_of_ms(row.latency_us.p50),
                tenths_of_ms(row.latency_us.p95),
                tenths_of_ms(row.latency_us.p99),
                tenths_of_ms(row.latency_us.max)
            );
            assert_eq!(latency.get(at), Some(&want.as_str()), "seed {seed}");
            assert!(text.contains(&format!("{}={}", row.row, row.engine)), "seed {seed}");
            let recovery = cells(&markdown, "step bodies run twice after recovery (checked)")?;
            match &row.recovery {
                Some(found) => {
                    let want = format!("0 ({})", found.steps_checked);
                    assert_eq!(recovery.get(at), Some(&want.as_str()));
                    assert_eq!(value["rows"][at]["recovery"]["rounds"].as_u64(), u64::try_from(found.rounds).ok());
                }
                None => {
                    assert_eq!(recovery.get(at), Some(&"not run"));
                    assert!(value["rows"][at]["recovery"].is_null());
                }
            }
        }
        assert_eq!(value["rows"].as_array().map(Vec::len), Some(doc.rows.len()));
        assert_eq!(value["mode"].as_str(), Some("table"));
    }
    Ok(())
}

/// Two rows swapped in the asking are swapped in every cell: no number follows
/// the wrong engine's name.
#[test]
fn sim_rows_keep_their_numbers_when_the_order_flips() -> TestResult {
    for seed in 1..=60_u64 {
        let mut dice = Dice::new(seed);
        let (a, b) = (row(&mut dice, "lgwks", false)?, row(&mut dice, "restate", false)?);
        let make = |rows: Vec<RowDoc>| -> Result<TableDoc, String> {
            Ok(TableDoc {
                envelope: Envelope::new("c", seed, 1).map_err(|e| e.line())?,
                mode: "table",
                tenants: 2,
                workers: 64,
                workload: Workload::of(Latencies::WORKLOAD),
                rows,
            })
        };
        let forward = table_markdown(&make(vec![a.clone(), b.clone()])?);
        let reverse = table_markdown(&make(vec![b, a])?);
        for label in ["engine", "durability", "throughput", "application code lines of the flow"] {
            let (mut first, second) = (cells(&forward, label)?, cells(&reverse, label)?);
            first.reverse();
            assert_eq!(first, second, "seed {seed} {label}");
        }
    }
    Ok(())
}

/// One seed, one table: the three renderings hash the same on a second run.
#[test]
fn sim_table_renderings_replay_from_one_seed() -> TestResult {
    fn scenario(seed: u64) -> Result<u64, String> {
        let mut dice = Dice::new(seed);
        let mut trace = Trace::new();
        for _ in 0..40 {
            let doc = table(&mut dice, seed)?;
            trace.note(&table_text(&doc));
            trace.note(&table_markdown(&doc));
            trace.note(&json(&doc).map_err(|e| e.line())?);
        }
        Ok(trace.hash())
    }
    for seed in [2, 20, 200] {
        replays(scenario, seed)?;
    }
    Ok(())
}

/// The lgwks row of a table is the flow report's own numbers, and the cold build
/// is left open for the doc.
#[test]
fn the_lgwks_row_carries_the_flow_reports_numbers() -> TestResult {
    let mut dice = Dice::new(9);
    let latency_us = summary(&mut dice)?;
    let flow = FlowDoc {
        envelope: Envelope::new("lgwks-bench-durable", 1, 77).map_err(|e| e.line())?,
        mode: "flow",
        tenants: 2,
        workers: 64,
        durability: Durability::new(5, 9),
        workload: Workload::of(Latencies::WORKLOAD),
        samples: latency_us.samples,
        latency_us,
        throughput_runs_per_s: 12.5,
        retries: 3,
        failures_injected: 4,
        throttled_runs: 5,
        exhausted_runs: 6,
        peak_rss_bytes: 7_000_000,
        peak_rss_source: "test",
        flow_code_lines: 99,
    };
    let made = RowDoc::of_lgwks(&flow);
    assert_eq!(made.row, "lgwks");
    assert_eq!((made.runs, made.samples, made.retries), (77, flow.samples, 3));
    assert_eq!((made.throttled_runs, made.exhausted_runs, made.code_lines), (5, 6, 99));
    assert_eq!(made.latency_us, flow.latency_us);
    assert!(made.cold_build.is_none() && made.recovery.is_none());
    Ok(())
}

/// The command line as the OS hands it.
fn argv(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

/// What the flags mean, written plainly: the rows, the markdown switch and the
/// server named, or `None` for a usage refusal.
fn model(words: &[&str]) -> Option<(Vec<Row>, bool, String)> {
    let (mut rows, mut markdown, mut server) = (Vec::new(), false, "restate-server".to_owned());
    let (mut json, mut child, mut work_dir) = (false, false, false);
    let mut cursor = words.iter();
    while let Some(word) = cursor.next() {
        match *word {
            "--rows" => {
                rows.clear();
                for name in cursor.next()?.split(',') {
                    let row = match name {
                        "lgwks" => Row::Lgwks,
                        "restate" => Row::Restate,
                        _ => return None,
                    };
                    if rows.contains(&row) {
                        return None;
                    }
                    rows.push(row);
                }
            }
            "--restate-server" => server = (*cursor.next()?).to_owned(),
            "--work-dir" => {
                cursor.next()?;
                work_dir = true;
            }
            "--markdown" => markdown = true,
            "--json" => json = true,
            "--park-child" => child = true,
            "--recover" => {}
            _ => return None,
        }
    }
    let refused = (child && !work_dir) || (markdown && (json || rows.is_empty())) || (child && !rows.is_empty());
    (!refused).then_some((rows, markdown, server))
}

/// Seeded command lines over the new flags parse as the model says, or are a
/// usage refusal that exits 2.
#[test]
fn sim_rows_flags_parse_like_the_model() -> TestResult {
    let vocabulary = [
        "--rows", "lgwks", "restate", "lgwks,restate", "restate,lgwks", "lgwks,lgwks", "restate,", "",
        "--markdown", "--json", "--recover", "--restate-server", "elsewhere", "--park-child", "--work-dir", "w",
    ];
    let mut dice = Dice::new(61);
    let mut parsed = 0_u32;
    for _ in 0..6_000 {
        let mut words: Vec<&str> = Vec::new();
        for _ in 0..dice.pick(6)? {
            words.push(vocabulary.get(dice.pick(vocabulary.len())?).ok_or("draw")?);
        }
        match (parse(argv(&words)), model(&words)) {
            (Ok(args), Some((rows, markdown, server))) => {
                assert_eq!((args.rows, args.markdown, args.restate_server), (rows, markdown, server), "{words:?}");
                parsed += 1;
            }
            (Err(error), None) => assert_eq!(error.exit_code(), 2, "{words:?}"),
            (got, want) => return Err(format!("{words:?}: parsed {got:?}, model {want:?}")),
        }
    }
    assert!(parsed > 100, "the draws parsed only {parsed} lines");
    Ok(())
}

/// Recovery with rows keeps the recovery run bound, and no flag at all keeps the
/// lgwks row alone as before.
#[test]
fn rows_with_recover_keep_the_recovery_bound_and_no_rows_means_the_old_report() {
    let refused = parse(argv(&["--rows", "restate", "--recover", "--runs", "10001"]));
    assert!(refused.is_err());
    let args = parse(argv(&["--rows", "lgwks,restate", "--recover"]));
    assert!(matches!(&args, Ok(found) if found.mode == Mode::Recover && found.rows.len() == 2), "{args:?}");
    let plain = parse(argv(&[]));
    assert!(matches!(&plain, Ok(found) if found.rows.is_empty() && !found.markdown), "{plain:?}");
}
