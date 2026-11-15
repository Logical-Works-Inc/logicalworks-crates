//! Seeded races of `Host::resume` on one decided run (#1055, INV-BOT-196).
//!
//! A run parked on an approval and then decided is resumed by a caller that
//! clicks twice, by a hundred and by a thousand. Before the host claimed a run
//! for the length of a resume, every racer found the steps after the approval
//! unrecorded, ran their bodies and only then recorded them, so a body ran once
//! per click. The store, the hosts, the threads and the decisions are real; the
//! seed draws the steps, the payloads, the cut and the fan-in. Each declared
//! test sweeps its band twice through [`sweep_twice`], so the same seed reaching
//! another trace hash fails even when every assertion passes.
//!
//! | Fool | Test | Bug it would catch |
//! |---|---|---|
//! | double-clicker | `sim_resume_race_double_clicker_*` | a body run once per racer |
//! | neighbour | `sim_resume_race_neighbour_*` | one tenant's resume refusing or running another's |
//! | pulled-plug | `sim_resume_race_pulled_plug_*` | a claim a dropped future never gives back |
//! | monkey | `sim_resume_race_saturation_*` | a claim table that grows past its bound |
//!
//! Every test ends with the saboteur: no permit and no live run left behind.

#![cfg(all(feature = "script", feature = "ephemeral", feature = "time"))]

use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::task::Poll;
use std::time::Duration;

use lgwks_bot::effect::RunId;
use lgwks_bot::script::{ApprovalDecision, Clock, FlowError, Scope, remember};
use lgwks_bot::task::{Disposition, Host, Report, Task, task};

use crate::approval_error::ApprovalError as ApprovalSimError;
use crate::sim::{self, Sim};
use crate::sim_approval_extra::{
    TestResult, host_on, parked_id_of, run_of, scratch, ship, sweep_twice,
};

/// The task every run here is recorded under.
const TASK: &str = "settle";

/// One run of `settle`, boxed so a task over it has a type a helper names.
type Settling = Pin<Box<dyn Future<Output = Result<Vec<u8>, FlowError>>>>;

/// The `settle` task, named so fixtures can hand one back.
type SettleTask = Task<fn(Scope, Job) -> Settling>;

/// A resume still in progress, held by a test that polls it by hand.
type Resuming<'host> = Pin<Box<dyn Future<Output = Report<Vec<u8>>> + 'host>>;

/// How many times each step's body ran, one counter per step.
struct Counts(Vec<AtomicU64>);

impl Counts {
    /// `steps` counters, all zero.
    fn new(steps: usize) -> Self {
        Self((0..steps).map(|_| AtomicU64::new(0)).collect())
    }

