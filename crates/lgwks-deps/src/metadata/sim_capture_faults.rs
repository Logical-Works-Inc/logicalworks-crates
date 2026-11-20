//! Seeded adversarial simulations of the collection fault plan and the capture owner.
//!
//! The fault plan became a value the caller passes (it was a thread-local), so these sims
//! attack exactly what that change claims: a plan fires each armed fault once and in order,
//! two plans never trade a fault, the production plan never fires, and every capture a run
//! created is gone however the run ends. The oracle is a reference model (a plain queue of
//! armed points run beside the real [`Faults`]) or an operating-system fact (the path is
//! absent, the program was refused), never the plan's own answer read back.

use std::collections::VecDeque;
use std::error::Error;
#[cfg(unix)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lgwks_std::seeded::Seeded;

use super::MetadataError;
use super::capture::{CaptureFiles, CleanupObligation, run_bounded};
use super::faults::{FaultPoint, Faults, TestResult, assert_paths_removed};

/// Every point a plan can arm, so a draw can name any of them.
const POINTS: [FaultPoint; 11] = [
    FaultPoint::CaptureStdout,
    FaultPoint::CaptureStderr,
    FaultPoint::Spawn,
    FaultPoint::WaitObservation,
    FaultPoint::StatStdout,
    FaultPoint::StatStderr,
    FaultPoint::ReadStdout,
    FaultPoint::ReadStderr,
    FaultPoint::Kill,
    FaultPoint::Reap,
    FaultPoint::Unlink,
];

/// Seeds per plan-only sweep: each seed is microseconds, so the sweep stays fast.
const SEEDS: u64 = 256;

/// What a sim helper answers.
type Drawn<T> = Result<T, Box<dyn Error>>;

/// One point, drawn uniformly.
fn point(stream: &mut Seeded) -> Drawn<FaultPoint> {
    let at = stream.index(POINTS.len())?;
    POINTS
        .get(at)
        .copied()
        .ok_or_else(|| "a drawn index fell outside the point table".into())
}

/// Up to `most` points, repeats allowed.
fn plan(stream: &mut Seeded, most: usize) -> Drawn<Vec<FaultPoint>> {
    let len = stream.index(most.saturating_add(1))?;
    (0..len).map(|_| point(stream)).collect()
}

/// The point a hostile caller asks for next: the model's front half the time, so the
/// plan is actually spent, and any point the rest, so a wrong fire has room to happen.
fn asked(stream: &mut Seeded, model: &Model) -> Drawn<FaultPoint> {
    match (stream.below(2)?, model.armed.front()) {
        (0, Some(front)) => Ok(*front),
        _ => point(stream),
    }
}

/// The reference: only the front armed point fires, once.
struct Model {
    /// Points still armed, in firing order.
    armed: VecDeque<FaultPoint>,
}

impl Model {
    /// A model armed with `points`.
    fn new(points: &[FaultPoint]) -> Self {
        Self {
            armed: points.iter().copied().collect(),
        }
    }

    /// Whether asking for `point` fires, consuming it when it does.
    fn take(&mut self, point: FaultPoint) -> bool {
        if self.armed.front() != Some(&point) {
            return false;
        }
        self.armed.pop_front();
        true
    }
}

/// A fresh capture name under the temp directory that no other run can hold.
fn unique_capture(label: &str) -> Drawn<PathBuf> {
    let token = lgwks_std::hex::encode(lgwks_std::random::bytes::<12>()?);
    Ok(std::env::temp_dir().join(format!("lgwks-deps-sim-{label}-{token}.capture")))
}

/// `count` real capture files on disk.
fn real_captures(count: usize) -> Drawn<Vec<PathBuf>> {
    let mut paths = Vec::with_capacity(count);
    for _ in 0..count {
        let path = unique_capture("real")?;
        std::fs::write(&path, b"captured")?;
        paths.push(path);
    }
    Ok(paths)
}

/// Monkey: thousands of nonsense asks; every answer must match the queue model.
#[test]
fn sim_faults_monkey_every_take_matches_the_queue_model() -> TestResult {
    for seed in 0..SEEDS {
        let mut stream = Seeded::from_seed(seed);
        let armed = plan(&mut stream, 16)?;
        let faults = Faults::armed(&armed);
        let mut model = Model::new(&armed);
        for step in 0..200_u32 {
            let ask = asked(&mut stream, &model)?;
            let fired = faults.take(ask).is_some();
            if fired != model.take(ask) {
                return Err(format!(
                    "seed {seed} step {step}: {ask:?} fired={fired}, the model disagrees"
                )
                .into());
            }
        }
        if faults.is_spent() != model.armed.is_empty() {
            return Err(
                format!("seed {seed}: the plan's spent flag disagrees with the model").into(),
            );
        }
    }
    Ok(())
}

