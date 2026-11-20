//! Simulated hosts, partitions, and clock skew against the fenced dispatch
//! path (#319).
//!
//! One seed drives three executor hosts against ONE lease authority and ONE
//! durable store — one domain, because two authorities for one domain would be
//! split-brain by construction. The faults injected per step are partitions (a
//! host cut off from the authority), clock skew (a host perceiving a shifted
//! epoch), coordinator loss (a lease dropped with runs open), and store
//! restarts (a second handle proving what the first committed). The real
//! [`AuthorityHandle`](lgwks_bot::dist_lease::AuthorityHandle),
//! [`LoopbackChannel`](lgwks_bot::dist_lease::LoopbackChannel),
//! [`WorkQueue`](lgwks_bot::dist_lease::WorkQueue),
//! [`Coordinator`](lgwks_bot::dist_lease::Coordinator) and
//! [`TenantStore`](lgwks_bot::tenant_store::TenantStore) are driven against an
//! independent model that predicts every validation, every commit, and every
//! refusal.
//!
//! The invariants held every step, under every fault:
//!
//! - **Fencing.** No commit ever lands under a lease the authority deems
//!   stale at that step: each commit records the authority epoch beside the
//!   lease epoch, and the two are equal.
//! - **Partition safety.** A partitioned host's validations all fail closed,
//!   and a partitioned host commits nothing.
//! - **Skew irrelevance.** Hosts perceive a shifted epoch, but perception
//!   never decides a grant: validation does. Whenever perception disagrees
//!   with the authority — behind through lag and skew, or ahead through
//!   positive skew — the grant follows the validation, and the trace counts
//!   the overruling.
//! - **Bounded queues.** Every host queue holds at most its capacity, and
//!   every refused push hands its item back.
//! - **Durable recovery.** A killed holder's open runs abandon with unknown
//!   outcomes, exactly once.
//!
//! One seed, exact replay: the same seed run twice gives the same trace hash,
//! and the first two seeds diverge.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::path::Path;

use lgwks_std::hash::Hasher;
use lgwks_std::seeded::Seeded;

use lgwks_bot::dist_lease::{
    AuthorityHandle, Coordinator, DispatchError, GrantChannel, HostId, Lease, LoopbackChannel,
    WorkQueue,
};
use lgwks_bot::tenant_store::{RunStart, TenantStore};

use crate::scratch::Scratch;
use sim_model::{lane_must_dispatch, lane_must_end, model_lane_slot, pick_grant};

/// The test result: fixtures bubble with `?`, like the rest of the estate.
type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// How many seeds the family sweeps.
const SEEDS: u64 = 24;

/// How many steps one seed drives.
const STEPS_PER_SEED: usize = 40;

/// How many executor hosts one seed runs.
const HOSTS: usize = 3;

/// What every host queue holds at most.
const QUEUE_CAPACITY: usize = 4;

/// One executor host: its lease, its perception, its queue, and its counters.
struct SimHost {
    /// The host's name, `host-{id}`.
    name: String,
    /// The lease the host currently holds, if any.
    lease: Option<Lease>,
    /// The highest announced epoch the host received.
    cache: u64,
    /// The host's clock skew in epochs: perception is `cache` shifted by
    /// this. Positive skew perceives ahead, negative behind.
    skew: i64,
    /// The host's dispatch queue of run ids.
    queue: WorkQueue<String>,
    /// Grants that took effect for this host.
    commits: u64,
    /// Grants refused as stale for this host.
    refusals: u64,
    /// Validations lost to partition for this host.
    waits: u64,
    /// Grants that proceeded while perception said otherwise.
    overruled: u64,
}

/// One modeled lane: pending, dispatched, or ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelLane {
    /// Admitted but not yet dispatched.
    Pending,
    /// Dispatched but not yet ended.
    Dispatched,
    /// Ended.
    Ended,
}

/// One modeled run: its host, its lanes, and whether it closed.
#[derive(Debug, Clone)]
struct ModelRun {
    /// Which host began the run.
    host: usize,
    /// The lanes in plan order.
    lanes: Vec<ModelLane>,
    /// Whether the run closed, and with what verdict.
    close: Option<String>,
}