    /// One more run of step `index`'s body.
    fn bump(&self, index: usize) {
        if let Some(slot) = self.0.get(index) {
            slot.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// How often step `index`'s body ran; a step the job lacks never ran.
    fn get(&self, index: usize) -> u64 {
        match self.0.get(index) {
            Some(slot) => slot.load(Ordering::SeqCst),
            None => 0,
        }
    }

    /// Every counter, in step order.
    fn all(&self) -> Vec<u64> {
        self.0
            .iter()
            .map(|slot| slot.load(Ordering::SeqCst))
            .collect()
    }
}

/// What one `settle` run is given.
#[derive(Clone)]
struct Job {
    /// The bytes parked for approval and given back.
    payload: Vec<u8>,
    /// How many durable steps follow the approval.
    steps: usize,
    /// How often each step's body ran, across every run of this job.
    counts: Arc<Counts>,
    /// The step whose body never finishes, when a test cuts the run there.
    hold: Option<usize>,
}

impl Job {
    /// A job of `steps` steps over `payload`, with its own counters.
    fn new(payload: Vec<u8>, steps: usize) -> Self {
        Self {
            payload,
            steps,
            counts: Arc::new(Counts::new(steps)),
            hold: None,
        }
    }

    /// The same job, with step `index`'s body never finishing.
    fn holding(&self, index: usize) -> Self {
        Self {
            hold: Some(index),
            ..self.clone()
        }
    }

    /// What a finished run gives back, from the model alone: the approved
    /// payload, then each step's value, which is its position plus one.
    fn expected(&self) -> Result<Vec<u8>, ApprovalSimError> {
        let mut output = self.payload.clone();
        for index in 0..self.steps {
            output.push(u8::try_from(index.saturating_add(1)).map_err(|error| {
                lgwks_std::trace::debug!(error = %error, "resume race: a step value fits a byte");
                ApprovalSimError::Setup {
                    what: "a step value fits a byte",
                }
            })?);
        }
        Ok(output)
    }
}

/// Park the payload, then run each durable step after the approval.
async fn settle(scope: Scope, job: Job) -> Result<Vec<u8>, FlowError> {
    let mut output = ship(&scope, job.payload.clone()).await?;
    for index in 0..job.steps {
        let value = remember(&scope, &format!("step-{index}"), || async {
            job.counts.bump(index);
            if job.hold == Some(index) {
                // The pulled plug: the body is entered and never returns, so the
                // caller's drop is the only way out.
                std::future::pending::<()>().await;
            }
            u32::try_from(index.saturating_add(1)).map_err(FlowError::failed)
        })
        .await?;
        output.push(u8::try_from(value).map_err(FlowError::failed)?);
    }
    Ok(output)
}

/// One run of `settle`.
fn settle_body(scope: Scope, job: Job) -> Settling {
    Box::pin(settle(scope, job))
}

/// The `settle` flow as a task a host runs and resumes.
fn settling() -> Result<SettleTask, ApprovalSimError> {
    let body: fn(Scope, Job) -> Settling = settle_body;
    Ok(task(TASK, body)?)
}

/// Park `job` for a human decision, as a fresh run or as an adoption of `adopt`,
/// and return its run, id and digest. No step body has run.
fn park_settle(
    host: &Host,
    adopt: Option<RunId>,
    job: &Job,
) -> Result<(RunId, String, lgwks_std::hash::Digest), ApprovalSimError> {
    lgwks_std::trace::debug!("resume race: parking one job for a decision");
    let settling = settling()?;
    let report = match adopt {
        Some(run) => lgwks_bot::block_on(host.resume(run, &settling, job.clone())),
        None => lgwks_bot::block_on(host.run(&settling, job.clone())),
    };
    if !matches!(report.disposition(), Disposition::Parked) {
        return Err(ApprovalSimError::Setup {
            what: "a first run parks",
        });
    }
    // The id and digest come from the park's own report, not from `pending`: its
    // scan is capped at the pending ceiling and counts decided records, so a
    // host with a low ceiling stops listing parks after that many decisions.
    let id = parked_id_of(&report)?;
    let run = run_of(&report)?;
    let parked = report.error().ok_or(ApprovalSimError::Setup {
        what: "a parked run names its digest",
    })?;
    let FlowError::Parked { ref digest, .. } = *parked else {
        return Err(ApprovalSimError::Setup {
            what: "a parked run names its digest",
        });
    };
    let digest = lgwks_std::hash::Digest::from_hex(digest).map_err(|error| {
        lgwks_std::trace::debug!(error = %error, "resume race: the parked digest parses");
        ApprovalSimError::Setup {
            what: "the parked digest parses",
        }
    })?;
    require(
        job.counts.all().iter().all(|&ran| ran == 0),
        "no step body runs before the decision",
    )?;
    Ok((run, id, digest))
}

/// Park `job` and approve it, so every resume after this finds a decided run.
fn park_decided(host: &Host, adopt: Option<RunId>, job: &Job) -> Result<RunId, ApprovalSimError> {
    let (run, id, digest) = park_settle(host, adopt, job)?;
    host.decide(&id, ApprovalDecision::approve("sim", digest))?;
    Ok(run)
}

/// What one racer's report says, judged against the model's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Succeeded with exactly the model's output, as the winner or by replay.
    Done,
    /// Refused as `RunInFlight` for the run it asked for.
    Held,
    /// Anything else: a failure, a park, a refusal for another reason.
    Odd,
}

/// Judge `report` for a resume of `run` whose finished output is `expected`.
fn verdict(report: &Report<Vec<u8>>, run: RunId, expected: &[u8]) -> Verdict {
    match (report.disposition(), report.output(), report.error()) {
        (Disposition::Succeeded, Some(output), _) if output.as_slice() == expected => Verdict::Done,
        (Disposition::Refused, None, Some(error)) if matches!(*error, FlowError::RunInFlight { run: held } if held == run) => {
            Verdict::Held
        }
        _ => Verdict::Odd,
    }
}

/// How a lane's racers fared.
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    /// Racers that finished the run or replayed it.
    done: usize,
    /// Racers refused because the run was held.
    held: usize,
    /// Racers that ended any other way.
    odd: usize,
}