/// The monkey's trace for one seed, folded with FNV-1a over each ask and its answer.
fn monkey_trace(seed: u64) -> Drawn<u64> {
    let mut stream = Seeded::from_seed(seed);
    let armed = plan(&mut stream, 16)?;
    let faults = Faults::armed(&armed);
    let mut model = Model::new(&armed);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for _ in 0..200_u32 {
        let ask = asked(&mut stream, &model)?;
        model.take(ask);
        let at = POINTS
            .iter()
            .position(|known| *known == ask)
            .ok_or("an unknown point")?;
        let fired = u8::from(faults.take(ask).is_some());
        for byte in [u8::try_from(at)?, fired] {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    Ok(hash)
}

/// Determinism: one seed replays one trace hash; different seeds do not all collapse.
#[test]
fn sim_faults_monkey_same_seed_same_trace_hash() -> TestResult {
    let mut distinct = std::collections::BTreeSet::new();
    for seed in 0..SEEDS {
        let first = monkey_trace(seed)?;
        assert_eq!(
            first,
            monkey_trace(seed)?,
            "seed {seed} replayed a different trace"
        );
        distinct.insert(first);
    }
    assert!(
        distinct.len() > 1,
        "every seed produced one trace: the generator is not driving the run"
    );
    Ok(())
}

/// Double-clicker: asks for each armed fault twice; each still fires exactly once.
#[test]
fn sim_faults_double_clicker_each_armed_fault_fires_exactly_once() -> TestResult {
    for seed in 0..SEEDS {
        let mut stream = Seeded::from_seed(seed);
        let armed = plan(&mut stream, 12)?;
        let faults = Faults::armed(&armed);
        let mut fired = 0_usize;
        for &ask in &armed {
            for _click in 0..2_u8 {
                if faults.take(ask).is_some() {
                    fired = fired.saturating_add(1);
                }
            }
        }
        assert_eq!(
            fired,
            armed.len(),
            "seed {seed}: {armed:?} fired {fired} times"
        );
        assert!(
            faults.is_spent(),
            "seed {seed}: a fault survived two clicks each"
        );
    }
    Ok(())
}

/// Wrong-order: asks for the second fault first; it must wait for its turn.
#[test]
fn sim_faults_wrong_order_a_later_fault_never_fires_before_its_turn() -> TestResult {
    for seed in 0..SEEDS {
        let mut stream = Seeded::from_seed(seed);
        let first = point(&mut stream)?;
        let others: Vec<FaultPoint> = POINTS
            .iter()
            .copied()
            .filter(|other| *other != first)
            .collect();
        let second = others
            .get(stream.index(others.len())?)
            .copied()
            .ok_or("no second point")?;
        let faults = Faults::armed(&[first, second]);
        assert!(
            faults.take(second).is_none(),
            "seed {seed}: {second:?} fired ahead of {first:?}"
        );
        assert!(
            faults.take(first).is_some(),
            "seed {seed}: the front fault did not fire"
        );
        assert!(
            faults.take(first).is_none(),
            "seed {seed}: the front fault fired twice"
        );
        assert!(
            faults.take(second).is_some(),
            "seed {seed}: the second fault never came due"
        );
        assert!(faults.is_spent(), "seed {seed}: the plan kept a fault");
    }
    Ok(())
}

/// Neighbour: two plans interleaved on one seed never trade a fault.
#[test]
fn sim_faults_neighbour_two_interleaved_plans_never_trade_a_fault() -> TestResult {
    for seed in 0..SEEDS {
        let mut stream = Seeded::from_seed(seed);
        let (armed_a, armed_b) = (plan(&mut stream, 10)?, plan(&mut stream, 10)?);
        let tenants = [
            (Faults::armed(&armed_a), Model::new(&armed_a)),
            (Faults::armed(&armed_b), Model::new(&armed_b)),
        ];
        let mut tenants = tenants;
        for step in 0..300_u32 {
            let which = stream.index(tenants.len())?;
            let &mut (ref faults, ref mut model) =
                tenants.get_mut(which).ok_or("no such tenant")?;
            let ask = asked(&mut stream, model)?;
            if faults.take(ask).is_some() != model.take(ask) {
                return Err(format!(
                    "seed {seed} step {step}: tenant {which} saw its neighbour's plan"
                )
                .into());
            }
        }
        for tenant in &tenants {
            assert_eq!(
                tenant.0.is_spent(),
                tenant.1.armed.is_empty(),
                "seed {seed}: a plan was spent by its neighbour"
            );
        }
    }
    Ok(())
}

/// Why one tenant's threaded run failed.
#[derive(Debug)]
enum TenantFault {
    /// The seeded generator refused a draw; its message, since the boxed cause is not `Send`.
    Draw { tenant: u64, cause: String },
    /// The tenant's plan answered differently from its model: a neighbour's fault arrived.
    Crossed { tenant: u64, step: u32 },
    /// The tenant's thread panicked.
    Panicked(String),
}

impl std::fmt::Display for TenantFault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Draw { tenant, ref cause } => {
                write!(formatter, "tenant {tenant}: a draw was refused: {cause}")
            }
            Self::Crossed { tenant, step } => {
                write!(
                    formatter,
                    "tenant {tenant} step {step}: another thread's fault reached this plan"
                )
            }
            Self::Panicked(ref payload) => write!(formatter, "a tenant thread panicked: {payload}"),
        }
    }
}