/// The independent model: the epoch, the holder, the queues, and the runs.
#[derive(Debug, Default)]
struct Model {
    /// The epoch the authority must report.
    epoch: u64,
    /// The host the authority must name as holder, if any.
    holder: Option<usize>,
    /// The queue contents each host must hold, oldest first.
    queues: BTreeMap<usize, VecDeque<String>>,
    /// Every run the seed began, by id.
    runs: BTreeMap<String, ModelRun>,
    /// How many runs the seed began, naming the next id.
    begun: usize,
}

/// The name of one executor host.
fn host_name(id: usize) -> String {
    format!("host-{id}")
}

/// The tenant one host's runs belong to.
fn host_tenant(id: usize) -> String {
    format!("sim-tenant-{id}")
}

/// What the host perceives: its cached epoch shifted by its skew.
///
/// Perception is read-only display state: it never decides a grant, which is
/// what makes skew a fault the fence absorbs rather than a second fence.
fn perceived(host: &SimHost) -> u64 {
    host.cache.saturating_add_signed(host.skew)
}

/// Feed one observation into the trace: the step's tag and the domain facts,
/// never a path or a wall time.
fn observe(hasher: &mut Hasher, tag: &str, text: &str) {
    hasher.write_framed(tag.as_bytes());
    hasher.write_framed(text.as_bytes());
}

/// The skew a step draws: minus two through two, named without a cast.
fn draw_skew(drawn: u64) -> i64 {
    match drawn {
        0 => -2,
        1 => -1,
        2 => 0,
        3 => 1,
        _ => 2,
    }
}

/// Announce the current epoch to every connected host: the broadcast an
/// acquire and a revoke both emit, dropped on partitioned links.
fn announce(hosts: &mut [SimHost], cut: &BTreeSet<usize>, epoch: u64) {
    for (id, host) in hosts.iter_mut().enumerate() {
        if !cut.contains(&id) && host.cache < epoch {
            host.cache = epoch;
        }
    }
}

/// The authority must report exactly what the model holds: the epoch and the
/// holder, after every mint and every revoke.
fn check_authority(authority: &AuthorityHandle, model: &Model, seed: u64) -> TestResult {
    assert_eq!(
        authority.epoch(),
        model.epoch,
        "seed {seed}: the authority epoch must be the modeled one"
    );
    let holder = authority.holder();
    match (holder, model.holder) {
        (None, None) => Ok::<(), Box<dyn Error>>(()),
        (Some(found), Some(wanted)) => {
            assert_eq!(
                found,
                HostId::new(&host_name(wanted)),
                "seed {seed}: the authority holder must be the modeled one"
            );
            Ok(())
        }
        (found, wanted) => Err(format!(
            "seed {seed}: the authority holder {found:?} must be the modeled {wanted:?}"
        )
        .into()),
    }
}

/// The verdict the trace names: the recorded one, or the open fact when the
/// run has none.
fn trace_verdict(verdict: Option<&str>) -> &str {
    match verdict {
        Some(found) => found,
        None => "open",
    }
}

/// One counter, one more.
fn bump(counter: &mut u64) {
    *counter = counter.saturating_add(1);
}

/// Item `id` of a per-host vector, or the refusal naming what the set lacks.
fn nth<'a, T>(items: &'a mut [T], id: usize, what: &str) -> TestResult<&'a mut T> {
    let found = items
        .get_mut(id)
        .ok_or_else(|| format!("the set holds no {what} {id}"))?;
    Ok(found)
}

/// The per-host drivers one seed step reaches: the hosts, their channels to
/// the authority, and their coordinators over the shared store.
///
/// Grouped because the three travel together to every step, and threading
/// three parallel vectors through every step signature is three chances to
/// hand one host's channel to another host's coordinator.
struct HostSet {
    /// The executor hosts, by id.
    hosts: Vec<SimHost>,
    /// One channel per host over the shared authority.
    channels: Vec<LoopbackChannel>,
    /// One coordinator per host over the shared store and authority.
    coordinators: Vec<Coordinator>,
}

/// One grant's target: the run, the lane, and whether it ends the lane.
type Target = (String, usize, bool);