impl Tally {
    /// Add one racer's verdict.
    fn add(&mut self, verdict: Verdict) {
        match verdict {
            Verdict::Done => self.done = self.done.saturating_add(1),
            Verdict::Held => self.held = self.held.saturating_add(1),
            Verdict::Odd => self.odd = self.odd.saturating_add(1),
        }
    }

    /// The oracle every lane meets: nobody ended oddly, somebody finished, and
    /// every racer is accounted for.
    fn check(&self, racers: usize) -> TestResult {
        require(self.odd == 0, "every racer is Done or Held")?;
        require(self.done >= 1, "at least one racer finishes the run")?;
        require(
            self.done.saturating_add(self.held) == racers,
            "every racer is accounted for",
        )
    }
}

/// One tenant's resumers of one run.
struct Lane {
    /// The tenant's host, shared by its racers as a clone shares a handle.
    host: Arc<Host>,
    /// The run every racer resumes.
    run: RunId,
    /// What each racer is given.
    job: Job,
    /// What a finished run gives back.
    expected: Arc<[u8]>,
    /// How many racers resume at once.
    racers: usize,
}

impl Lane {
    /// A lane of `racers` over `run`, which `job` parked and decided on `host`.
    fn new(
        host: &Arc<Host>,
        run: RunId,
        job: &Job,
        racers: usize,
    ) -> Result<Self, ApprovalSimError> {
        Ok(Self {
            host: Arc::clone(host),
            run,
            job: job.clone(),
            expected: Arc::from(job.expected()?),
            racers,
        })
    }

    /// Resume the run once more, alone, and require the model's output.
    fn resume_alone(&self) -> TestResult {
        let settling = settling()?;
        let report = lgwks_bot::block_on(self.host.resume(self.run, &settling, self.job.clone()));
        require(
            verdict(&report, self.run, &self.expected) == Verdict::Done,
            "a resume after the race finishes with the model's output",
        )
    }

    /// Every step body ran exactly once.
    fn ran_once(&self) -> bool {
        self.job.counts.all().iter().all(|&ran| ran == 1)
    }
}

/// One racer thread, released by `barrier` with every other racer.
fn spawn_racer(
    lane: &Lane,
    barrier: &Arc<Barrier>,
    name: String,
) -> Result<std::thread::JoinHandle<Verdict>, ApprovalSimError> {
    let (host, job, expected, barrier, run) = (
        Arc::clone(&lane.host),
        lane.job.clone(),
        Arc::clone(&lane.expected),
        Arc::clone(barrier),
        lane.run,
    );
    std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            let Ok(settling) = settling() else {
                return Verdict::Odd;
            };
            barrier.wait();
            let report = lgwks_bot::block_on(host.resume(run, &settling, job));
            verdict(&report, run, &expected)
        })
        .map_err(|error| {
            lgwks_std::trace::debug!(error = %error, "resume race: a racer failed to spawn");
            ApprovalSimError::Setup {
                what: "a racer spawns",
            }
        })
}

/// Release every lane's racers together and tally how each lane fared. Every
/// thread is joined before this returns, so none outlives the test.
fn race(lanes: &[Lane]) -> Result<Vec<Tally>, ApprovalSimError> {
    let total: usize = lanes.iter().map(|lane| lane.racers).sum();
    let barrier = Arc::new(Barrier::new(total));
    let mut handles = Vec::with_capacity(total);
    for (lane_index, lane) in lanes.iter().enumerate() {
        for racer in 0..lane.racers {
            let name = format!("resume-race-{lane_index}-{racer}");
            handles.push((lane_index, spawn_racer(lane, &barrier, name)?));
        }
    }
    let mut tallies = vec![Tally::default(); lanes.len()];
    for (lane_index, handle) in handles {
        let verdict = handle.join().map_err(|error| {
            lgwks_std::trace::debug!(?error, "resume race: a racer joins");
            ApprovalSimError::Setup {
                what: "a racer joins",
            }
        })?;
        tallies
            .get_mut(lane_index)
            .ok_or(ApprovalSimError::Setup {
                what: "a racer has a lane",
            })?
            .add(verdict);
    }
    Ok(tallies)
}

/// Fail with `what` unless `holds`.
fn require(holds: bool, what: &'static str) -> TestResult {
    if holds {
        Ok(())
    } else {
        lgwks_std::trace::debug!(what, "resume race: an oracle failed");
        Err(ApprovalSimError::Setup { what })
    }
}

/// The saboteur: nothing is admitted, nothing holds a permit, no run is live.
fn nothing_left_behind(host: &Host) -> TestResult {
    let admission = host.admission();
    require(admission.in_flight() == 0, "no run is left in flight")?;
    require(
        admission.available_permits() == host.limits().max_concurrent_tasks(),
        "every permit is back",
    )
}

