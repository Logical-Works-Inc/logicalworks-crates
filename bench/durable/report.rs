//! What a measurement prints, human or JSON, from one set of numbers.
//!
//! A report is a plain struct. The JSON document is that struct serialized and
//! the text is that struct formatted, so the two cannot carry different numbers.
//! Every document names the host, the `lgwks_bot` version, the command line and
//! the seed, and every distribution names its sample count.

use lgwks_std::json::Serialize;

use crate::error::BenchError;
use crate::stats::{Summary, micros};
use crate::world::{ATTEMPTS, FAILURE_PERCENT, Latencies};

/// The flow's own source, counted at build time for the code-size line.
const FLOW_SOURCE: &str = include_str!("flow.rs");

/// The manifest of the crate this binary measures, read at build time for its
/// version: `lgwks_bot` exports no version constant.
const BOT_MANIFEST: &str = include_str!("../../crates/lgwks-bot/Cargo.toml");

/// What the store is, in words a reader can hold the numbers against.
const STORE: &str = "run store on disk, one file per tenant";

/// The flush setting, which the store does not let a caller change.
const FSYNC: &str = "sync_all on every group-committed batch; not configurable";

/// Lines of code in `source`: not blank, not a comment.
pub(crate) fn code_lines(source: &str) -> usize {
    source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//"))
        .count()
}

/// Lines of code in the flow, counted from the source embedded at build time.
pub(crate) fn flow_code_lines() -> usize {
    code_lines(FLOW_SOURCE)
}

/// The `version` the manifest's `[package]` table declares.
///
/// # Errors
///
/// [`BenchError::MalformedRecord`] when the table has no quoted version.
pub(crate) fn bot_version(manifest: &str) -> Result<String, BenchError> {
    let mut in_package = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package
            && let Some(rest) = line.strip_prefix("version")
            && let Some(quoted) = rest.trim_start().strip_prefix('=')
            && let Some(text) = quoted.trim().strip_prefix('"')
            && let Some((version, _)) = text.split_once('"')
        {
            return Ok(version.to_owned());
        }
    }
    Err(BenchError::MalformedRecord {
        file: "lgwks-bot/Cargo.toml",
        line: "no [package] version".to_owned(),
    })
}

/// The facts every number in a document is read against.
#[derive(Debug, Clone, Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub(crate) struct Envelope {
    /// The binary.
    tool: &'static str,
    /// The `lgwks_bot` version measured.
    lgwks_bot_version: String,
    /// The OS and architecture, as `std::env::consts` names them.
    host: String,
    /// The command line.
    command: String,
    /// The seed every draw came from.
    seed: u64,
    /// The runs asked for.
    runs: u32,
}

impl Envelope {
    /// The envelope for a run of `command` under `seed` over `runs` runs.
    ///
    /// # Errors
    ///
    /// [`BenchError::MalformedRecord`] if the bot's version cannot be read.
    pub(crate) fn new(command: &str, seed: u64, runs: u32) -> Result<Self, BenchError> {
        Ok(Self {
            tool: crate::args::PROGRAM,
            lgwks_bot_version: bot_version(BOT_MANIFEST)?,
            host: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            command: command.to_owned(),
            seed,
            runs,
        })
    }

    /// The lines every text report opens with.
    fn lines(&self, mode: &str) -> Vec<String> {
        vec![
            format!("{} ({mode})", self.tool),
            format!("host: {}", self.host),
            format!("lgwks_bot: {}", self.lgwks_bot_version),
            format!("command: {}", self.command),
            format!("seed: {}", self.seed),
            format!("runs: {}", self.runs),
        ]
    }
}

/// Where the records went and how they were flushed.
#[derive(Debug, Clone, Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub(crate) struct Durability {
    /// What the store is.
    store: &'static str,
    /// The flush setting.
    fsync: &'static str,
    /// `sync_all` calls made.
    fsyncs: u64,
    /// Records staged.
    records_staged: u64,
}

impl Durability {
    /// The durability a run measured.
    pub(crate) const fn new(fsyncs: u64, records_staged: u64) -> Self {
        Self {
            store: STORE,
            fsync: FSYNC,
            fsyncs,
            records_staged,
        }
    }
}