/// One seed in flight: the authority, the hosts, the model that predicts them,
/// and the seeded draws and trace that make the run replayable.
struct Sim {
    /// The seed that drives every draw.
    seed: u64,
    /// The one lease authority the hosts share.
    authority: AuthorityHandle,
    /// The hosts and their drivers.
    set: HostSet,
    /// What every step must produce.
    model: Model,
    /// The hosts cut off from the authority.
    cut: BTreeSet<usize>,
    /// The seeded draws.
    rng: Seeded,
    /// The trace the seed hashes to.
    hasher: Hasher,
}

impl Sim {
    /// Take the step the draw `choice` selects, on host `hid`.
    fn step(&mut self, choice: u64, hid: usize, dir: &Path) -> TestResult {
        let seed = self.seed;
        match choice {
            0..20 => {
                let lease = self.authority.acquire(&host_name(hid));
                announce(&mut self.set.hosts, &self.cut, lease.epoch());
                nth(&mut self.set.hosts, hid, "host")?.lease = Some(lease);
                self.model.epoch = self.model.epoch.saturating_add(1);
                self.model.holder = Some(hid);
                check_authority(&self.authority, &self.model, seed)?;
                let tag = format!("host-{hid}/{}", self.model.epoch);
                observe(&mut self.hasher, "acquire", &tag);
                Ok(())
            }
            20..40 => self.grant_step(hid),
            40..50 => {
                let healing = self.cut.remove(&hid);
                nth(&mut self.set.channels, hid, "channel")?.set_partitioned(!healing);
                if healing {
                    announce(&mut self.set.hosts, &self.cut, self.authority.epoch());
                } else {
                    self.cut.insert(hid);
                }
                let tag = if healing { "heal" } else { "partition" };
                observe(&mut self.hasher, tag, &format!("host-{hid}"));
                Ok(())
            }
            50..58 => {
                let drawn = draw_skew(self.rng.below(5)?);
                nth(&mut self.set.hosts, hid, "host")?.skew = drawn;
                observe(&mut self.hasher, "skew", &format!("host-{hid}/{drawn}"));
                Ok(())
            }
            58..68 => self.push(hid),
            68..76 => {
                let queue = self.model.queues.entry(hid).or_default();
                let got = nth(&mut self.set.hosts, hid, "host")?.queue.pop();
                assert_eq!(
                    got,
                    queue.pop_front(),
                    "seed {seed}: host-{hid} must pop its modeled queue"
                );
                let tag = format!("host-{hid}/{}", got.is_some());
                observe(&mut self.hasher, "pop", &tag);
                Ok(())
            }
            76..84 => self.kill(hid),
            84..92 => self.begin(hid),
            _ => self.reopen(dir),
        }
    }

    /// The lease host `hid` holds, tracing `tag` when it holds none.
    fn lease_or_trace(&mut self, hid: usize, tag: &str) -> TestResult<Option<Lease>> {
        let held = nth(&mut self.set.hosts, hid, "host")?.lease.clone();
        if held.is_none() {
            observe(&mut self.hasher, tag, &format!("host-{hid}/no-lease"));
        }
        Ok(held)
    }

    /// Push one item on host `hid`'s queue, and the model's.
    fn push(&mut self, hid: usize) -> TestResult {
        let seed = self.seed;
        let item = format!("work-{begun:04}", begun = self.model.begun);
        self.model.begun = self.model.begun.saturating_add(1);
        let queue = self.model.queues.entry(hid).or_default();
        let host = nth(&mut self.set.hosts, hid, "host")?;
        match host.queue.try_push(item.clone()) {
            Ok(()) => {
                queue.push_back(item);
                observe(&mut self.hasher, "push", &format!("host-{hid}/ok"));
            }
            Err(refused) => {
                assert_eq!(
                    refused.into_item(),
                    item,
                    "seed {seed}: a refused push must hand its item back"
                );
                observe(&mut self.hasher, "push", &format!("host-{hid}/full"));
            }
        }
        let held = host.queue.len();
        assert!(
            held <= QUEUE_CAPACITY,
            "seed {seed}: host-{hid} must never exceed its bound"
        );
        assert_eq!(
            held,
            queue.len(),
            "seed {seed}: host-{hid} must hold its modeled queue"
        );
        Ok(())
    }

