//! The estate evening as a smoke: every tier, both doors, every refusal, one report.
//!
//! Run it on a built artifact to prove the assembly there. It builds each tier, runs through
//! `run_task` and `run_blocking`, cancels, drains both ways, drops an estate inside a runtime,
//! exercises every typed refusal, and prints the terminal record. Every number it prints is also
//! asserted: the first deviation records a mismatch and the exit is non-zero, so a clean exit is a
//! proved evening. `the_smoke_binary_proves_the_evening` (`tests/it/estate.rs`) runs it on every
//! test run and reads the same exit.

use std::future::Future;
use std::io::Write as _;
use std::pin::Pin;

use lgwks_bot::app_error::BotError;
use lgwks_bot::cap::Cap;
use lgwks_bot::ecs::EcsError;
use lgwks_bot::error::KernelError;
use lgwks_bot::gate::GrantReplacement;
use lgwks_bot::gate::GrantSet;
use lgwks_bot::rt::estate::{Estate, EstateError, EstateReport};
use lgwks_bot::rt::runtime::Handle;
use lgwks_bot::rt::supervise::TaskId;
use lgwks_bot::script::{FlowError, Scope};
use lgwks_bot::spec::Bot;
use lgwks_bot::spec::EffectScope;
use lgwks_bot::task::{Disposition, Report, Task, task};

/// The script vocabulary on the shipped binary: what each runtime answers
/// through the public interface. A submodule rather than inline code, so the
/// smoke keeps one responsibility and the vocabulary keeps its own.
#[path = "estate/vocabulary.rs"]
mod vocabulary;

/// Human approvals on the shipped binary: list, approve, deny over the file
/// store, driving the same decide path a flow's park is decided through.
#[path = "estate/approvals.rs"]
mod approvals;

/// Runs entering the world through the journal (ADR 0006 R3), on the shipped
/// binary.
#[path = "estate/run_events.rs"]
mod run_events;

/// The reference tenant HTTP edge (`estate serve`, #782): one typed draft
/// route over a file store, with the tenant from the path and idempotency.
#[path = "estate/serve_ref.rs"]
mod serve_ref;

/// Model-written plans over the tenant HTTP edge (`estate apps`, #786):
/// refuse, repair, run, and assert every printed line over loopback.
#[path = "estate/apps.rs"]
mod apps;
/// The offline half of `estate apps`: the library's verdicts match the edge's.
#[path = "estate/apps_offline.rs"]
mod apps_offline;

/// The sealed evening (`estate sealed`, #785): two tenants sealed, one
/// shredded, a restart, and a chain that still verifies.
#[path = "estate/sealed.rs"]
mod sealed;
/// The journal and owned-sweeper half of `estate sealed`.
#[cfg(feature = "sealed")]
#[path = "estate/sealed_journal.rs"]
mod sealed_journal;

/// Library doors with no front of their own, read as a downstream caller reads them: the versioned
/// record write, the guidance session, the upstream credential refusal and the stable read.
#[path = "estate/doors.rs"]
mod doors;

/// The supervisor's process doors, proved on the shipped artifact rather than
/// trusted from the tests: record-then-release gating, the release handshake,
/// and every deny-builder refusal arm.
#[cfg(all(unix, feature = "process"))]
#[path = "estate/process_doors.rs"]
mod process_doors;

/// One line to the caller's locked stdout; a write error reaches `main` as an I/O error.
fn say(line: &str) -> std::io::Result<()> {
    writeln!(std::io::stdout().lock(), "{line}")
}

/// What one run proved, owned so the async evening can hand it back to `main`.
#[derive(Debug)]
struct RunCheck {
    /// Whether the run succeeded.
    succeeded: bool,
    /// The value it answered, when it succeeded.
    output: Option<u64>,
    /// The located failure, when it did not.
    error: Option<String>,
}

/// The smoke's doubles, its error and its two directory helpers.
#[path = "estate/smoke.rs"]
mod smoke;
use smoke::{SmokeAction, SmokeError, SmokeSource};

/// The probe's body as a function pointer: a scope and a number in, one more out.
type ProbeBody = fn(Scope, u64) -> Pin<Box<dyn Future<Output = Result<u64, FlowError>>>>;

/// The probe task every tier runs.
type Probe = Task<ProbeBody>;

/// The probe's body: one more than its input.
fn probe_body(_scope: Scope, input: u64) -> Pin<Box<dyn Future<Output = Result<u64, FlowError>>>> {
    Box::pin(async move { Ok(input.saturating_add(1)) })
}