impl Error for TenantFault {}

/// One tenant's private run for the threaded neighbour sim.
fn tenant_run(tenant: u64) -> Result<(), TenantFault> {
    let refused = |cause: Box<dyn Error>| TenantFault::Draw {
        tenant,
        cause: cause.to_string(),
    };
    let mut stream = Seeded::from_seed(tenant);
    let armed = plan(&mut stream, 32).map_err(refused)?;
    let faults = Faults::armed(&armed);
    let mut model = Model::new(&armed);
    for step in 0..1_000_u32 {
        let ask = asked(&mut stream, &model).map_err(refused)?;
        if faults.take(ask).is_some() != model.take(ask) {
            let refusal = Err(TenantFault::Crossed { tenant, step });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "tenant_run: returning an error to the caller");
            return refusal;
        }
    }
    Ok(())
}

/// Neighbour at concurrency: 100 threads, one plan each, the old thread-local's failure shape.
#[test]
fn sim_faults_neighbour_a_hundred_threads_each_see_only_their_plan() -> TestResult {
    let outcomes: Vec<Result<(), TenantFault>> = std::thread::scope(|scope| {
        // Every tenant is started before any is joined, or the threads run one at a time.
        let mut workers = Vec::with_capacity(100);
        for tenant in 0..100_u64 {
            workers.push(scope.spawn(move || tenant_run(tenant)));
        }
        workers
            .into_iter()
            .map(|worker| match worker.join() {
                Ok(outcome) => outcome,
                Err(panic) => Err(TenantFault::Panicked(format!("{panic:?}"))),
            })
            .collect()
    });
    for outcome in outcomes {
        outcome?;
    }
    Ok(())
}

/// Saboteur: the production plan never fires, never records, and is always spent.
#[test]
fn sim_faults_saboteur_the_production_plan_never_fires_or_records() -> TestResult {
    for seed in 0..SEEDS {
        let mut stream = Seeded::from_seed(seed);
        let faults = Faults::default();
        let twin = faults.clone();
        for _ in 0..50_u32 {
            let ask = point(&mut stream)?;
            assert!(
                faults.take(ask).is_none() && twin.take(ask).is_none(),
                "seed {seed}: production fired {ask:?}"
            );
        }
        twin.record_capture(&unique_capture("production")?);
        assert!(
            faults.captured_paths().is_empty(),
            "seed {seed}: production kept a capture list"
        );
        assert!(
            faults.is_spent(),
            "seed {seed}: production reports an armed fault"
        );
    }
    Ok(())
}

/// Giant-and-empty: an empty plan and a 10,000-point plan each fire exactly their length.
#[test]
fn sim_faults_giant_and_empty_plans_fire_exactly_their_length() -> TestResult {
    for len in [0_usize, 1, POINTS.len(), 10_000] {
        let mut stream = Seeded::from_seed(u64::try_from(len)?);
        let armed: Vec<FaultPoint> = (0..len).map(|_| point(&mut stream)).collect::<Drawn<_>>()?;
        let faults = Faults::armed(&armed);
        let fired = armed
            .iter()
            .filter(|ask| faults.take(**ask).is_some())
            .count();
        assert_eq!(fired, len, "a {len}-point plan fired {fired}");
        let extra = point(&mut stream)?;
        assert!(
            faults.take(extra).is_none(),
            "a spent {len}-point plan fired {extra:?}"
        );
        assert!(faults.is_spent(), "a {len}-point plan kept a fault");
    }
    Ok(())
}

