//! Seeded simulations of the public wait, `Clock::sleep`, against the real `Clock`.
//!
//! `Clock::sleep` is public because a wait outside any step (the durable bench's fixed-latency
//! upstreams and a human's decision delay) must be made on a declared clock: the live process
//! passes the wall clock and a simulation passes a virtual one and moves it. These sims hold the
//! virtual half of that contract. Nothing here waits on a timer or a delay: a sleep is polled by
//! hand with a counting waker, the way an executor polls it, so a seed names one run exactly.
//!
//! - the clock-fool: a sleep never completes before `advance` reaches its deadline, however the
//!   advances undershoot it, and completes on the advance that lands exactly on it;
//! - the neighbour: a thousand sleeps on one clock complete exactly when their own deadline has
//!   passed, checked against a sorted list of the deadlines;
//! - the giant and the empty: a sleep of zero completes at once, a sleep of `Duration::MAX` never
//!   does, and the clock saturates at its ceiling without wrapping;
//! - the double-clicker: a sleep parked on another thread ends on the advance and not before.

#![cfg(all(feature = "rt", feature = "time"))]

use crate::sim::seed::{Rng, Trace};

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Wake, Waker};
use std::time::Duration;

use lgwks_bot::clock::{Clock, ClockError};

/// A scenario's result: the message names what broke.
type SimResult<T> = Result<T, String>;

/// A test's result.
type TestResult = Result<(), String>;

/// Seeds one in-process family sweeps.
const SEEDS: u64 = 64;

/// Seeds the threaded family sweeps: each owns a thread.
const THREAD_SEEDS: u64 = 8;

/// Sleeps the neighbour puts on one clock.
const CROWD: usize = 1_000;

/// `Ok` when `held`, else the message `what` builds.
fn require(held: bool, what: impl FnOnce() -> String) -> SimResult<()> {
    if held { Ok(()) } else { Err(what()) }
}

/// A waker that counts its wakes, so a test can tell a sleep the clock woke from one it left parked.
struct Wakes(AtomicUsize);

impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// One `Clock::sleep`, polled by hand.
struct Sleeper<'clock> {
    /// The sleep.
    future: Pin<Box<dyn Future<Output = ()> + 'clock>>,
    /// How often the clock woke it.
    wakes: Arc<Wakes>,
    /// The waker handed to every poll.
    waker: Waker,
    /// Whether a poll has returned ready.
    done: bool,
}

impl<'clock> Sleeper<'clock> {
    /// A sleep of `duration` on `clock`, not yet polled.
    fn new(clock: &'clock Clock, duration: Duration) -> Self {
        let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
        Self {
            future: Box::pin(clock.sleep(duration)),
            waker: Waker::from(Arc::clone(&wakes)),
            wakes,
            done: false,
        }
    }

    /// Poll once if it has not finished; whether it has finished.
    fn poll(&mut self) -> bool {
        if !self.done {
            let mut context = Context::from_waker(&self.waker);
            self.done = self.future.as_mut().poll(&mut context).is_ready();
        }
        self.done
    }

    /// How often the clock has woken it.
    fn woken(&self) -> usize {
        self.wakes.0.load(Ordering::Relaxed)
    }
}

/// Move `clock` forward by `by`, the error as a message.
fn advance(clock: &Clock, by: Duration) -> SimResult<Duration> {
    clock
        .advance(by)
        .map_err(|error| format!("advance by {by:?}: {error}"))
}

/// Run `scenario` twice on each of `seeds` seeds: both runs must leave one trace hash.
fn replays(scenario: fn(u64) -> SimResult<u64>, seeds: u64) -> TestResult {
    for seed in 0..seeds {
        let (first, second) = (scenario(seed)?, scenario(seed)?);
        require(first == second, || {
            format!("seed {seed} replayed differently: {first:x} then {second:x}")
        })?;
    }
    Ok(())
}