/// Record the first deviation and keep it: the first mismatch is the root cause, and later ones
/// are its symptoms.
fn record(mismatch: &mut Option<String>, message: String) {
    if mismatch.is_none() {
        *mismatch = Some(message);
    }
}

/// Eight random bytes as hex, the suffix that keeps one smoke's scratch names its own.
fn unique_hex() -> std::io::Result<String> {
    let unique = lgwks_std::random::bytes::<8>().map_err(std::io::Error::other)?;
    Ok(unique.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The smoke's scratch directory, removed on every exit — an early `?` return included.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    /// Remove the directory; a refusal is logged, because a destructor has nowhere to return it.
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            lgwks_std::trace::debug!(error = ?error, "estate-smoke: the scratch dir was not removed");
        }
    }
}

/// The sub-command the arguments name, run; `None` when they name none and the smoke is the job.
fn subcommand() -> Option<Result<(), SmokeError>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (first, rest) = args.split_first()?;
    match first.as_str() {
        "serve" => Some(serve_ref::run(rest).map_err(|cause| SmokeError::command("serve", &cause))),
        "apps" => Some(apps::run(rest).map_err(|cause| SmokeError::command("apps", &cause))),
        "approvals" => {
            let stdout = std::io::stdout();
            Some(
                approvals::run(rest, &mut stdout.lock())
                    .map_err(|cause| SmokeError::command("approvals", &cause)),
            )
        }
        "sealed" => {
            let stdout = std::io::stdout();
            Some(
                sealed::run(rest, &mut stdout.lock())
                    .map_err(|cause| SmokeError::command("sealed", &cause)),
            )
        }
        _ => None,
    }
}

/// What one probe run proved, read off its report.
fn check_of(run: &Report<u64>) -> RunCheck {
    RunCheck {
        succeeded: matches!(run.disposition(), Disposition::Succeeded),
        output: run.output().copied(),
        error: run.error().map(|error| error.to_string()),
    }
}

/// Say one blocking `label` run and assert it succeeded with `want`.
fn check_blocking_run(
    label: &str,
    run: &Report<u64>,
    want: u64,
    mismatch: &mut Option<String>,
) -> Result<(), SmokeError> {
    say(&format!(
        "{label} run: disposition={} output={:?}",
        run.disposition(),
        run.output(),
    ))?;
    if !matches!(run.disposition(), Disposition::Succeeded) || run.output() != Some(&want) {
        record(
            mismatch,
            format!(
                "the {label} run is not a successful {want}: {:?}",
                run.error()
            ),
        );
    }
    Ok(())
}

/// Assert a `label` drain accounts for exactly the one run it was given.
fn one_run_admitted(label: &str, report: &EstateReport, mismatch: &mut Option<String>) {
    if report.runs_admitted() != 1 {
        record(
            mismatch,
            format!(
                "the {label} drain accounts for one run, not {}",
                report.runs_admitted(),
            ),
        );
    }
}

/// The supervised tier: two probe runs through the process driver, a cancel, and the drain that
/// accounts for both.
fn supervised_tier(
    mut estate: Estate,
    driver: &Handle,
    probe: &Probe,
    mismatch: &mut Option<String>,
) -> Result<(), SmokeError> {
    let supervised = estate.supervisor().is_some();
    say(&format!(
        "estate: tenant={} tier={} runtime={} supervisor={supervised} store={}",
        estate.tenant(),
        estate.tier(),
        estate.handle().is_some(),
        estate.run_store().is_some(),
    ))?;
    let (first, second, report) = driver.block_on(async {
        let first = check_of(&estate.run_task(probe, 41).await);
        let second = check_of(&estate.run_task(probe, 42).await);
        estate.cancel();
        (first, second, estate.shutdown().await)
    });
    say(&format!(
        "run 41: succeeded={} output={:?} error={:?}",
        first.succeeded, first.output, first.error,
    ))?;
    say(&format!(
        "run 42: succeeded={} output={:?} error={:?}",
        second.succeeded, second.output, second.error,
    ))?;
    for (name, target, check) in [("41", 42u64, &first), ("42", 43u64, &second)] {
        if !check.succeeded || check.output != Some(target) {
            record(
                mismatch,
                format!(
                    "run {name} is not a successful {target}: succeeded={} output={:?} error={:?}",
                    check.succeeded, check.output, check.error,
                ),
            );
        }
    }
    say(&format!(
        "shutdown: tenant={} tier={} admitted={} refused={} supervised={}",
        report.tenant(),
        report.tier(),
        report.runs_admitted(),
        report.runs_refused(),
        report.supervised().succeeded,
    ))?;
    if report.runs_admitted() != 2 || report.runs_refused() != 0 {
        record(
            mismatch,
            format!(
                "the shutdown accounts for two admitted runs, not {} admitted and {} refused",
                report.runs_admitted(),
                report.runs_refused(),
            ),
        );
    }
    Ok(())
}