/// A draw below `bound`.
fn draw(sim: &mut Sim, bound: usize) -> Result<usize, ApprovalSimError> {
    let bound = u32::try_from(bound).map_err(|error| {
        lgwks_std::trace::debug!(error = %error, "resume race: a bound fits a draw");
        ApprovalSimError::Setup {
            what: "a bound fits a draw",
        }
    })?;
    usize::try_from(sim.rng().below(bound)).map_err(|error| {
        lgwks_std::trace::debug!(error = %error, "resume race: a draw fits its bound");
        ApprovalSimError::Setup {
            what: "a draw fits its bound",
        }
    })
}

/// A step count in `2..=5`, drawn from the seed.
fn draw_steps(sim: &mut Sim) -> Result<usize, ApprovalSimError> {
    Ok(draw(sim, 4)?.saturating_add(2))
}

/// The payload a seed gives one run: unique to the seed and the tag.
fn payload_for(sim: &mut Sim, tag: &str) -> Vec<u8> {
    let salt = sim.rng().below(1_000_000);
    format!("{tag}-{}-{salt}", sim.seed).into_bytes()
}

/// Poll `resuming` by hand until `reached` holds, which proves it entered the
/// place the test wants it parked in; `false` if it finished instead. No timer
/// and no sleep: the waker the host's own awaits registered is the only wake.
async fn poll_until(resuming: &mut Resuming<'_>, reached: impl Fn() -> bool) -> bool {
    std::future::poll_fn(|cx| match resuming.as_mut().poll(cx) {
        Poll::Ready(_) => Poll::Ready(false),
        Poll::Pending if reached() => Poll::Ready(true),
        Poll::Pending => Poll::Pending,
    })
    .await
}

/// Park one run, decide it, race `racers` resumes of it behind a barrier, then
/// resume it once more alone; every body must have run exactly once.
fn double_click(sim: &mut Sim, racers: usize) -> TestResult {
    let steps = draw_steps(sim)?;
    let scratch = scratch("resume-race-double-clicker")?;
    let host = Arc::new(host_on(scratch.path(), "acme")?);
    let job = Job::new(payload_for(sim, "click"), steps);
    let run = park_decided(&host, None, &job)?;
    let lane = Lane::new(&host, run, &job, racers)?;
    let tallies = race(std::slice::from_ref(&lane))?;
    tallies
        .first()
        .ok_or(ApprovalSimError::Setup {
            what: "the race has a lane",
        })?
        .check(racers)?;
    require(
        lane.ran_once(),
        "every step body ran exactly once across every racer",
    )?;
    lane.resume_alone()?;
    require(
        lane.ran_once(),
        "a resume after the race replays and runs no body",
    )?;
    nothing_left_behind(&host)?;
    // Only what no thread schedule can change: how many racers fell on which
    // side is the schedule's, so it stays out of the trace.
    sim.record(&format!(
        "double-click:racers={racers}:steps={steps}:each-once"
    ));
    Ok(())
}

/// A hundred and then a thousand racers over one decided run.
fn double_clicker(sim: &mut Sim) -> TestResult {
    for racers in [100, 1_000] {
        double_click(sim, racers)?;
    }
    Ok(())
}

/// Two tenants resume one run id at once. Each parks its own run under it.
fn neighbours(sim: &mut Sim) -> TestResult {
    let scratch = scratch("resume-race-neighbour")?;
    let acme = Arc::new(host_on(scratch.path(), "acme")?);
    let zed = Arc::new(host_on(scratch.path(), "zed")?);
    for round in 0..4usize {
        let steps = draw_steps(sim)?;
        let acme_job = Job::new(payload_for(sim, "acme"), steps);
        let zed_job = Job::new(payload_for(sim, "zed-with-a-longer-payload"), steps);
        let run = park_decided(&acme, None, &acme_job)?;
        require(
            park_decided(&zed, Some(run), &zed_job)? == run,
            "the second tenant parks under the first's run id",
        )?;
        // One racer each on the even rounds, so neither has a rival of its own
        // and a refusal could only come from the neighbour.
        let (acme_racers, zed_racers) = if round % 2 == 0 {
            (1, 1)
        } else {
            (
                draw(sim, 6)?.saturating_add(1),
                draw(sim, 6)?.saturating_add(1),
            )
        };
        let lanes = [
            Lane::new(&acme, run, &acme_job, acme_racers)?,
            Lane::new(&zed, run, &zed_job, zed_racers)?,
        ];
        let tallies = race(&lanes)?;
        for (lane, tally) in lanes.iter().zip(&tallies) {
            tally.check(lane.racers)?;
            if lane.racers == 1 {
                require(
                    tally.held == 0,
                    "a lone racer is never refused by a neighbour",
                )?;
            }
            require(lane.ran_once(), "each tenant's bodies ran exactly once")?;
            lane.resume_alone()?;
            require(lane.ran_once(), "each tenant's replay runs no body")?;
        }
        sim.record(&format!("neighbours:round={round}:steps={steps}:each-once"));
    }
    nothing_left_behind(&acme)?;
    nothing_left_behind(&zed)
}

