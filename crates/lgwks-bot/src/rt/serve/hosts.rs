//! The tenant host table, sharded by tenant (#1008).
//!
//! One `Mutex<HashMap>` held while a host was built made every request wait on
//! every other: the lookup of a built host queued behind another tenant's first
//! request, which opens a file under an advisory lock. The table is now
//! [`SHARDS`] read-write locks, each over a map of tenant to [`Slot`]. A lookup
//! of a built host takes one shard's read lock for a map get and a clone. A
//! first request takes the shard's write lock only to add an empty slot, and
//! builds the host under the slot's own lock, so two tenants never wait on each
//! other's build and two builders never open one tenant's run store. No lock is
//! held across an await: the table is plain `std` locks over short, sync work.
//!
//! Bounds: at most [`MAX_TENANT_HOSTS`] slots per serve, claimed with one atomic
//! step before the slot exists. Past it a new tenant is refused as
//! [`ServeError::TooManyTenants`] and a known tenant is served as before.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

use super::ServeError;
use crate::task::Host;

/// How many shards the table has: a power of two, so a hash picks one by mask.
pub(super) const SHARDS: usize = 64;

/// The most tenant slots one serve holds. Each built slot is a run store file
/// open and a storage thread, so the bound is the serve's file and thread
/// budget, not a memory one; it sits well under a 4,096 descriptor limit and
/// leaves room for the sockets of a full in-flight ceiling.
pub(super) const MAX_TENANT_HOSTS: usize = 1_024;

/// One shard's map, tenant name to slot. Ordered, so a walk of a shard is.
type Table = BTreeMap<String, Arc<Slot>>;

/// One shard.
type Shard = RwLock<Table>;

/// One tenant's place in the table: its host once built, and the lock that lets
/// one caller build it.
struct Slot {
    /// The host, set once by the caller that built it.
    host: OnceLock<Host>,
    /// Held while the host is built, so a second caller waits for the first.
    build: Mutex<()>,
}

/// The shard a tenant's name falls in.
fn shard_of(tenant: &str) -> usize {
    let mut hasher = DefaultHasher::new();
    tenant.hash(&mut hasher);
    let [low, ..] = hasher.finish().to_le_bytes();
    usize::from(low) & (SHARDS - 1)
}

/// The read half of a shard, recovered if a panicking holder poisoned it: a
/// shard is a plain map that is consistent after any single call returns.
fn read(shard: &Shard) -> RwLockReadGuard<'_, Table> {
    match shard.read() {
        Ok(held) => held,
        Err(poisoned) => {
            lgwks_std::trace::debug!("serve: a host shard was poisoned; recovered");
            poisoned.into_inner()
        }
    }
}

/// The write half of a shard, recovered as [`read`] recovers it.
fn write(shard: &Shard) -> RwLockWriteGuard<'_, Table> {
    match shard.write() {
        Ok(held) => held,
        Err(poisoned) => {
            lgwks_std::trace::debug!("serve: a host shard was poisoned; recovered");
            poisoned.into_inner()
        }
    }
}

/// A slot's build lock, recovered as [`read`] recovers a shard.
fn building(lock: &Mutex<()>) -> MutexGuard<'_, ()> {
    match lock.lock() {
        Ok(held) => held,
        Err(poisoned) => {
            lgwks_std::trace::debug!("serve: a host build lock was poisoned; recovered");
            poisoned.into_inner()
        }
    }
}

/// Every tenant's host, in [`SHARDS`] shards.
pub(super) struct Hosts {
    /// The shards, each a map of tenant to slot.
    shards: [Shard; SHARDS],
    /// Slots claimed, never past `cap`.
    slots: AtomicUsize,
    /// The most slots this table holds.
    cap: usize,
}

impl Hosts {
    /// An empty table that holds at most `cap` tenants.
    pub(super) fn new(cap: usize) -> Self {
        Self {
            shards: std::array::from_fn(|_| RwLock::new(BTreeMap::new())),
            slots: AtomicUsize::new(0),
            cap,
        }
    }