/// The blocking drain of an estate cancelled before it ran anything: empty, and said so.
fn blocking_drain(mismatch: &mut Option<String>) -> Result<(), SmokeError> {
    let mut estate = Estate::supervised("acme", 8)?;
    estate.cancel();
    let report = estate.shutdown_blocking().map_err(std::io::Error::other)?;
    say(&format!(
        "blocking drain: completed={} admitted={}",
        report.supervised().completed,
        report.runs_admitted(),
    ))?;
    if report.supervised().completed != 0 || report.runs_admitted() != 0 {
        record(mismatch, "the empty cancel drain is not empty".to_owned());
    }
    Ok(())
}

/// The in-process tier: one blocking run, its drain, and an estate dropped inside the driver.
fn in_process_tier(
    driver: &Handle,
    probe: &Probe,
    mismatch: &mut Option<String>,
) -> Result<(), SmokeError> {
    let mut bare = Estate::in_process("acme")?;
    say(&format!(
        "in-process: runtime={} supervisor={}",
        bare.handle().is_some(),
        bare.supervisor().is_some(),
    ))?;
    let run = bare.run_blocking(probe, 6).map_err(std::io::Error::other)?;
    check_blocking_run("in-process", &run, 7, mismatch)?;
    let report = bare.shutdown_blocking().map_err(std::io::Error::other)?;
    say(&format!(
        "in-process drain: tier={} admitted={}",
        report.tier(),
        report.runs_admitted(),
    ))?;
    one_run_admitted("in-process", &report, mismatch);
    // Dropping a runtime inside async is the panic tokio raises; the estate owns none, so reaching
    // the next line proves dropping it there releases its work and leaves the driver running.
    driver.block_on(async move { drop(bare) });
    say("in-process drop inside a runtime: ok")?;
    Ok(())
}

/// The durable tier over the scratch directory: one blocking run and the drain that accounts for it.
fn durable_tier(
    driver: &Handle,
    probe: &Probe,
    dir: &std::path::Path,
    mismatch: &mut Option<String>,
) -> Result<(), SmokeError> {
    let durable = Estate::durable("acme", 8, dir)?;
    say(&format!("durable: store={}", durable.run_store().is_some()))?;
    let run = durable
        .run_blocking(probe, 7)
        .map_err(std::io::Error::other)?;
    check_blocking_run("durable", &run, 8, mismatch)?;
    let report = driver.block_on(async { durable.shutdown().await });
    say(&format!(
        "durable drain: admitted={}",
        report.runs_admitted()
    ))?;
    one_run_admitted("durable", &report, mismatch);
    Ok(())
}

/// Every typed refusal of a constructor, then the run events and the unopenable store.
fn refusals(dir: &std::path::Path, mismatch: &mut Option<String>) -> Result<(), SmokeError> {
    match Estate::supervised("", 8) {
        Err(EstateError::InvalidTenant { .. }) => say("refused: an empty tenant")?,
        Err(error) => record(
            mismatch,
            format!("an empty tenant is InvalidTenant, not {error}"),
        ),
        Ok(_) => record(mismatch, "an empty tenant must not build".to_owned()),
    }
    match Estate::supervised("acme", 0) {
        Err(EstateError::Host { .. }) => say("refused: a zero bound")?,
        Err(error) => record(mismatch, format!("a zero bound is Host, not {error}")),
        Ok(_) => record(mismatch, "a zero bound must not build".to_owned()),
    }
    match Estate::supervised("acme", usize::MAX) {
        Err(EstateError::Host { .. }) => say("refused: a bound past the ceiling")?,
        Err(error) => record(
            mismatch,
            format!("a bound past the ceiling is Host, not {error}"),
        ),
        Ok(_) => record(
            mismatch,
            "a bound past the ceiling must not build".to_owned(),
        ),
    }
    match run_events::run(dir) {
        Ok(line) => say(&line)?,
        Err(error) => record(mismatch, format!("run events: {error}")),
    }

    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"a file, not a directory")?;
    match Estate::durable("acme", 8, &blocker) {
        Err(EstateError::Store { .. }) => say("refused: an unopenable store")?,
        Err(error) => record(
            mismatch,
            format!("an unopenable store is Store, not {error}"),
        ),
        Ok(_) => record(mismatch, "an unopenable store must not build".to_owned()),
    }
    Ok(())
}