    /// Drop host `hid`'s lease with runs open, and let a rival host recover them.
    fn kill(&mut self, hid: usize) -> TestResult {
        nth(&mut self.set.hosts, hid, "host")?.lease = None;
        observe(&mut self.hasher, "kill", &format!("host-{hid}"));
        let rid = self.rng.index(HOSTS)?;
        if rid == hid {
            return Ok(());
        }
        if self.cut.contains(&rid) {
            bump(&mut nth(&mut self.set.hosts, rid, "host")?.waits);
            return Ok(());
        }
        if let Some(lease) = nth(&mut self.set.hosts, rid, "host")?.lease.clone() {
            self.recover_for(hid, rid, &lease)?;
        }
        Ok(())
    }

    /// Host `rid` recovers the open runs of the dead host `hid`, once.
    fn recover_for(&mut self, hid: usize, rid: usize, lease: &Lease) -> TestResult {
        let seed = self.seed;
        if !nth(&mut self.set.channels, rid, "channel")?.validate(lease)? {
            bump(&mut nth(&mut self.set.hosts, rid, "host")?.refusals);
            return Ok(());
        }
        let dead = host_name(hid);
        let coordinator = nth(&mut self.set.coordinators, rid, "coordinator")?;
        let closed = coordinator.recover(lease, &dead, "t-recover")?;
        let mut wanted = 0usize;
        for run in self.model.runs.values_mut() {
            if run.host == hid && run.close.is_none() {
                run.close = Some("UNMEASURED".to_owned());
                run.lanes.fill(ModelLane::Ended);
                wanted = wanted.saturating_add(1);
            }
        }
        assert_eq!(
            closed, wanted,
            "seed {seed}: recovery must close exactly the open runs"
        );
        assert_eq!(
            coordinator.recover(lease, &dead, "t-recover-again")?,
            0,
            "seed {seed}: a second recovery must close nothing"
        );
        Ok(())
    }

    /// Host `hid` begins a run: committed under a current lease, fenced under a
    /// stale one, and waited out when cut off.
    fn begin(&mut self, hid: usize) -> TestResult {
        let seed = self.seed;
        let lease = match self.lease_or_trace(hid, "begin")? {
            Some(lease) => lease,
            None => return Ok(()),
        };
        if self.cut.contains(&hid) {
            bump(&mut nth(&mut self.set.hosts, hid, "host")?.waits);
            observe(&mut self.hasher, "begin", &format!("host-{hid}/wait"));
            return Ok(());
        }
        let run = format!(
            "seed-{seed}-host-{hid}-{begun:04}",
            begun = self.model.begun
        );
        self.model.begun = self.model.begun.saturating_add(1);
        let tenant = host_tenant(hid);
        let hostname = nth(&mut self.set.hosts, hid, "host")?.name.clone();
        let start = RunStart::new(&run, &tenant, "/repo", &hostname, 1, "lock", "t-begin");
        let validated = nth(&mut self.set.channels, hid, "channel")?.validate(&lease)?;
        let coordinator = nth(&mut self.set.coordinators, hid, "coordinator")?;
        let outcome = coordinator.begin_run(&lease, &start, &["fmt", "clippy"]);
        let host = nth(&mut self.set.hosts, hid, "host")?;
        if validated {
            outcome?;
            let lanes = vec![ModelLane::Pending, ModelLane::Pending];
            let modeled = ModelRun {
                host: hid,
                lanes,
                close: None,
            };
            self.model.runs.insert(run, modeled);
            bump(&mut host.commits);
            observe(&mut self.hasher, "begin", &format!("host-{hid}/ok"));
        } else {
            bump(&mut host.refusals);
            require_fenced(seed, &run, outcome, "begin")?;
            observe(&mut self.hasher, "begin", &format!("host-{hid}/stale"));
        }
        Ok(())
    }