/// The workload the numbers are for.
#[derive(Debug, Clone, Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub(crate) struct Workload {
    /// Durable steps per run.
    steps: u32,
    /// Fixed latency of the outside call.
    outside_call_us: u64,
    /// Fixed latency of the model call.
    model_call_us: u64,
    /// Delay before the approval is granted.
    approval_us: u64,
    /// Share of attempts that fail, in percent.
    failure_percent: u64,
    /// Attempts per call under the retry policy.
    attempts: u32,
    /// The retry class the failures carry.
    retry_class: &'static str,
}

impl Workload {
    /// The workload with `latencies`.
    pub(crate) fn of(latencies: Latencies) -> Self {
        Self {
            steps: 10,
            outside_call_us: micros(latencies.outside),
            model_call_us: micros(latencies.model),
            approval_us: micros(latencies.approval),
            failure_percent: FAILURE_PERCENT,
            attempts: ATTEMPTS.get(),
            retry_class: "Safe",
        }
    }
}

/// The report of a flow measurement.
#[derive(Debug, Clone, Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub(crate) struct FlowDoc {
    /// What every number is read against.
    #[serde(flatten)]
    pub(crate) envelope: Envelope,
    /// `flow`.
    pub(crate) mode: &'static str,
    /// Tenants the runs were split over.
    pub(crate) tenants: usize,
    /// Workers driving runs.
    pub(crate) workers: u32,
    /// Where records went.
    pub(crate) durability: Durability,
    /// What was run.
    pub(crate) workload: Workload,
    /// Runs that finished: the sample count of the latency distribution.
    pub(crate) samples: usize,
    /// End-to-end latency of a finished run, in microseconds.
    pub(crate) latency_us: Summary,
    /// Microseconds from starting the rig, both tenants' hosts and run stores,
    /// until it could take its first run.
    pub(crate) deploy_us: Summary,
    /// Finished runs per second of wall time.
    pub(crate) throughput_runs_per_s: f64,
    /// Retries taken.
    pub(crate) retries: u64,
    /// Failures injected.
    pub(crate) failures_injected: u64,
    /// Runs the host's retry budget gave up.
    pub(crate) throttled_runs: u64,
    /// Runs in which every attempt of a call failed.
    pub(crate) exhausted_runs: u64,
    /// The process resident set.
    pub(crate) peak_rss_bytes: u64,
    /// What read the resident set.
    pub(crate) peak_rss_source: &'static str,
    /// Lines of code in the flow.
    pub(crate) flow_code_lines: usize,
}

/// The report of a recovery measurement.
#[derive(Debug, Clone, Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub(crate) struct RecoverDoc {
    /// What every number is read against.
    #[serde(flatten)]
    pub(crate) envelope: Envelope,
    /// `recover`.
    pub(crate) mode: &'static str,
    /// Tenants the runs were split over.
    pub(crate) tenants: usize,
    /// Where records went.
    pub(crate) durability: Durability,
    /// What was run.
    pub(crate) workload: Workload,
    /// The recovery numbers.
    pub(crate) recovery: Recovery,
}

/// What the kills and the resumes came to.
#[derive(Debug, Clone, Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub(crate) struct Recovery {
    /// Kill-and-resume rounds taken.
    pub(crate) rounds: usize,
    /// Runs parked, so mid-flight, when each round's child was killed.
    pub(crate) parked_runs_per_round: Vec<u32>,
    /// Microseconds from the kill to the last resumed run done: the
    /// distribution over the rounds.
    pub(crate) kill_to_done_us: Summary,
    /// The same, round by round.
    pub(crate) per_round_us: Vec<u64>,
    /// Runs resumed to their end across all rounds.
    pub(crate) resumed_runs: u64,
    /// (tenant, run, step) triples the step log was checked on.
    pub(crate) steps_checked: u64,
    /// Triples whose step body ran more than once: always zero in a document,
    /// because a nonzero count ends the run with an error instead.
    pub(crate) duplicate_step_executions: u64,
}