/// One advance that must leave the sleep parked: the clock lands where the model says, the sleep
/// was not woken and a poll finds it pending.
fn hold(clock: &Clock, sleeper: &mut Sleeper<'_>, by: u64, want: u64, seed: u64) -> SimResult<()> {
    let landed = advance(clock, Duration::from_nanos(by))?;
    require(
        landed == Duration::from_nanos(want) && clock.now() == landed,
        || format!("seed {seed}: advanced to {landed:?}, the model says {want} ns"),
    )?;
    require(sleeper.woken() == 0, || {
        format!("seed {seed}: the sleep was woken at {want} ns, before its deadline")
    })?;
    let early = sleeper.poll();
    require(!early, || {
        format!("seed {seed}: the sleep completed at {want} ns, before its deadline")
    })
}

/// A sleep of N nanoseconds, advances that undershoot its deadline by one nanosecond in all,
/// then the advance that lands on it.
fn fool(seed: u64) -> SimResult<u64> {
    let mut rng = Rng::new(seed ^ 0xf001_f001_f001_f001);
    let mut trace = Trace::new();
    let origin = u64::from(rng.below(1_000_000));
    let clock = Clock::virtual_at(Duration::from_nanos(origin));
    let span = u64::from(rng.between(2, 1_000_000));
    let mut sleeper = Sleeper::new(&clock, Duration::from_nanos(span));
    let first = sleeper.poll();
    require(!first, || {
        format!("seed {seed}: a sleep of {span} ns completed before any advance")
    })?;
    let mut left = span.saturating_sub(1);
    let mut at = origin;
    for _ in 0..32 {
        if left == 0 {
            break;
        }
        let most = u32::try_from(left).map_err(|error| error.to_string())?;
        let step = u64::from(rng.between(1, most));
        left = left.saturating_sub(step);
        at = at.saturating_add(step);
        hold(&clock, &mut sleeper, step, at, seed)?;
        trace.record_number("undershoot", at);
    }
    if left > 0 {
        at = at.saturating_add(left);
        hold(&clock, &mut sleeper, left, at, seed)?;
    }
    let deadline = origin.saturating_add(span);
    require(at == deadline.saturating_sub(1), || {
        format!("seed {seed}: the undershoots reached {at}, one short of {deadline} was meant")
    })?;
    let landed = advance(&clock, Duration::from_nanos(1))?;
    require(landed == Duration::from_nanos(deadline), || {
        format!("seed {seed}: the landing advance reached {landed:?}, not {deadline} ns")
    })?;
    require(sleeper.woken() >= 1, || {
        format!("seed {seed}: the advance onto the deadline did not wake the sleep")
    })?;
    let done = sleeper.poll();
    require(done, || {
        format!("seed {seed}: the sleep is pending at its deadline")
    })?;
    advance(&clock, Duration::from_nanos(u64::from(rng.below(1_000))))?;
    require(sleeper.poll(), || {
        format!("seed {seed}: a finished sleep went pending again")
    })?;
    trace.record_number("deadline", deadline);
    Ok(trace.hash())
}

#[test]
fn sim_clock_fool_a_virtual_sleep_never_completes_before_its_deadline() -> TestResult {
    replays(fool, SEEDS)
}