    /// A second handle over the same directory reads exactly the modeled runs.
    fn reopen(&mut self, dir: &Path) -> TestResult {
        let seed = self.seed;
        let probe = TenantStore::open(dir)?;
        for tenant_no in 0..HOSTS {
            let rows = probe.runs(&host_tenant(tenant_no), 500)?;
            let modeled = self
                .model
                .runs
                .values()
                .filter(|run| run.host == tenant_no)
                .count();
            assert_eq!(
                rows.len(),
                modeled,
                "seed {seed}: a reopened handle must read the modeled runs"
            );
        }
        observe(&mut self.hasher, "reopen", "ok");
        Ok(())
    }

    /// One grant attempt by host `hid`: validate through the channel, then
    /// commit through the coordinator only past a current lease.
    fn grant_step(&mut self, hid: usize) -> TestResult {
        let seed = self.seed;
        let lease = match self.lease_or_trace(hid, "grant")? {
            Some(lease) => lease,
            None => return Ok(()),
        };
        let channel = nth(&mut self.set.channels, hid, "channel")?;
        if channel.partitioned() {
            assert!(
                self.cut.contains(&hid),
                "seed {seed}: a partitioned channel must be a cut host"
            );
            if channel.validate(&lease).is_ok() {
                lgwks_std::trace::debug!("grant_step: returning an error to the caller");
                return Err(format!("seed {seed}: a partitioned channel must not answer").into());
            }
            bump(&mut nth(&mut self.set.hosts, hid, "host")?.waits);
            observe(&mut self.hasher, "grant", &format!("host-{hid}/wait"));
            return Ok(());
        }
        let validated = channel.validate(&lease)?;
        assert_eq!(
            validated,
            lease.epoch() == self.model.epoch,
            "seed {seed}: host-{hid} must validate exactly as the model predicts"
        );
        let host = nth(&mut self.set.hosts, hid, "host")?;
        let saw = perceived(host);
        if saw != self.model.epoch {
            bump(&mut host.overruled);
        }
        let tag = format!("host-{hid}/{validated}/saw-{saw}");
        observe(&mut self.hasher, "grant", &tag);
        match pick_grant(&mut self.rng, &self.model, hid)? {
            Some(target) => self.apply_grant(hid, &lease, validated, target),
            None => Ok(()),
        }
    }

    /// Commit the drawn target through the coordinator when the lease validated,
    /// and require the fence to refuse it when it did not.
    fn apply_grant(
        &mut self,
        hid: usize,
        lease: &Lease,
        validated: bool,
        target: Target,
    ) -> TestResult {
        let seed = self.seed;
        let (run, lane, end) = target;
        let coordinator = nth(&mut self.set.coordinators, hid, "coordinator")?;
        let at = coordinator.handle().epoch();
        let must_commit = if end {
            lane_must_end(&self.model, &run, lane)?
        } else {
            lane_must_dispatch(&self.model, &run, lane)?
        };
        let answer = if end {
            coordinator.end_lane(lease, &run, lane, "pass", "detail", b"tail")
        } else {
            coordinator.dispatch_lane(lease, &run, lane)
        };
        let host = nth(&mut self.set.hosts, hid, "host")?;
        if !validated {
            bump(&mut host.refusals);
            return require_fenced(seed, &run, answer, "grant");
        }
        match answer {
            Ok(()) => {
                assert!(
                    must_commit,
                    "seed {seed}: a committed grant must be modeled"
                );
                let to = if end {
                    ModelLane::Ended
                } else {
                    ModelLane::Dispatched
                };
                *model_lane_slot(&mut self.model, &run, lane)? = to;
                assert_eq!(
                    at,
                    lease.epoch(),
                    "seed {seed}: no commit under a stale epoch"
                );
                bump(&mut host.commits);
            }
            Err(refusal) => {
                lgwks_std::trace::debug!(%refusal, seed, "sim_dist_lease: the coordinator refused a grant");
                assert!(!must_commit, "seed {seed}: a refused grant must be modeled");
            }
        }
        Ok(())
    }