/// A flow report as text.
pub(crate) fn flow_text(doc: &FlowDoc) -> String {
    let mut lines = doc.envelope.lines("flow");
    lines.push(format!("tenants: {}", doc.tenants));
    lines.push(format!("workers: {}", doc.workers));
    lines.push(format!("store: {}", doc.durability.store));
    lines.push(format!("fsync: {}", doc.durability.fsync));
    lines.push(format!("fsyncs: {}", doc.durability.fsyncs));
    lines.push(format!("records_staged: {}", doc.durability.records_staged));
    lines.push(workload_line(&doc.workload));
    lines.push(format!("samples: {}", doc.samples));
    lines.push(summary_line("latency_us", &doc.latency_us));
    lines.push(summary_line("deploy_us", &doc.deploy_us));
    lines.push(format!(
        "throughput_runs_per_s: {}",
        doc.throughput_runs_per_s
    ));
    lines.push(format!("retries: {}", doc.retries));
    lines.push(format!("failures_injected: {}", doc.failures_injected));
    lines.push(format!("throttled_runs: {}", doc.throttled_runs));
    lines.push(format!("exhausted_runs: {}", doc.exhausted_runs));
    lines.push(format!("peak_rss_bytes: {}", doc.peak_rss_bytes));
    lines.push(format!("peak_rss_source: {}", doc.peak_rss_source));
    lines.push(format!("flow_code_lines: {}", doc.flow_code_lines));
    lines.join("\n")
}

/// A recovery report as text.
pub(crate) fn recover_text(doc: &RecoverDoc) -> String {
    let mut lines = doc.envelope.lines("recover");
    lines.push(format!("tenants: {}", doc.tenants));
    lines.push(format!("store: {}", doc.durability.store));
    lines.push(format!("fsync: {}", doc.durability.fsync));
    lines.push(workload_line(&doc.workload));
    let recovery = &doc.recovery;
    lines.push(format!("rounds: {}", recovery.rounds));
    lines.push(format!(
        "parked_runs_per_round: {:?}",
        recovery.parked_runs_per_round
    ));
    lines.push(summary_line("kill_to_done_us", &recovery.kill_to_done_us));
    lines.push(format!("per_round_us: {:?}", recovery.per_round_us));
    lines.push(format!("resumed_runs: {}", recovery.resumed_runs));
    lines.push(format!("steps_checked: {}", recovery.steps_checked));
    lines.push(format!(
        "duplicate_step_executions: {}",
        recovery.duplicate_step_executions
    ));
    lines.join("\n")
}

/// One engine's numbers in a table: the same measures for every row.
#[derive(Debug, Clone, Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub(crate) struct RowDoc {
    /// The row's name, as `--rows` spells it.
    pub(crate) row: &'static str,
    /// The engine and its version.
    pub(crate) engine: String,
    /// How tenants are kept apart.
    pub(crate) tenancy: String,
    /// What the engine flushes.
    pub(crate) durability: String,
    /// Which processes the resident set covers.
    pub(crate) rss_covers: &'static str,
    /// The runs asked for.
    pub(crate) runs: u32,
    /// Runs that finished: the sample count of the latency distribution.
    pub(crate) samples: usize,
    /// End-to-end latency of a finished run, in microseconds.
    pub(crate) latency_us: Summary,
    /// Finished runs per second of wall time.
    pub(crate) throughput_runs_per_s: f64,
    /// The resident set, in bytes.
    pub(crate) peak_rss_bytes: u64,
    /// Retries taken.
    pub(crate) retries: u64,
    /// Failures injected.
    pub(crate) failures_injected: u64,
    /// Runs the engine's retry budget gave up.
    pub(crate) throttled_runs: u64,
    /// Runs in which every attempt of a call failed.
    pub(crate) exhausted_runs: u64,
    /// Lines of code in the flow.
    pub(crate) code_lines: usize,
    /// Microseconds from starting the engine until it could take its first run:
    /// one sample per start, and per restart of the recovery rounds where the
    /// engine has a server to restart.
    pub(crate) deploy_us: Summary,
    /// The kill -9 recovery, when it was asked for.
    pub(crate) recovery: Option<Recovery>,
}