/// A thousand sleeps with seeded deadlines on one clock. After every advance the finished set is
/// exactly the sleeps whose deadline has passed: the reference is a sorted list of the deadlines.
fn crowd(seed: u64) -> SimResult<u64> {
    let mut rng = Rng::new(seed ^ 0x0c20_3d0c_203d_0c20);
    let mut trace = Trace::new();
    let clock = Clock::virtual_at(Duration::ZERO);
    let deadlines: Vec<u64> = (0..CROWD)
        .map(|_| u64::from(rng.between(1, 50_000)))
        .collect();
    let mut sleepers: Vec<Sleeper<'_>> = deadlines
        .iter()
        .map(|deadline| Sleeper::new(&clock, Duration::from_nanos(*deadline)))
        .collect();
    for sleeper in &mut sleepers {
        let early = sleeper.poll();
        require(!early, || {
            format!("seed {seed}: a sleep completed before any advance")
        })?;
    }
    let mut sorted = deadlines.clone();
    sorted.sort_unstable();
    let top = sorted.last().copied().ok_or("the crowd is empty")?;
    let (mut now, mut finished_before) = (0_u64, 0_usize);
    while now < top {
        let step = u64::from(rng.between(1, 2_000));
        let landed = advance(&clock, Duration::from_nanos(step))?;
        now = now.saturating_add(step);
        require(landed == Duration::from_nanos(now), || {
            format!("seed {seed}: the clock is at {landed:?}, the model says {now} ns")
        })?;
        for (sleeper, deadline) in sleepers.iter_mut().zip(&deadlines) {
            let due = *deadline <= now;
            require((sleeper.woken() > 0) == due, || {
                format!(
                    "seed {seed}: at {now} ns a sleep due at {deadline} ns has woken {} times",
                    sleeper.woken()
                )
            })?;
            if sleeper.woken() > 0 {
                sleeper.poll();
            }
            require(sleeper.done == due, || {
                format!(
                    "seed {seed}: at {now} ns a sleep due at {deadline} ns is done: {}",
                    sleeper.done
                )
            })?;
        }
        let finished = sleepers.iter().filter(|sleeper| sleeper.done).count();
        let model = sorted.partition_point(|deadline| *deadline <= now);
        require(finished == model, || {
            format!("seed {seed}: at {now} ns {finished} finished, the model says {model}")
        })?;
        require(finished >= finished_before, || {
            format!("seed {seed}: the finished set shrank from {finished_before} to {finished}")
        })?;
        finished_before = finished;
        trace.record_number("finished", finished);
    }
    require(finished_before == CROWD, || {
        format!("seed {seed}: {finished_before} of {CROWD} finished once the last deadline passed")
    })?;
    Ok(trace.hash())
}

#[test]
fn sim_clock_neighbour_a_thousand_sleeps_finish_exactly_when_due() -> TestResult {
    replays(crowd, SEEDS)
}

/// A sleep of zero and a sleep of `Duration::MAX` on a clock driven by seeded advances, from a few
/// nanoseconds to `Duration::MAX`. The clock must land where the saturating model says and never
/// wrap or pass its ceiling; the zero sleep finishes at once; the `MAX` sleep never does.
fn extremes(seed: u64) -> SimResult<u64> {
    let mut rng = Rng::new(seed ^ 0xe47e_0e47_e0e4_7e00);
    let mut trace = Trace::new();
    let origin = Duration::from_secs(u64::from(rng.below(1_000_000)));
    let clock = Clock::virtual_at(origin);
    let mut empty = Sleeper::new(&clock, Duration::ZERO);
    let ready = empty.poll();
    require(ready, || {
        format!("seed {seed}: a sleep of zero was pending at once")
    })?;
    require(clock.now() == origin, || {
        format!("seed {seed}: a sleep of zero moved the clock")
    })?;
    let mut giant = Sleeper::new(&clock, Duration::MAX);
    let early = giant.poll();
    require(!early, || {
        format!("seed {seed}: a sleep of Duration::MAX completed at once")
    })?;
    let ceiling = Clock::elapsed_ceiling();
    let mut previous = clock.now();
    for _ in 0..8 {
        let by = match rng.below(4) {
            0 => Duration::MAX,
            1 => Duration::from_secs(u64::from(rng.below(u32::MAX))),
            2 => Duration::from_nanos(u64::from(rng.below(1_000))),
            _ => Duration::ZERO,
        };
        let want = previous.saturating_add(by).min(ceiling);
        match clock.advance(by) {
            Ok(landed) => {
                require(previous < ceiling && landed == want, || {
                    format!(
                        "seed {seed}: advance by {by:?} from {previous:?} landed on {landed:?}, the model says {want:?}"
                    )
                })?;
            }
            Err(ClockError::OutOfRange { .. }) => {
                require(previous >= ceiling, || {
                    format!(
                        "seed {seed}: advance by {by:?} was refused short of the ceiling, at {previous:?}"
                    )
                })?;
            }
            Err(other) => require(false, || format!("seed {seed}: advance refused as {other}"))?,
        }
        let now = clock.now();
        require(now >= previous && now <= ceiling, || {
            format!("seed {seed}: the clock went from {previous:?} to {now:?}, ceiling {ceiling:?}")
        })?;
        previous = now;
        require(giant.woken() == 0, || {
            format!("seed {seed}: the sleep of Duration::MAX was woken at {now:?}")
        })?;
        let finished = giant.poll();
        require(!finished, || {
            format!("seed {seed}: the sleep of Duration::MAX finished at {now:?}")
        })?;
        trace.record_number("now_ns", now.as_nanos());
    }
    let last = clock.advance(Duration::MAX);
    require(
        matches!(&last, Ok(landed) if *landed == ceiling)
            || matches!(&last, Err(ClockError::OutOfRange { .. }) if previous >= ceiling),
        || format!("seed {seed}: the last advance to the ceiling gave {last:?}"),
    )?;
    require(clock.now() == ceiling, || {
        format!(
            "seed {seed}: the clock is at {:?}, not its ceiling",
            clock.now()
        )
    })?;
    let past = clock.advance(Duration::from_nanos(1));
    require(matches!(&past, Err(ClockError::OutOfRange { .. })), || {
        format!("seed {seed}: an advance past the ceiling gave {past:?}")
    })?;
    require(clock.now() == ceiling && !giant.poll(), || {
        format!("seed {seed}: the ceiling moved, or the sleep of Duration::MAX finished")
    })?;
    let mut again = Sleeper::new(&clock, Duration::ZERO);
    require(again.poll(), || {
        format!("seed {seed}: a sleep of zero at the ceiling was pending")
    })?;
    lgwks_bot::rt::runtime::block_on(Clock::wall().sleep(Duration::ZERO));
    Ok(trace.hash())
}