/// The winner's future is dropped inside a body; a later resume claims the run.
fn pulled_plug(sim: &mut Sim) -> TestResult {
    let steps = draw_steps(sim)?;
    let cut = draw(sim, steps)?;
    let probes = draw(sim, 8)?.saturating_add(1);
    let scratch = scratch("resume-race-plug")?;
    let host = Arc::new(host_on(scratch.path(), "acme")?);
    let job = Job::new(payload_for(sim, "plug"), steps);
    let run = park_decided(&host, None, &job)?;
    let lane = Lane::new(&host, run, &job, 1)?;
    let report = lgwks_bot::block_on(cut_then_resume(&lane, cut, probes))?;
    require(
        verdict(&report, run, &lane.expected) == Verdict::Done,
        "the resume after the cut finishes the run",
    )?;
    for (index, ran) in job.counts.all().into_iter().enumerate() {
        // The cut body was entered by the dropped winner and again by the
        // resume: at-least-once for an unrecorded step. Every other body ran
        // once, before the cut because it was recorded and after it because
        // the winner never got there.
        let want = if index == cut { 2 } else { 1 };
        require(ran == want, "only the cut body runs again after the cut")?;
    }
    lane.resume_alone()?;
    require(
        job.counts.get(cut) == 2 && job.counts.all().iter().sum::<u64>() == steps_plus_one(steps)?,
        "a finished run replays every step, the cut one included",
    )?;
    nothing_left_behind(&host)?;
    sim.record(&format!("plug:steps={steps}:cut={cut}:probes={probes}"));
    Ok(())
}

/// The bodies a plugged run runs in all: one per step, and the cut one twice.
fn steps_plus_one(steps: usize) -> Result<u64, ApprovalSimError> {
    u64::try_from(steps.saturating_add(1)).map_err(|error| {
        lgwks_std::trace::debug!(error = %error, "resume race: the body count fits");
        ApprovalSimError::Setup {
            what: "the body count fits",
        }
    })
}

/// Park the winner inside step `cut`'s body, refuse `probes` rivals while it
/// holds the run, drop it, and resume the run to its end.
async fn cut_then_resume(
    lane: &Lane,
    cut: usize,
    probes: usize,
) -> Result<Report<Vec<u8>>, ApprovalSimError> {
    let (host, run, job) = (&lane.host, lane.run, &lane.job);
    let settling = settling()?;
    let mut winner: Resuming<'_> = Box::pin(host.resume(run, &settling, job.holding(cut)));
    require(
        poll_until(&mut winner, || job.counts.get(cut) > 0).await,
        "the winner is parked inside the cut body",
    )?;
    for _ in 0..probes {
        let rival = host.resume(run, &settling, job.clone()).await;
        require(
            verdict(&rival, run, &lane.expected) == Verdict::Held,
            "a rival of a live winner is Refused with RunInFlight",
        )?;
    }
    require(job.counts.get(cut) == 1, "a refused rival runs no body")?;
    drop(winner);
    nothing_left_behind(host)?;
    Ok(host.resume(run, &settling, job.clone()).await)
}

/// The admission ceiling of the saturation host.
const PERMITS: usize = 2;

/// The pending-approval ceiling of the saturation host.
const PENDING: usize = 2;