impl RowDoc {
    /// The lgwks row of a table, from the flow report of its own run.
    pub(crate) fn of_lgwks(doc: &FlowDoc) -> Self {
        Self {
            row: "lgwks",
            engine: format!("lgwks_bot {}", doc.envelope.lgwks_bot_version),
            tenancy: "two tenants: a host and a run store each".to_owned(),
            durability: format!("{}; {}", doc.durability.store, doc.durability.fsync),
            rss_covers: "this process, ps after the run",
            runs: doc.envelope.runs,
            samples: doc.samples,
            latency_us: doc.latency_us,
            throughput_runs_per_s: doc.throughput_runs_per_s,
            peak_rss_bytes: doc.peak_rss_bytes,
            retries: doc.retries,
            failures_injected: doc.failures_injected,
            throttled_runs: doc.throttled_runs,
            exhausted_runs: doc.exhausted_runs,
            code_lines: doc.flow_code_lines,
            deploy_us: doc.deploy_us,
            recovery: None,
        }
    }
}

/// The report of a table: every row the command asked for, on one workload.
#[derive(Debug, Clone, Serialize)]
#[serde(crate = "lgwks_std::json::serde")]
pub(crate) struct TableDoc {
    /// What every number is read against.
    #[serde(flatten)]
    pub(crate) envelope: Envelope,
    /// `table`.
    pub(crate) mode: &'static str,
    /// Tenants the runs were split over.
    pub(crate) tenants: usize,
    /// Runs in flight at once.
    pub(crate) workers: u32,
    /// What was run.
    pub(crate) workload: Workload,
    /// One row per engine, in the order asked.
    pub(crate) rows: Vec<RowDoc>,
}

/// Milliseconds with one decimal, from microseconds.
fn ms(us: u64) -> String {
    format!("{}.{}", us / 1_000, us % 1_000 / 100)
}