    /// The trace hash once every step has run: each stored run reads its
    /// modeled verdict, and the counters are part of the replay.
    fn finish(&mut self, store: &TenantStore) -> TestResult<String> {
        let seed = self.seed;
        for tenant_no in 0..HOSTS {
            for row in &store.runs(&host_tenant(tenant_no), 500)? {
                let held = self
                    .model
                    .runs
                    .get(row.run_id())
                    .ok_or_else(|| format!("seed {seed}: the model lost {}", row.run_id()))?;
                assert_eq!(
                    row.verdict(),
                    held.close.as_deref(),
                    "seed {seed}: {} must read its modeled verdict",
                    row.run_id()
                );
                let tag = format!("{}/{}", row.run_id(), trace_verdict(row.verdict()));
                observe(&mut self.hasher, "final", &tag);
            }
        }
        for (hid, host) in self.set.hosts.iter().enumerate() {
            let (commits, refusals) = (host.commits, host.refusals);
            let tag = format!(
                "host-{hid}/{commits}/{refusals}/{}/{}",
                host.waits, host.overruled
            );
            observe(&mut self.hasher, "counters", &tag);
        }
        Ok(self.hasher.finalize().to_hex())
    }
}

/// A stale `what` must be fenced by the lease check, and by nothing else.
fn require_fenced(
    seed: u64,
    run: &str,
    outcome: Result<(), DispatchError>,
    what: &str,
) -> TestResult {
    match outcome {
        Err(DispatchError::Lease(_)) => Ok(()),
        Err(other) => {
            lgwks_std::trace::warn!(seed, %other, what, "sim_dist_lease: a stale step was refused by something else");
            Err(format!("seed {seed}: a stale {what} must fence, got {other}").into())
        }
        Ok(()) => {
            lgwks_std::trace::warn!(seed, run = %run, what, "sim_dist_lease: a stale step committed");
            Err(format!("seed {seed}: a stale {what} must not commit").into())
        }
    }
}

/// Drive one seed and return its trace hash.
fn run_seed(seed: u64) -> TestResult<String> {
    let scratch = Scratch::new("sim-dist-lease")?;
    let dir = scratch.path();
    let store = TenantStore::open(dir)?;
    let authority = AuthorityHandle::new();
    let mut set = HostSet {
        hosts: Vec::new(),
        channels: Vec::new(),
        coordinators: Vec::new(),
    };
    for id in 0..HOSTS {
        set.hosts.push(SimHost {
            name: host_name(id),
            lease: None,
            cache: 0,
            skew: 0,
            queue: WorkQueue::new(QUEUE_CAPACITY)
                .map_err(|error| format!("the host queue must build: {error}"))?,
            commits: 0,
            refusals: 0,
            waits: 0,
            overruled: 0,
        });
        set.channels.push(LoopbackChannel::new(authority.clone()));
        set.coordinators.push(Coordinator::with_authority(
            store.clone(),
            &host_name(id),
            authority.clone(),
        ));
    }
    let mut hasher = Hasher::new();
    hasher.write_framed(&seed.to_le_bytes());
    let mut sim = Sim {
        seed,
        authority,
        set,
        model: Model::default(),
        cut: BTreeSet::new(),
        rng: Seeded::from_seed(seed),
        hasher,
    };
    for _ in 0..STEPS_PER_SEED {
        let choice = sim.rng.below(100)?;
        let hid = sim.rng.index(HOSTS)?;
        sim.step(choice, hid, dir)?;
    }
    sim.finish(&store)
}

/// Every seed replays to the same trace hash, and the first two seeds
/// diverge: one seed, exact replay, and a nondeterministic run fails even
/// when every assertion happens to pass.
#[test]
fn partitions_and_skew_never_break_the_fence() -> TestResult {
    let mut hashes: Vec<String> = Vec::new();
    let mut seed = 0u64;
    while seed < SEEDS {
        let first = run_seed(seed)?;
        let second = run_seed(seed)?;
        assert_eq!(
            first, second,
            "seed {seed} must replay to the same trace hash"
        );
        hashes.push(first);
        seed = seed.saturating_add(1);
    }
    let count = u64::try_from(hashes.len())
        .map_err(|error| format!("the seed count does not fit: {error}"))?;
    assert_eq!(count, SEEDS, "every seed must leave a hash");
    let diverge = match (hashes.first(), hashes.get(1)) {
        (Some(first), Some(second)) => first != second,
        _ => false,
    };
    assert!(diverge, "distinct seeds must drive distinct histories");
    Ok(())
}

mod sim_model;