/// More distinct live runs than the claim ceiling: the rest are refused, typed,
/// and every run finishes once the live ones are given up.
fn saturation(sim: &mut Sim) -> TestResult {
    let ceiling = PERMITS.saturating_add(PENDING);
    let over = draw(sim, 3)?.saturating_add(1);
    let steps = draw_steps(sim)?;
    let scratch = scratch("resume-race-saturation")?;
    let host = Host::builder("acme")?
        .run_store(scratch.path())?
        .clock(Clock::virtual_at(Duration::ZERO))
        .max_concurrent_tasks(NonZeroUsize::MIN.saturating_add(PERMITS.saturating_sub(1)))
        .max_pending_approvals(PENDING)
        .build()?;
    let mut runs = Vec::new();
    for index in 0..ceiling.saturating_add(over) {
        let job = Job::new(payload_for(sim, &format!("sat{index}")), steps);
        runs.push((park_decided(&host, None, &job)?, job));
    }
    lgwks_bot::block_on(saturate(&host, &runs, ceiling, over))?;
    for (index, &(run, ref job)) in runs.iter().enumerate() {
        let settling = settling()?;
        let report = lgwks_bot::block_on(host.resume(run, &settling, job.clone()));
        require(
            verdict(&report, run, &job.expected()?) == Verdict::Done,
            "every run finishes once the live ones settle",
        )?;
        // A held run whose body was entered ran step 0 twice: once cut, once
        // finished. Every other body, refused runs included, ran once.
        let entered = index < PERMITS;
        for (step, ran) in job.counts.all().into_iter().enumerate() {
            let want = if entered && step == 0 { 2 } else { 1 };
            require(
                ran == want,
                "only a cut body runs twice, a refusal runs none",
            )?;
        }
    }
    nothing_left_behind(&host)?;
    sim.record(&format!(
        "saturation:over={over}:steps={steps}:refused-then-finished"
    ));
    Ok(())
}

/// Hold `ceiling` runs live, refuse everything past them, then give them up.
async fn saturate(host: &Host, runs: &[(RunId, Job)], ceiling: usize, over: usize) -> TestResult {
    let settling = settling()?;
    let mut live: Vec<Resuming<'_>> = Vec::with_capacity(ceiling);
    for (index, &(run, ref job)) in runs.iter().take(ceiling).enumerate() {
        let mut resuming: Resuming<'_> = Box::pin(host.resume(run, &settling, job.holding(0)));
        // The first `PERMITS` get a permit and park in their body. The rest are
        // polled once: their claim is taken before admission, so they hold it
        // while they wait for a permit that never comes.
        let enters_body = index < PERMITS;
        require(
            poll_until(&mut resuming, || !enters_body || job.counts.get(0) > 0).await,
            "a live run parks in its body or waits for a permit",
        )?;
        live.push(resuming);
    }
    let refused_before = host.admission().refused();
    // A rival of a held run is told the run is held, even with the table full.
    if let Some(&(run, ref job)) = runs.first() {
        let rival = host.resume(run, &settling, job.clone()).await;
        require(
            verdict(&rival, run, &job.expected()?) == Verdict::Held,
            "a rival of a held run is RunInFlight at the ceiling",
        )?;
    }
    for &(run, ref job) in runs.iter().skip(ceiling) {
        let report = host.resume(run, &settling, job.clone()).await;
        let saturated = matches!(
            report.error(),
            Some(error) if matches!(
                *error,
                FlowError::ResumesSaturated { ceiling: seen } if seen == ceiling
            )
        );
        require(
            matches!(report.disposition(), Disposition::Refused) && saturated,
            "a claim past the ceiling is Refused with ResumesSaturated",
        )?;
        require(
            job.counts.all().iter().all(|&ran| ran == 0),
            "a refused resume runs no body",
        )?;
    }
    let refusals = u64::try_from(over.saturating_add(1)).map_err(|error| {
        lgwks_std::trace::debug!(error = %error, "resume race: the refusal count fits");
        ApprovalSimError::Setup {
            what: "the refusal count fits",
        }
    })?;
    require(
        host.admission().refused() == refused_before.saturating_add(refusals),
        "every refusal is counted once",
    )?;
    drop(live);
    nothing_left_behind(host)
}

#[test]
fn sim_resume_race_double_clicker_n_resumes_of_one_decided_run_run_each_body_once() -> TestResult {
    sweep_twice(sim::band_of(40), double_clicker)
}

#[test]
fn sim_resume_race_neighbour_two_tenants_racing_one_run_id_never_block_each_other() -> TestResult {
    sweep_twice(sim::band_of(41), neighbours)
}

#[test]
fn sim_resume_race_pulled_plug_a_dropped_winner_releases_its_claim() -> TestResult {
    sweep_twice(sim::band_of(42), pulled_plug)
}

#[test]
fn sim_resume_race_saturation_claims_past_the_ceiling_are_refused_and_recover() -> TestResult {
    sweep_twice(sim::band_of(43), saturation)
}