/// Megabytes with one decimal, from bytes.
fn mb(bytes: u64) -> String {
    let tenths = bytes / 100_000;
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// The measures of a table, one line each: its label and one cell per row.
fn table_lines(doc: &TableDoc) -> Vec<(&'static str, Vec<String>)> {
    let each = |cell: &dyn Fn(&RowDoc) -> String| doc.rows.iter().map(cell).collect::<Vec<_>>();
    let recovery = |cell: &dyn Fn(&Recovery) -> String| {
        each(&|row| match row.recovery.as_ref() {
            Some(found) => cell(found),
            None => "not run".to_owned(),
        })
    };
    vec![
        ("engine", each(&|row| row.engine.clone())),
        ("tenancy", each(&|row| row.tenancy.clone())),
        ("durability", each(&|row| row.durability.clone())),
        (
            "runs asked, finished (samples)",
            each(&|row| format!("{}, {}", row.runs, row.samples)),
        ),
        (
            "throughput",
            each(&|row| format!("{} runs/s", row.throughput_runs_per_s)),
        ),
        (
            "latency p50 / p95 / p99 (max)",
            each(&|row| {
                let l = &row.latency_us;
                format!("{} / {} / {} ms ({})", ms(l.p50), ms(l.p95), ms(l.p99), ms(l.max))
            }),
        ),
        (
            "peak RSS (ps, after the run)",
            each(&|row| format!("{} MB ({})", mb(row.peak_rss_bytes), row.rss_covers)),
        ),
        (
            "retries taken, failures injected",
            each(&|row| format!("{}, {}", row.retries, row.failures_injected)),
        ),
        (
            "runs given up: retry budget, every attempt failed",
            each(&|row| format!("{}, {}", row.throttled_runs, row.exhausted_runs)),
        ),
        (
            "application code lines of the flow",
            each(&|row| row.code_lines.to_string()),
        ),
        (
            "kill -9 to every run done, p50 over the rounds",
            recovery(&|found| {
                format!(
                    "{} ms ({} rounds, {} runs parked in all)",
                    ms(found.kill_to_done_us.p50),
                    found.rounds,
                    found.resumed_runs
                )
            }),
        ),
        (
            "step bodies run twice after recovery (checked)",
            recovery(&|found| {
                format!("{} ({})", found.duplicate_step_executions, found.steps_checked)
            }),
        ),
        (
            "deploy: start to ready (samples)",
            each(&|row| {
                let noun = if row.deploy_us.samples == 1 { "sample" } else { "samples" };
                format!("{} ms p50 ({} {noun})", ms(row.deploy_us.p50), row.deploy_us.samples)
            }),
        ),
    ]
}

/// A table as text: the envelope, the workload, then one line per measure.
pub(crate) fn table_text(doc: &TableDoc) -> String {
    let mut lines = doc.envelope.lines("table");
    lines.push(format!("tenants: {}", doc.tenants));
    lines.push(format!("workers: {}", doc.workers));
    lines.push(workload_line(&doc.workload));
    let names: Vec<&str> = doc.rows.iter().map(|row| row.row).collect();
    lines.push(format!("rows: {}", names.join(", ")));
    for (label, cells) in table_lines(doc) {
        let paired: Vec<String> = names
            .iter()
            .zip(&cells)
            .map(|(name, cell)| format!("{name}={cell}"))
            .collect();
        lines.push(format!("{label}: {}", paired.join(" | ")));
    }
    lines.join("\n")
}

/// A table as a Markdown table: one column per row, one line per measure.
pub(crate) fn table_markdown(doc: &TableDoc) -> String {
    let names: Vec<&str> = doc.rows.iter().map(|row| row.row).collect();
    let mut lines = vec![
        format!("| measure | {} |", names.join(" | ")),
        format!("|---|{}", "---|".repeat(names.len())),
    ];
    for (label, cells) in table_lines(doc) {
        lines.push(format!("| {label} | {} |", cells.join(" | ")));
    }
    lines.join("\n")
}

/// One distribution as a text line.
fn summary_line(name: &str, summary: &Summary) -> String {
    format!(
        "{name}: samples={} p50={} p95={} p99={} max={}",
        summary.samples, summary.p50, summary.p95, summary.p99, summary.max
    )
}

/// The workload as a text line.
fn workload_line(workload: &Workload) -> String {
    format!(
        "workload: steps={} outside_call_us={} model_call_us={} approval_us={} failure_percent={} attempts={} retry_class={}",
        workload.steps,
        workload.outside_call_us,
        workload.model_call_us,
        workload.approval_us,
        workload.failure_percent,
        workload.attempts,
        workload.retry_class
    )
}

/// The one JSON document for `doc`, on one line.
///
/// # Errors
///
/// [`BenchError::Json`] when the writer refuses.
pub(crate) fn json<T: Serialize>(doc: &T) -> Result<String, BenchError> {
    lgwks_std::json::to_string(doc).map_err(|source| BenchError::Json { source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lgwks_std::json::Value;
    use lgwks_std::seeded::Seeded;

    /// A test's result.
    type TestResult = Result<(), String>;

    /// A flow document with seeded numbers.
    fn seeded_doc(seed: u64) -> Result<FlowDoc, String> {
        let mut stream = Seeded::from_seed(seed);
        let mut draw = |bound| stream.below(bound).map_err(|error| error.to_string());
        let mut samples: Vec<u64> = Vec::new();
        for _ in 0..draw(60)? + 1 {
            samples.push(draw(90_000)?);
        }
        let latency_us = Summary::of(&mut samples);
        let envelope =
            Envelope::new("lgwks-bench-durable --runs 7", seed, 7).map_err(|error| error.line())?;
        let syncs = draw(500)?;
        let staged = draw(900)?;
        let centi_rate = draw(100_000)?;
        let retries = draw(99)?;
        let failures_injected = draw(99)?;
        let throttled_runs = draw(9)?;
        let exhausted_runs = draw(9)?;
        let resident = draw(1 << 30)?;
        let deploy_us = Summary::of(&mut [draw(900_000)?]);
        Ok(FlowDoc {
            envelope,
            mode: "flow",
            tenants: 2,
            workers: 64,
            durability: Durability::new(syncs, staged),
            workload: Workload::of(Latencies::WORKLOAD),
            samples: latency_us.samples,
            latency_us,
            deploy_us,
            throughput_runs_per_s: centi_rate as f64 / 100.0,
            retries,
            failures_injected,
            throttled_runs,
            exhausted_runs,
            peak_rss_bytes: resident + 1,
            peak_rss_source: "test",
            flow_code_lines: flow_code_lines(),
        })
    }

    /// Parse a document back.
    fn parsed(doc: &FlowDoc) -> Result<Value, String> {
        let text = json(doc).map_err(|error| error.line())?;
        lgwks_std::json::from_str::<Value>(&text).map_err(|error| error.to_string())
    }

    /// The text and the JSON of one document carry the same numbers, over many
    /// seeded documents: every number the text prints is the one in the JSON.
    #[test]
    fn sim_text_and_json_carry_the_same_numbers() -> TestResult {
        for seed in 1_u64..=50 {
            let doc = seeded_doc(seed)?;
            let value = parsed(&doc)?;
            let text = flow_text(&doc);
            let line = |key: &str| {
                text.lines()
                    .find_map(|line| line.strip_prefix(key))
                    .map(str::to_owned)
            };
            let number = |key: &str| value[key].as_u64().map(|found| found.to_string());
            for key in [
                "samples",
                "retries",
                "failures_injected",
                "throttled_runs",
                "exhausted_runs",
                "peak_rss_bytes",
                "flow_code_lines",
            ] {
                assert_eq!(line(&format!("{key}: ")), number(key), "{key} seed {seed}");
            }
            let rate = value["throughput_runs_per_s"]
                .as_f64()
                .map(|found| found.to_string());
            assert_eq!(line("throughput_runs_per_s: "), rate, "seed {seed}");
            let p99 = value["latency_us"]["p99"].as_u64().ok_or("no p99")?;
            let text_p99 = line("latency_us: ").ok_or("no latency line")?;
            assert!(text_p99.contains(&format!("p99={p99} ")), "{text_p99}");
            let deployed = value["deploy_us"]["p50"].as_u64().ok_or("no deploy p50")?;
            let text_deploy = line("deploy_us: ").ok_or("no deploy line")?;
            assert!(text_deploy.starts_with("samples=1 "), "{text_deploy}");
            assert!(text_deploy.contains(&format!("p50={deployed} ")), "{text_deploy}");
        }
        Ok(())
    }

    /// The document is one object with the envelope's facts at its top.
    #[test]
    fn a_document_names_host_version_command_and_seed() -> TestResult {
        let value = parsed(&seeded_doc(3)?)?;
        for key in ["host", "lgwks_bot_version", "command", "tool"] {
            assert!(value[key].is_string(), "{key} missing");
        }
        assert_eq!(value["seed"].as_u64(), Some(3));
        assert_eq!(value["mode"].as_str(), Some("flow"));
        Ok(())
    }

    /// The version is read from a manifest's `[package]` table and from nowhere
    /// else: a dependency's `version` key does not stand in for it.
    #[test]
    fn the_version_comes_from_the_package_table_only() -> TestResult {
        let manifest = "[dependencies]\nx = { version = \"9.9.9\" }\nversion = \"7.7.7\"\n[package]\nname = \"x\"\nversion = \"1.2.3\"\n[features]\nversion = \"4.4.4\"\n";
        assert_eq!(bot_version(manifest).map_err(|e| e.line())?, "1.2.3");
        assert!(matches!(
            bot_version("[package]\nname = \"x\"\n"),
            Err(BenchError::MalformedRecord { .. })
        ));
        let real = bot_version(BOT_MANIFEST).map_err(|error| error.line())?;
        assert_eq!(real.split('.').count(), 3, "{real}");
        Ok(())
    }

    /// Blank lines and comments are not code; the flow has real code in it.
    #[test]
    fn code_lines_skip_blanks_and_comments() {
        assert_eq!(code_lines("// a\n\n  // b\nlet x = 1;\n  y();\n"), 2);
        assert!(flow_code_lines() > 20);
    }
}