    /// Claim one slot against the cap, or refuse as the typed error.
    fn admit(&self) -> Result<(), ServeError> {
        let claimed = self
            .slots
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |held| {
                (held < self.cap).then(|| held.saturating_add(1))
            });
        match claimed {
            Ok(_before) => Ok(()),
            Err(held) => {
                let refusal = ServeError::TooManyTenants { cap: self.cap };
                lgwks_std::trace::debug!(error = ?refusal, held, "serve: the tenant table is full");
                Err(refusal)
            }
        }
    }

    /// The slot for `tenant`, adding an empty one if the tenant is new.
    fn slot(&self, tenant: &str) -> Result<Arc<Slot>, ServeError> {
        let shard = self.shards.get(shard_of(tenant)).ok_or_else(|| {
            let refusal = ServeError::Host {
                reason: String::from("the tenant fell outside the host shards"),
            };
            lgwks_std::trace::debug!(error = ?refusal, "serve: the host shard is refused");
            refusal
        })?;
        if let Some(slot) = read(shard).get(tenant) {
            return Ok(Arc::clone(slot));
        }
        let mut table = write(shard);
        if let Some(slot) = table.get(tenant) {
            return Ok(Arc::clone(slot));
        }
        self.admit()?;
        let slot = Arc::new(Slot {
            host: OnceLock::new(),
            build: Mutex::new(()),
        });
        table.insert(tenant.to_owned(), Arc::clone(&slot));
        Ok(slot)
    }

    /// The host for `tenant`, built by `build` on its first request.
    ///
    /// Callers of one tenant wait for one build and share its host; callers of
    /// other tenants do not wait at all. A failed build leaves the slot empty,
    /// so the next request builds again.
    pub(super) fn host(
        &self,
        tenant: &str,
        build: impl FnOnce() -> Result<Host, ServeError>,
    ) -> Result<Host, ServeError> {
        let slot = self.slot(tenant)?;
        if let Some(host) = slot.host.get() {
            return Ok(host.clone());
        }
        let _building = building(&slot.build);
        if let Some(host) = slot.host.get() {
            return Ok(host.clone());
        }
        let built = build()?;
        Ok(slot.host.get_or_init(|| built).clone())
    }

    /// Every built host with its tenant, in tenant order, so a drain cancels the
    /// same way every time.
    pub(super) fn all(&self) -> Vec<(String, Host)> {
        let mut built = Vec::new();
        for shard in &self.shards {
            for (tenant, slot) in read(shard).iter() {
                if let Some(host) = slot.host.get() {
                    built.push((tenant.clone(), host.clone()));
                }
            }
        }
        built.sort_by(|left, right| left.0.cmp(&right.0));
        built
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use lgwks_std::seeded::Seeded;

    use super::{Hosts, MAX_TENANT_HOSTS, SHARDS, ServeError, shard_of};
    use crate::task::Host;

    /// A real host for `tenant`, in memory.
    fn host_of(tenant: &str) -> Result<Host, ServeError> {
        let builder = Host::builder(tenant).map_err(|cause| ServeError::Host {
            reason: cause.to_string(),
        })?;
        builder.build().map_err(|cause| ServeError::Host {
            reason: cause.to_string(),
        })
    }

    /// The tenant name of draw `number`.
    fn name(number: u64) -> String {
        format!("tenant-{number}")
    }

    /// A refusal as the text a test reports.
    fn said(refusal: ServeError) -> String {
        refusal.to_string()
    }

    /// Requirement: a tenant lands in one shard, and names spread over them (#1008).
    ///
    /// Catches a shard index that ignores the name, or a mask that leaves most
    /// shards empty so the sharding buys nothing.
    #[test]
    fn a_tenant_hashes_to_one_stable_shard_and_names_spread() {
        let mut seen = BTreeSet::new();
        for number in 0..512_u64 {
            let tenant = name(number);
            let shard = shard_of(&tenant);
            assert_eq!(shard, shard_of(&tenant), "{tenant} changed shard");
            assert!(shard < SHARDS, "{tenant} fell outside the shards");
            seen.insert(shard);
        }
        assert!(
            seen.len().saturating_mul(2) > SHARDS,
            "512 names reached only {} of {SHARDS} shards",
            seen.len()
        );
    }

    /// Requirement: each tenant's host is built once, whoever asks and however
    /// often, and every caller gets its own tenant's host (#1008).
    ///
    /// Eight threads draw tenants from a seeded stream; the builds must equal the
    /// distinct tenants and no caller may see another tenant's host.
    #[test]
    fn sim_each_tenant_is_built_once_under_threads() -> Result<(), String> {
        for seed in 0..4_u64 {
            let hosts = Hosts::new(MAX_TENANT_HOSTS);
            let builds = AtomicUsize::new(0);
            let drawn: Vec<Result<Vec<String>, String>> = std::thread::scope(|scope| {
                let workers: Vec<_> = (0..8_u64)
                    .map(|worker| {
                        let (hosts, builds) = (&hosts, &builds);
                        scope.spawn(move || -> Result<Vec<String>, String> {
                            let mut draws =
                                Seeded::from_seed(seed.wrapping_mul(8).wrapping_add(worker));
                            let mut asked = Vec::new();
                            for _ in 0..40 {
                                let tenant =
                                    name(draws.below(12).map_err(|refusal| {
                                        format!("a draw is refused: {refusal}")
                                    })?);
                                let host = hosts
                                    .host(&tenant, || {
                                        builds.fetch_add(1, Ordering::SeqCst);
                                        host_of(&tenant)
                                    })
                                    .map_err(said)?;
                                if host.tenant().as_str() != tenant {
                                    return Err(format!("{tenant} got another tenant's host"));
                                }
                                asked.push(tenant);
                            }
                            Ok(asked)
                        })
                    })
                    .collect();
                workers
                    .into_iter()
                    .map(|worker| {
                        worker
                            .join()
                            .map_err(|_panic| String::from("a worker panicked"))
                            .and_then(|asked| asked)
                    })
                    .collect()
            });
            let mut distinct = BTreeSet::new();
            for asked in drawn {
                distinct.extend(asked?);
            }
            let (built, listed) = (builds.load(Ordering::SeqCst), hosts.all().len());
            if built != distinct.len() || listed != distinct.len() {
                return Err(format!(
                    "seed {seed}: {built} builds and {listed} hosts for {} tenants",
                    distinct.len()
                ));
            }
        }
        Ok(())
    }

    /// Requirement: one tenant's slow build never holds another tenant (#1008).
    ///
    /// Tenant `a`'s build parks until released. While it is parked, tenant `b`
    /// must be built and served. Catches a table with one lock held across a
    /// build, which is the shape this change replaces.
    #[test]
    fn a_parked_build_for_one_tenant_does_not_hold_another() -> Result<(), String> {
        let hosts = Hosts::new(MAX_TENANT_HOSTS);
        let (entered_tx, entered_rx) = mpsc::sync_channel::<()>(1);
        let (release_tx, release_rx) = mpsc::sync_channel::<()>(1);
        let (done_tx, done_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        std::thread::scope(|scope| {
            let hosts = &hosts;
            let parked = scope.spawn(move || {
                hosts
                    .host("a", || {
                        entered_tx.send(()).map_err(|cause| ServeError::Host {
                            reason: cause.to_string(),
                        })?;
                        release_rx.recv().map_err(|cause| ServeError::Host {
                            reason: cause.to_string(),
                        })?;
                        host_of("a")
                    })
                    .map(|_host| ())
                    .map_err(said)
            });
            let quick = scope.spawn(move || {
                let served = hosts
                    .host("b", || host_of("b"))
                    .map(|_host| ())
                    .map_err(said);
                done_tx.send(served).map_err(|cause| cause.to_string())
            });
            let started = entered_rx
                .recv_timeout(Duration::from_secs(30))
                .map_err(|cause| format!("the parked build never started: {cause}"));
            let served = started.and_then(|()| {
                done_rx
                    .recv_timeout(Duration::from_secs(30))
                    .map_err(|cause| format!("tenant b waited on tenant a's build: {cause}"))
            });
            let released = release_tx.send(()).map_err(|cause| cause.to_string());
            let first = parked
                .join()
                .map_err(|_panic| String::from("the parked build panicked"))?;
            let second = quick
                .join()
                .map_err(|_panic| String::from("the quick build panicked"))?;
            released?;
            second?;
            served??;
            first
        })
    }

    /// Requirement: past the cap a new tenant is refused with the typed error and
    /// a known tenant is still served; concurrent admissions never overshoot
    /// (#1008).
    #[test]
    fn past_the_cap_a_new_tenant_is_refused_and_a_known_one_is_served() -> Result<(), String> {
        const CAP: usize = 5;
        const ASKS: usize = 32;
        let hosts = Hosts::new(CAP);
        let admitted = AtomicUsize::new(0);
        let refused = AtomicUsize::new(0);
        let stray = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for worker in 0..8_u64 {
                let (hosts, admitted, refused, stray) = (&hosts, &admitted, &refused, &stray);
                scope.spawn(move || {
                    for number in 0..4_u64 {
                        let tenant = name(worker.wrapping_mul(4).wrapping_add(number));
                        let counter = match hosts.host(&tenant, || host_of(&tenant)) {
                            Ok(_host) => admitted,
                            Err(ServeError::TooManyTenants { cap }) if cap == CAP => refused,
                            Err(_other) => stray,
                        };
                        counter.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
        });
        let took = admitted.load(Ordering::SeqCst);
        let turned = refused.load(Ordering::SeqCst);
        let strays = stray.load(Ordering::SeqCst);
        if took != CAP || turned != ASKS - CAP || strays != 0 {
            return Err(format!(
                "{took} admitted, {turned} refused, {strays} other; wanted {CAP}, {}, 0",
                ASKS - CAP
            ));
        }
        let known = hosts.all();
        let first = known
            .first()
            .map(|entry| entry.0.clone())
            .ok_or_else(|| String::from("no tenant was admitted"))?;
        hosts
            .host(&first, || host_of(&first))
            .map(|_host| ())
            .map_err(|refusal| format!("a known tenant was refused after the cap: {refusal}"))
    }

    /// Requirement: a failed build leaves no host and the next request builds
    /// again; `all` lists built hosts in tenant order (#1008).
    #[test]
    fn a_failed_build_is_retried_and_all_lists_hosts_in_tenant_order() -> Result<(), String> {
        let hosts = Hosts::new(MAX_TENANT_HOSTS);
        let first = hosts.host("flaky", || {
            Err(ServeError::Host {
                reason: String::from("the first build fails"),
            })
        });
        if first.is_ok() || !hosts.all().is_empty() {
            return Err(String::from("a failed build left a host behind"));
        }
        for tenant in ["zed", "flaky", "amy", "mid"] {
            hosts.host(tenant, || host_of(tenant)).map_err(said)?;
        }
        let order: Vec<String> = hosts
            .all()
            .into_iter()
            .map(|(tenant, _host)| tenant)
            .collect();
        if order == ["amy", "flaky", "mid", "zed"] {
            Ok(())
        } else {
            Err(format!("the hosts came back as {order:?}"))
        }
    }
}