/// The #703 owner pattern on the shipped artifact: place probe tasks on the
/// estate's own supervisor and reconcile the tracked set through the same
/// accessors an owner reads. Nothing here depends on scheduler timing: the
/// tracked set changes only on this thread (spawn, reap, wait_idle join),
/// so a snapshot and the accessors read beside it always agree, and
/// `wait_idle` ends every placed body before the drain counts its reports.
/// Every number below is asserted, so a lost report fails the smoke rather
/// than parking an owner.
fn tracked_tasks(driver: &Handle, mismatch: &mut Option<String>) -> Result<(), SmokeError> {
    let mut tracked_estate = Estate::supervised("acme", 8)?;
    let mut seen: Vec<TaskId> = Vec::new();
    let tracking = driver.block_on(async {
        let supervisor = tracked_estate.supervisor()?;
        for _ in 0..3 {
            supervisor.spawn(|_| async {}).await;
        }
        // One state read three ways, with no entry point between the reads
        // that could reap: a listed task is tracked, always.
        let snapshot = supervisor.snapshot();
        if supervisor.in_flight()
            != snapshot.live().len().saturating_add(snapshot.live_truncated())
        {
            record(
                mismatch,
                format!(
                    "the tracking accessors disagree with the snapshot: in_flight={} listed={} truncated={}",
                    supervisor.in_flight(),
                    snapshot.live().len(),
                    snapshot.live_truncated(),
                ),
            );
        }
        for live in snapshot.live() {
            seen.push(live.task);
            if supervisor.task_state(live.task).is_none() {
                record(
                    mismatch,
                    format!("a listed task has no state: {}", live.task.get()),
                );
            }
        }
        // Every placed body ends here, never on a timer: `wait_idle` joins
        // what the spawns' own reaps have not already joined, and the roomy
        // bound keeps every report. Totals come from the supervisor's own
        // counters, so a body absorbed early still counts exactly once.
        supervisor.wait_idle().await;
        supervisor.reap();
        let mut drained = 0usize;
        while supervisor.next_report().is_some() {
            drained = drained.saturating_add(1);
        }
        let stats = supervisor.stats();
        Some((stats.spawned, stats.completed, drained, supervisor.in_flight()))
    });
    let Some((spawned, completed, drained, live_now)) = tracking else {
        record(
            mismatch,
            "the supervised tier has no supervisor to track".to_owned(),
        );
        return Ok(());
    };
    say(&format!(
        "tracking: placed=3 spawned={spawned} completed={completed} drained={drained} live_now={live_now}"
    ))?;
    if spawned != 3 || completed != 3 {
        record(
            mismatch,
            format!(
                "three placed tasks did not end exactly once: spawned={spawned} completed={completed}"
            ),
        );
    }
    if drained != 3 {
        record(
            mismatch,
            format!("three placed probe tasks drained {drained} reports, not 3"),
        );
    }
    if live_now != 0 {
        record(mismatch, format!("the drain left {live_now} tasks tracked"));
    }
    // Joins are consumptive: every task the snapshot listed above left the
    // set through a join, so each has a report and no state.
    if let Some(supervisor) = tracked_estate.supervisor() {
        for task in &seen {
            if supervisor.task_state(*task).is_some() {
                record(
                    mismatch,
                    format!("a joined task still has a state: {}", task.get()),
                );
            }
        }
    }
    Ok(())
}