/// Double-clicker on clones: the run and its cleanup owner spend one plan, never two.
#[test]
fn sim_faults_double_clicker_a_clone_spends_the_same_plan() -> TestResult {
    for seed in 0..SEEDS {
        let mut stream = Seeded::from_seed(seed);
        let armed = plan(&mut stream, 8)?;
        let faults = Faults::armed(&armed);
        let twin = faults.clone();
        for (at, ask) in armed.iter().enumerate() {
            let holder = if at % 2 == 0 { &faults } else { &twin };
            assert!(
                holder.take(*ask).is_some(),
                "seed {seed}: fault {at} did not fire through its holder"
            );
        }
        assert!(
            faults.is_spent() && twin.is_spent(),
            "seed {seed}: the clones hold separate plans"
        );
        let path = unique_capture("twin")?;
        twin.record_capture(&path);
        assert_eq!(
            faults.captured_paths(),
            vec![path],
            "seed {seed}: a clone's capture is invisible to its origin"
        );
    }
    Ok(())
}

/// Weird-bytes: absent captures with hostile names are owned verbatim and cleared;
/// a NUL name the OS cannot even look up is a retained failure, never a silent drop.
#[test]
fn sim_capture_files_weird_bytes_names_are_owned_verbatim() -> TestResult {
    const NAMES: [&str; 8] = [
        "",
        " ",
        "..",
        "名前",
        "a\u{202e}b",
        "nul-%00",
        "zero\u{200b}width",
        "../escape",
    ];
    for seed in 0..32_u64 {
        let mut stream = Seeded::from_seed(seed);
        let faults = Faults::armed(&[]);
        let mut captures = CaptureFiles::new(faults.clone());
        let mut owned = Vec::new();
        for _ in 0..NAMES.len() {
            let name = NAMES.get(stream.index(NAMES.len())?).ok_or("no name")?;
            let path = std::env::temp_dir().join(format!("lgwks-deps-sim-absent-{seed}-{name}"));
            captures.own(path.clone());
            owned.push(path);
        }
        assert_eq!(
            faults.captured_paths(),
            owned,
            "seed {seed}: a hostile name was rewritten"
        );
        assert!(
            captures.cleanup().is_empty(),
            "seed {seed}: an absent capture counted as a failure"
        );
        assert!(
            captures.paths.is_empty(),
            "seed {seed}: an absent capture stayed owned"
        );
        captures.own(std::env::temp_dir().join("lgwks-deps-sim-nul-\0-name"));
        assert_eq!(
            captures.cleanup().len(),
            1,
            "seed {seed}: a NUL name's refusal was dropped"
        );
        assert_eq!(
            captures.paths.len(),
            1,
            "seed {seed}: a NUL name left ownership unreported"
        );
        captures.paths.clear();
    }
    Ok(())
}

/// Pulled-plug: the first k unlinks fail; exactly those k paths stay owned and on disk.
#[test]
fn sim_capture_files_pulled_plug_keeps_exactly_the_failed_paths() -> TestResult {
    for seed in 0..64_u64 {
        let mut stream = Seeded::from_seed(seed);
        let count = stream.index(6)?.saturating_add(1);
        let failing = stream.index(count.saturating_add(1))?;
        let paths = real_captures(count)?;
        let faults = Faults::armed(&vec![FaultPoint::Unlink; failing]);
        let mut captures = CaptureFiles::new(faults.clone());
        for path in &paths {
            captures.own(path.clone());
        }
        assert_eq!(
            captures.cleanup().len(),
            failing,
            "seed {seed}: {failing} injected unlinks"
        );
        let kept = paths.get(..failing).ok_or("fewer paths than failures")?;
        assert_eq!(
            captures.paths.as_slice(),
            kept,
            "seed {seed}: the wrong paths stayed owned"
        );
        assert!(
            kept.iter().all(|path| path.is_file()),
            "seed {seed}: a failed unlink lost its file"
        );
        assert_paths_removed(paths.get(failing..).ok_or("no removed tail")?);
        assert!(
            captures.cleanup().is_empty(),
            "seed {seed}: the retry failed with no fault armed"
        );
        assert_paths_removed(&paths);
        assert!(
            faults.is_spent(),
            "seed {seed}: an unlink fault was never reached"
        );
    }
    Ok(())
}