#[test]
fn sim_clock_giant_and_empty_zero_ends_at_once_and_max_never_and_the_ceiling_holds() -> TestResult {
    replays(extremes, SEEDS)
}

/// The parked half of a sleep on another thread: it has not ended before the advance, nor after an
/// advance that falls short, and it ends once an advance reaches its deadline.
fn hold_until_deadline(clock: &Clock, span: Duration, finished: &AtomicBool) -> SimResult<()> {
    lgwks_bot::rt::runtime::block_on(clock.wait_for_waiter());
    require(!finished.load(Ordering::Acquire), || {
        "a sleep nobody advanced to its deadline completed".to_owned()
    })?;
    let half = span
        .checked_div(2)
        .ok_or_else(|| format!("{span:?} does not halve"))?;
    advance(clock, half)?;
    require(!finished.load(Ordering::Acquire), || {
        format!("half of {span:?} completed the sleep")
    })?;
    advance(clock, span.saturating_sub(half))?;
    Ok(())
}

/// A sleep parked on a second thread, as the bench's workers park theirs.
fn double_click(seed: u64) -> SimResult<u64> {
    let mut rng = Rng::new(seed ^ 0xd0b1_ec11_cd0b_1ec1);
    let mut trace = Trace::new();
    let span = Duration::from_secs(u64::from(rng.between(2, 100_000)));
    let clock = Clock::virtual_at(Duration::ZERO);
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let sleeper = scope.spawn(|| {
            lgwks_bot::rt::runtime::block_on(clock.sleep(span));
            finished.store(true, Ordering::Release);
        });
        let verdict = hold_until_deadline(&clock, span, &finished);
        // Whatever the checks saw, carry the clock past the deadline so the thread ends.
        let released = advance(&clock, span);
        sleeper
            .join()
            .map_err(|payload| format!("seed {seed}: the sleeping thread panicked: {payload:?}"))?;
        verdict?;
        released?;
        require(finished.load(Ordering::Acquire), || {
            format!("seed {seed}: the advance onto the deadline did not end the sleep")
        })
    })?;
    trace.record_number("span_ns", span.as_nanos());
    Ok(trace.hash())
}

#[test]
fn sim_clock_double_clicker_a_sleep_on_another_thread_ends_only_on_the_advance() -> TestResult {
    replays(double_click, THREAD_SEEDS)
}