/// Live revocation on the shipped artifact (#716): a firing bot is narrowed
/// mid-run through `replace_grants`, and its very next tick is denied
/// naming the withdrawn capability. Every number below is asserted, so a
/// grant replacement that does not take effect fails the smoke.
fn grant_replacement(mismatch: &mut Option<String>) -> Result<(), SmokeError> {
    let fired = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let mut bot = Bot::builder("smoke-replace")
        .observe(SmokeSource::gated())
        .on(
            |value: &u16| *value >= 500,
            SmokeAction {
                fired: std::rc::Rc::clone(&fired),
            },
        )
        .with_effects(EffectScope::ephemeral().map_err(std::io::Error::other)?)
        .build(&GrantSet::empty().grant(Cap::net()))
        .map_err(std::io::Error::other)?;
    match bot.tick() {
        Ok(1) => say("grant replacement: the bot fired once under its grant")?,
        other => record(
            mismatch,
            format!("the smoke bot did not fire once under its grant: {other:?}"),
        ),
    }
    let report: GrantReplacement = bot.replace_grants(&GrantSet::empty());
    say(&format!(
        "grant replacement: withdrawn={:?} added={:?}",
        report.withdrawn(),
        report.added()
    ))?;
    if report.withdrawn().to_vec() != vec![Cap::net()] || !report.added().is_empty() {
        record(
            mismatch,
            format!("the swap did not report exactly the withdrawn net grant: {report:?}"),
        );
    }
    // Read as the app layer's error: a consumer holding `BotError` still sees the
    // kernel's refusal, tagged with the layer that carried it.
    // The operator reads the certainty beside the error: whether a retry of this
    // run could double an effect is the fact the failure carries, not its text.
    match bot.tick().map_err(BotError::from) {
        Err(refused @ BotError::World(EcsError::Kernel(KernelError::CapabilityDenied { .. }))) => {
            say(&format!(
                "grant replacement: the next tick is denied: dispatch={:?} retry={:?}",
                refused.dispatch_certainty(),
                refused.retry_class(),
            ))?;
        }
        other => record(
            mismatch,
            format!("the tick after the swap must be denied, got {other:?}"),
        ),
    }
    if fired.get() != 1 {
        record(
            mismatch,
            format!("the effect ran {} times, not once", fired.get()),
        );
    }
    Ok(())
}

/// The whole estate smoke: every tier, every refusal and every door, then the first mismatch.
fn evening() -> Result<(), SmokeError> {
    let mut mismatch: Option<String> = None;
    let estate = Estate::supervised("acme", 8)?;
    // The process driver (ADR 0006 R2): every block below runs on it, so the binary that proves
    // the estate holds one runtime, as the estate does.
    let driver = estate
        .handle()
        .ok_or("a supervised estate carries the process driver")?;
    let body: ProbeBody = probe_body;
    let probe: Probe = task("estate-probe", body).map_err(std::io::Error::other)?;
    supervised_tier(estate, &driver, &probe, &mut mismatch)?;
    blocking_drain(&mut mismatch)?;
    in_process_tier(&driver, &probe, &mut mismatch)?;

    let dir = std::env::temp_dir().join(format!("lgwks-estate-smoke-{}", unique_hex()?));
    let _scratch = Scratch(dir.clone());
    durable_tier(&driver, &probe, &dir, &mut mismatch)?;
    refusals(&dir, &mut mismatch)?;
    tracked_tasks(&driver, &mut mismatch)?;
    grant_replacement(&mut mismatch)?;

    // The supervisor's process doors on the shipped artifact (#689, #690,
    // #691): a gated lane runs only after its identity is recorded, and the
    // deny builders refuse what they cannot confine. The smoke proves the
    // assembly it ships, and these are doors of the supervisor the assembly
    // owns: a lane that ran before its record existed, or a denial the
    // builders accepted but no platform could hold, fails here rather than in
    // a coordinator.
    #[cfg(all(unix, feature = "process"))]
    driver.block_on(process_doors::run(&mut mismatch))?;

    // The script vocabulary on the shipped artifact (#649, #650, #651, #653,
    // #654, #657, #694): the runtime the words desugar into, exercised through
    // the public interface with every refusal asserted.
    match vocabulary::run(&driver) {
        Ok(()) => say("vocabulary: every runtime answered through the public interface")?,
        Err(error) => record(&mut mismatch, format!("the vocabulary failed: {error}")),
    }

    // The library doors with no front of their own, on the shipped artifact (#990): each is called
    // as a downstream caller calls it, and every outcome is asserted.
    for door in doors::run(&dir) {
        match door {
            Ok(line) => say(&line)?,
            Err(problem) => record(&mut mismatch, problem.to_string()),
        }
    }

    if let Some(mismatch) = mismatch {
        lgwks_std::trace::debug!(
            mismatch = mismatch.as_str(),
            "estate-smoke: the evening proved nothing"
        );
        return Err(mismatch.into());
    }
    Ok(())
}

/// Run one named sub-command, or the whole estate smoke when none is named.
fn main() -> Result<(), SmokeError> {
    match subcommand() {
        Some(done) => done,
        None => evening(),
    }
}