/// Saboteur: a dropped cleanup owner retries through its faults and leaves no file.
#[test]
fn sim_obligation_saboteur_a_dropped_owner_leaves_no_capture_behind() -> TestResult {
    for seed in 0..16_u64 {
        let mut stream = Seeded::from_seed(seed);
        let paths = real_captures(stream.index(4)?.saturating_add(1))?;
        let faults = Faults::armed(&vec![FaultPoint::Unlink; stream.index(4)?]);
        let mut captures = CaptureFiles::new(faults.clone());
        for path in &paths {
            captures.own(path.clone());
        }
        {
            let obligation = CleanupObligation::new(None, None, &mut captures);
            assert_eq!(
                obligation.report.capture_paths, paths,
                "seed {seed}: the owner lost a path"
            );
        }
        assert!(
            captures.paths.is_empty(),
            "seed {seed}: the paths have two owners"
        );
        assert_paths_removed(&paths);
        assert!(
            faults.is_spent(),
            "seed {seed}: the drop never retried through the faults"
        );
    }
    Ok(())
}

/// Fat-finger: a program that does not exist is a spawn refusal and leaves no capture.
#[test]
fn sim_run_bounded_fat_finger_a_missing_program_leaves_nothing() -> TestResult {
    for seed in 0..8_u64 {
        let program = format!(
            "lgwks-no-such-program-{seed}-{}",
            lgwks_std::hex::encode(lgwks_std::random::bytes::<6>()?)
        );
        let faults = Faults::armed(&[]);
        let refused = run_bounded(
            &program,
            &[],
            Path::new("."),
            Duration::from_secs(5),
            1024,
            &faults,
        );
        assert!(
            matches!(refused, Err(MetadataError::Spawn(_))),
            "seed {seed}: {program} was not a spawn refusal"
        );
        let recorded = faults.captured_paths();
        assert_eq!(
            recorded.len(),
            2,
            "seed {seed}: both captures are created before the spawn"
        );
        assert_paths_removed(&recorded);
    }
    Ok(())
}

/// Clock-fool: a deadline already past kills the child and still removes both captures.
#[cfg(unix)]
#[test]
fn sim_run_bounded_clock_fool_a_past_deadline_kills_and_cleans() {
    let args = [OsString::from("-c"), OsString::from("sleep 10 & wait")];
    for timeout in [Duration::ZERO, Duration::from_nanos(1)] {
        // A run that waited out the 10 s sleeper would answer its exit status, so
        // `Timeout` alone proves the past deadline fired without waiting.
        let faults = Faults::armed(&[]);
        let refused = run_bounded("sh", &args, Path::new("."), timeout, 1024, &faults);
        assert!(
            matches!(refused, Err(MetadataError::Timeout { after }) if after == timeout),
            "a {timeout:?} deadline did not time out"
        );
        assert_paths_removed(&faults.captured_paths());
    }
}

/// Clock-fool: a deadline past the end of the clock is no deadline; the run completes.
#[cfg(unix)]
#[test]
fn sim_run_bounded_clock_fool_a_deadline_past_the_clock_still_completes() -> TestResult {
    let args = [OsString::from("-c"), OsString::from("printf clock")];
    let faults = Faults::armed(&[]);
    let output = run_bounded("sh", &args, Path::new("."), Duration::MAX, 1024, &faults)?;
    assert_eq!(output.stdout, b"clock", "the bytes the child printed");
    assert_paths_removed(&faults.captured_paths());
    Ok(())
}

/// Giant-and-empty at the byte budget: exactly the cap passes, one byte over is refused.
#[cfg(unix)]
#[test]
fn sim_run_bounded_giant_and_empty_the_cap_is_inclusive_by_one_byte() -> TestResult {
    for seed in 0..6_u64 {
        let mut stream = Seeded::from_seed(seed);
        let len = stream.index(64)?.saturating_add(1);
        let payload = "a".repeat(len);
        let args = [
            OsString::from("-c"),
            OsString::from(format!("printf {payload}")),
        ];
        let at_cap = Faults::armed(&[]);
        let output = run_bounded(
            "sh",
            &args,
            Path::new("."),
            Duration::from_secs(30),
            len,
            &at_cap,
        )?;
        assert_eq!(
            output.stdout,
            payload.as_bytes(),
            "seed {seed}: {len} bytes at a {len}-byte cap"
        );
        assert_paths_removed(&at_cap.captured_paths());
        let over = Faults::armed(&[]);
        let cap = len.saturating_sub(1);
        let refused = run_bounded(
            "sh",
            &args,
            Path::new("."),
            Duration::from_secs(30),
            cap,
            &over,
        );
        assert!(
            matches!(refused, Err(MetadataError::OutputTooLarge { stream: "stdout", limit }) if limit == cap),
            "seed {seed}: {len} bytes passed a {cap}-byte cap"
        );
        assert_paths_removed(&over.captured_paths());
    }
    Ok(())
}
