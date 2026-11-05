//! Seeded disk-fault simulation over the real sealed journals (#819).
//!
//! One `u64` seed drives everything that varies: the op schedule, which tenant
//! acts, attempt numbers, and where every fault lands. The subjects are the
//! real [`FileJournal`](lgwks_bot::journal::FileJournal) sealed files on a real
//! directory through the public interface only: `open_sealed`,
//! `compare_and_append_sealed`, `sealed_entries`, `verify_sealed_chain`,
//! `recover` and the reopen that repairs a torn tail. Nothing here reimplements
//! a journal, a frame or a fold.
//!
//! The faults are scheduled events, never ambient filesystem luck: a short
//! write (a mid-frame cut, torn tail), a write whose bytes never landed (an
//! aligned cut, clean), a lying frame (a flipped payload byte, refused with
//! the file byte-identical), a lost directory entry (the file removed,
//! genesis), a clean crash (drop and reopen) and a cross-tenant open (refused).
//! Each fault asserts its own observable effect, so a fault the path never
//! observes fails instead of passing quietly.
//!
//! Four invariants hold after every step of every seed, checked against an
//! independent model that only tracks what the sim kept as acknowledged:
//! no acknowledged append is lost across crash-replay; no tenant reads another
//! tenant's record; a torn tail repairs to the prefix holding every kept
//! record; a second replay changes nothing (bytes, keys and fold identical).
//! Every breach names its seed, step and op, so a failing seed replays by hand.
//!
//! Sources: FoundationDB's simulation testing (one seed replays a run);
//! Pillai et al., "All File Systems Are Not Created Equal" (OSDI '14), whose
//! torn-write shapes are the injected faults; the sealed journal's own torn
//! tail disposition (a killed writer leaves a prefix, open truncates it).

use std::error::Error;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use lgwks_bot::effect::EffectKey;
use lgwks_bot::journal::{EffectJournal as _, FileJournal, JournalError, Recovered};
use lgwks_bot::sealed::keys::KeyProvider;

use crate::scratch::Scratch;
use crate::sealed_harness::{
    FaultingProvider, append_journal_event, assert_journal_chain, journal_event, journal_key,
    live_journal_keys,
};
use crate::sim::{Rng, Trace};
use crate::sweep_fixtures::{FIRST_SEED, SWEEP_SEEDS, peak_rss_bytes, refuse};

type TestResult = Result<(), Box<dyn Error>>;

/// Steps per seed: every op arm fires about once per seed on average.
const STEPS: u32 = 8;
/// Tenants per seed: two, so the cross-tenant fence is always exercised.
const TENANTS: u32 = 2;
/// Wall budget for the 1,024-seed sweep (#819 acceptance: under 60 s).
const SWEEP_BUDGET_SECS: u64 = 60;
/// Worker cap: other sessions share this machine.
const MAX_WORKERS: usize = 4;
/// Resident-set ceiling for the sweep, read from the platform high-water mark
/// where it reports one (the #278 row precedent is 512 MiB for 1,024 seeds).
const RSS_CEILING_BYTES: u64 = 512 * 1024 * 1024;

/// Tenant labels: file-safe, distinct, fixed across seeds.
const TENANT_A: &str = "disk-a";
const TENANT_B: &str = "disk-b";

/// Attempt namespace stride per tenant: the fixture fixes run, action,
/// environment and flow, so the attempt number is what keeps two tenants'
/// keys disjoint.
const ATTEMPT_STRIDE: u64 = 1_000_000;

/// What one seed produced: its trace hash and how much it appended.
struct SeedRun {
    /// The FNV-1a digest of everything the seed recorded.
    hash: u64,
    /// Kept appends across both tenants.
    facts: usize,
}

/// The independent oracle for one tenant: the keys the sim kept as
/// acknowledged, the fold they must replay to, and frame boundaries.
struct TenantModel {
    /// Keys kept as acknowledged, in append order.
    live: Vec<EffectKey>,
    /// File length before each kept frame, parallel to `live`.
    frames: Vec<u64>,
    /// The fold the last check observed; the next check must agree exactly.
    fold: Option<Recovered>,
    /// Next attempt offset inside this tenant's namespace.
    seq: u64,
}

/// The whole seed's oracle: one model per tenant.
struct Model {
    /// Index 0 is A, index 1 is B.
    tenants: [TenantModel; 2],
}

impl Model {
    /// An empty oracle for a fresh seed.
    const fn new() -> Self {
        Self {
            tenants: [TenantModel::new(), TenantModel::new()],
        }
    }
}

impl TenantModel {
    /// An empty tenant oracle.
    const fn new() -> Self {
        Self {
            live: Vec::new(),
            frames: Vec::new(),
            fold: None,
            seq: 0,
        }
    }

    /// The next attempt number in this tenant's namespace.
    fn next_attempt(&mut self, tenant: u64) -> u64 {
        let attempt = tenant
            .saturating_add(1)
            .saturating_mul(ATTEMPT_STRIDE)
            .saturating_add(self.seq);
        self.seq = self.seq.saturating_add(1);
        attempt
    }
}

/// One seed's journals: the directory, the shared provider, both files.
struct SeedCtx {
    /// Scratch directory holding both journal files; removed on drop.
    _dir: Scratch,
    /// The provider both journals open under, shared across reopens.
    provider: Arc<dyn KeyProvider>,
    /// Journal file per tenant.
    paths: [PathBuf; 2],
    /// Whether the next open must report a repaired torn tail, per tenant.
    expect_repair: [bool; 2],
}

/// Both tenants' live handles.
struct Handles {
    /// One open journal per tenant.
    journals: [FileJournal; 2],
}

/// The tenant label for index 0 (A) or 1 (B).
const fn tenant_name(index: usize) -> &'static str {
    if index == 0 { TENANT_A } else { TENANT_B }
}

/// The other tenant's index: the fence is always between the two.
const fn other_tenant(index: usize) -> usize {
    if index == 0 { 1 } else { 0 }
}

/// Where a fault was scheduled: the seed, step and op that name a failure.
#[derive(Clone, Copy)]
struct At {
    /// The seed being swept.
    seed: u64,
    /// The step inside the seed.
    step: u32,
    /// The op inside the step.
    op: u32,
}

/// Refuse with the seed attached, so a failing band names its seed.
fn fail<Outcome>(at: At, detail: String) -> Result<Outcome, Box<dyn Error>> {
    refuse(format!(
        "seed {} step {} op {}: {detail}",
        at.seed, at.step, at.op
    ))
}

/// Everything one op may touch, in one hand: the arms share one calling
/// shape instead of repeating one signature eight times.
struct Op<'a> {
    /// The model the arm keeps.
    model: &'a mut Model,
    /// The trace the arm records to.
    trace: &'a mut Trace,
    /// The seed stream the arm draws from.
    rng: &'a mut Rng,
    /// The journals the arm faults.
    ctx: &'a mut SeedCtx,
    /// Where the fault was scheduled.
    at: At,
    /// Which tenant acts.
    tenant: usize,
}

/// Reopen after a fault, fold the fresh state, and record the arm: every
/// destructive arm ends here rather than repeating the three lines.
fn reopened(op: &mut Op<'_>, name: &str) -> Result<Handles, Box<dyn Error>> {
    let at = op.at;
    let handles = reopen(&mut *op.ctx, at)?;
    sync_fold(&handles, &mut *op.model);
    op.trace.record(name);
    Ok(handles)
}

/// Pop the last kept frame's start, or refuse when the tenant holds nothing.
fn pop_frame(op: &mut Op<'_>) -> Result<u64, Box<dyn Error>> {
    let tenant = op.tenant;
    let at = op.at;
    match op.model.tenants[tenant].frames.pop() {
        Some(frame_start) => Ok(frame_start),
        None => fail(at, format!("tenant {tenant}: the fault needs a kept frame")),
    }
}

/// Open both journals, asserting each repair flag is exactly what the last
/// fault scheduled: a tear reads true, anything else false. A fault the path
/// never observes fails here rather than passing quietly.
fn reopen(ctx: &mut SeedCtx, at: At) -> Result<Handles, Box<dyn Error>> {
    let seed = at.seed;
    let step = at.step;
    let op = at.op;
    let first = FileJournal::open_sealed(&ctx.paths[0], Arc::clone(&ctx.provider), TENANT_A)
        .map_err(|error| -> Box<dyn Error> {
            format!("seed {seed} step {step} op {op} tenant 0: reopen refused: {error}").into()
        })?;
    if first.torn_tail_repaired() != ctx.expect_repair[0] {
        return fail(
            at,
            format!(
                "tenant 0: torn_tail_repaired={}, scheduled={}",
                first.torn_tail_repaired(),
                ctx.expect_repair[0]
            ),
        );
    }
    ctx.expect_repair[0] = false;
    let second = FileJournal::open_sealed(&ctx.paths[1], Arc::clone(&ctx.provider), TENANT_B)
        .map_err(|error| -> Box<dyn Error> {
            format!("seed {seed} step {step} op {op} tenant 1: reopen refused: {error}").into()
        })?;
    if second.torn_tail_repaired() != ctx.expect_repair[1] {
        return fail(
            at,
            format!(
                "tenant 1: torn_tail_repaired={}, scheduled={}",
                second.torn_tail_repaired(),
                ctx.expect_repair[1]
            ),
        );
    }
    ctx.expect_repair[1] = false;
    Ok(Handles {
        journals: [first, second],
    })
}

/// Fold both handles into the model after a mutating op.
fn sync_fold(handles: &Handles, model: &mut Model) {
    for (index, journal) in handles.journals.iter().enumerate() {
        model.tenants[index].fold = Some(journal.recover());
    }
}

/// The live keys of `journal`, naming the seed, step and op on refusal.
fn live_keys(
    journal: &FileJournal,
    at: At,
    tenant: usize,
) -> Result<Vec<EffectKey>, Box<dyn Error>> {
    let seed = at.seed;
    let step = at.step;
    let op = at.op;
    live_journal_keys(journal).map_err(|error| {
        format!("seed {seed} step {step} op {op} tenant {tenant}: live read refused: {error}")
            .into()
    })
}

/// The four-invariant check on the live handles: each tenant replays exactly
/// its kept keys, the chains verify, the folds agree with the last
/// observation, and no key crosses tenants.
fn check(handles: &Handles, model: &mut Model, at: At) -> Result<(), Box<dyn Error>> {
    let seed = at.seed;
    let step = at.step;
    let op = at.op;
    for (index, journal) in handles.journals.iter().enumerate() {
        let got = live_keys(journal, at, index)?;
        let tenant = &mut model.tenants[index];
        if got != tenant.live {
            return refuse(format!(
                "seed {seed} step {step} op {op} tenant {index}: replay holds {} keys, kept {}",
                got.len(),
                tenant.live.len()
            ));
        }
        assert_journal_chain(journal).map_err(|error| -> Box<dyn Error> {
            format!("seed {seed} step {step} op {op} tenant {index}: chain refused: {error}").into()
        })?;
        let fold = journal.recover();
        if let Some(previous) = tenant.fold.as_ref()
            && previous != &fold
        {
            return refuse(format!(
                "seed {seed} step {step} op {op} tenant {index}: a second replay changed the fold"
            ));
        }
        tenant.fold = Some(fold);
    }
    for key in model.tenants[0].live.iter() {
        if model.tenants[1].live.contains(key) {
            return refuse(format!(
                "seed {seed} step {step} op {op}: tenant B holds tenant A's key"
            ));
        }
    }
    Ok(())
}

/// Drop both handles, reopen, and prove recovery is idempotent: the file
/// bytes, the live keys and the fold are identical across the second replay.
fn prove_idempotent(op: &mut Op<'_>, handles: Handles) -> Result<Handles, Box<dyn Error>> {
    let at = op.at;
    let seed = at.seed;
    let step = at.step;
    let op_code = at.op;
    let mut before: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
    for (index, slot) in before.iter_mut().enumerate() {
        *slot = std::fs::read(&op.ctx.paths[index]).map_err(|error| -> Box<dyn Error> {
            format!("seed {seed} step {step} op {op_code} tenant {index}: reread refused: {error}")
                .into()
        })?;
    }
    drop(handles);
    let handles = reopen(&mut *op.ctx, at)?;
    check(&handles, &mut *op.model, at)?;
    for (index, slot) in before.iter().enumerate() {
        let after = std::fs::read(&op.ctx.paths[index])?;
        if after != *slot {
            return refuse(format!(
                "seed {seed} step {step} op {op_code} tenant {index}: a second replay changed the file"
            ));
        }
    }
    op.trace.record("idempotent");
    Ok(handles)
}

/// Append one event and keep it as acknowledged: the one append path the
/// steady-state and post-loss arms share.
fn append_kept(
    handles: &mut Handles,
    op: &mut Op<'_>,
    key: EffectKey,
    before: u64,
    why: &str,
) -> Result<(), Box<dyn Error>> {
    let tenant = op.tenant;
    let at = op.at;
    append_journal_event(&mut handles.journals[tenant], &journal_event(key)).map_err(
        |error| -> Box<dyn Error> {
            format!(
                "seed {} step {} op {} tenant {tenant}: {why} refused: {error}",
                at.seed, at.step, at.op
            )
            .into()
        },
    )?;
    op.model.tenants[tenant].live.push(key);
    op.model.tenants[tenant].frames.push(before);
    sync_fold(handles, &mut *op.model);
    Ok(())
}

/// Append one event for the acting tenant and keep it as acknowledged.
fn op_append(mut handles: Handles, op: &mut Op<'_>) -> Result<Handles, Box<dyn Error>> {
    let tenant = op.tenant;
    let before = match std::fs::metadata(&op.ctx.paths[tenant]) {
        Ok(metadata) => metadata.len(),
        Err(_) => 0,
    };
    let index = u64::try_from(tenant)?;
    let attempt = op.model.tenants[tenant].next_attempt(index);
    let key = journal_key(&attempt.to_string())?;
    append_kept(&mut handles, op, key, before, "append")?;
    op.trace.record("appended");
    Ok(handles)
}

/// A short write: cut the file mid-frame. The new frame never landed whole,
/// so the lost-reply fiction drops it and the reopen must repair to the
/// kept prefix with the flag set.
fn op_torn(handles: Handles, op: &mut Op<'_>) -> Result<Handles, Box<dyn Error>> {
    drop(handles);
    let tenant = op.tenant;
    let at = op.at;
    let file_len = std::fs::metadata(&op.ctx.paths[tenant])?.len();
    let frame_start = pop_frame(op)?;
    let frame_len = file_len.saturating_sub(frame_start);
    if frame_len < 2 {
        return fail(
            at,
            format!("tenant {tenant}: a torn write needs a frame to cut"),
        );
    }
    let span = u32::try_from(frame_len.saturating_sub(1))?;
    let cut = frame_start.saturating_add(u64::from(op.rng.below(span).saturating_add(1)));
    let cut_at = usize::try_from(cut)?;
    let bytes = std::fs::read(&op.ctx.paths[tenant])?;
    if bytes.len() <= cut_at || cut_at == 0 {
        return fail(
            at,
            format!("tenant {tenant}: the seeded cut is outside the file"),
        );
    }
    std::fs::write(&op.ctx.paths[tenant], &bytes[..cut_at])?;
    op.model.tenants[tenant].live.pop();
    op.ctx.expect_repair[tenant] = true;
    reopened(op, "torn")
}

/// A write whose bytes never landed: cut the file to the frame boundary. The
/// frame is gone whole, so the reopen reads clean with the flag clear, and
/// the journal proves it tells the two cuts apart.
fn op_lost_write(handles: Handles, op: &mut Op<'_>) -> Result<Handles, Box<dyn Error>> {
    drop(handles);
    let tenant = op.tenant;
    let start_at = usize::try_from(pop_frame(op)?)?;
    let bytes = std::fs::read(&op.ctx.paths[tenant])?;
    if bytes.len() < start_at {
        return fail(
            op.at,
            format!("tenant {tenant}: the frame boundary is outside the file"),
        );
    }
    std::fs::write(&op.ctx.paths[tenant], &bytes[..start_at])?;
    op.model.tenants[tenant].live.pop();
    reopened(op, "lost-write")
}

/// A lying frame: flip the first payload byte of the last frame. The chain
/// cannot re-derive its head, so the open refuses with the file
/// byte-identical; restoring the bytes heals it, which the reopen proves.
fn op_lying(handles: Handles, op: &mut Op<'_>) -> Result<Handles, Box<dyn Error>> {
    drop(handles);
    let tenant = op.tenant;
    let at = op.at;
    let Some(frame_start) = op.model.tenants[tenant].frames.last().copied() else {
        return fail(
            at,
            format!("tenant {tenant}: a lying frame needs a kept frame"),
        );
    };
    let Some(offset) = frame_start.checked_add(4) else {
        return fail(at, format!("tenant {tenant}: the frame offset overflows"));
    };
    let good = std::fs::read(&op.ctx.paths[tenant])?;
    let position = usize::try_from(offset)?;
    if good.len() <= position {
        return fail(
            at,
            format!("tenant {tenant}: the lying offset is outside the file"),
        );
    }
    let mut bad = good.clone();
    bad[position] ^= 0xFF;
    std::fs::write(&op.ctx.paths[tenant], &bad)?;
    match FileJournal::open_sealed(
        &op.ctx.paths[tenant],
        Arc::clone(&op.ctx.provider),
        tenant_name(tenant),
    ) {
        Err(JournalError::Corrupt(_)) => {}
        Err(other) => {
            return fail(
                at,
                format!("tenant {tenant}: a lying frame refused as {other:?}, not Corrupt"),
            );
        }
        Ok(_) => {
            return fail(at, format!("tenant {tenant}: a lying frame opened"));
        }
    }
    let still = std::fs::read(&op.ctx.paths[tenant])?;
    if still != bad {
        return fail(
            at,
            format!("tenant {tenant}: the refused open moved a byte"),
        );
    }
    std::fs::write(&op.ctx.paths[tenant], &good)?;
    reopened(op, "lying-refused")
}

/// A lost directory entry: the file is gone, so the open creates genesis.
/// The loss is declared, the model clears the tenant, and a fresh append
/// proves the journal continues from empty.
fn op_dirent_loss(handles: Handles, op: &mut Op<'_>) -> Result<Handles, Box<dyn Error>> {
    drop(handles);
    let tenant = op.tenant;
    let at = op.at;
    std::fs::remove_file(&op.ctx.paths[tenant])?;
    let mut handles = reopen(&mut *op.ctx, at)?;
    op.model.tenants[tenant].live.clear();
    op.model.tenants[tenant].frames.clear();
    // The file is new, so its fold starts over too: the next check adopts
    // the genesis fold rather than comparing it against the lost history.
    op.model.tenants[tenant].fold = None;
    check(&handles, &mut *op.model, at)?;
    let index = u64::try_from(tenant)?;
    let attempt = op.model.tenants[tenant].next_attempt(index);
    let key = journal_key(&attempt.to_string())?;
    let before = std::fs::metadata(&op.ctx.paths[tenant])?.len();
    append_kept(&mut handles, op, key, before, "post-loss append")?;
    op.trace.record("dirent-lost");
    Ok(handles)
}

/// A clean crash: drop every handle and reopen. Recovery must change nothing.
fn op_crash(handles: Handles, op: &mut Op<'_>) -> Result<Handles, Box<dyn Error>> {
    drop(handles);
    reopened(op, "crash")
}

/// Re-append the last kept event at the current tail. The ladder admits each
/// rung once, so the duplicate is refused and the journal does not move.
fn op_reappend(mut handles: Handles, op: &mut Op<'_>) -> Result<Handles, Box<dyn Error>> {
    let tenant = op.tenant;
    let at = op.at;
    let Some(key) = op.model.tenants[tenant].live.last().copied() else {
        op.trace.record("reappend-skipped");
        return Ok(handles);
    };
    let tail = handles.journals[tenant].tail();
    match handles.journals[tenant].compare_and_append_sealed(tail, &journal_event(key), None) {
        Err(JournalError::OutOfOrder { .. }) => {}
        Err(other) => {
            return fail(
                at,
                format!("tenant {tenant}: a duplicate refused as {other:?}, not OutOfOrder"),
            );
        }
        Ok(_) => {
            return fail(at, format!("tenant {tenant}: a duplicate append landed"));
        }
    }
    sync_fold(&handles, &mut *op.model);
    op.trace.record("reappend-refused");
    Ok(handles)
}

/// Open this tenant's file under the other tenant's label. The frames name
/// another namespace, so the open refuses and the file stays byte-identical:
/// no read returns another tenant's record.
fn op_wrong_tenant(handles: Handles, op: &mut Op<'_>) -> Result<Handles, Box<dyn Error>> {
    drop(handles);
    let tenant = op.tenant;
    let at = op.at;
    let peer = other_tenant(tenant);
    let good = std::fs::read(&op.ctx.paths[tenant])?;
    let live = !op.model.tenants[tenant].live.is_empty();
    match FileJournal::open_sealed(
        &op.ctx.paths[tenant],
        Arc::clone(&op.ctx.provider),
        tenant_name(peer),
    ) {
        Err(JournalError::Corrupt(_)) => {
            if !live {
                return fail(
                    at,
                    format!("tenant {tenant}: an empty file refused a foreign open"),
                );
            }
        }
        Err(other) => {
            return fail(
                at,
                format!("tenant {tenant}: a foreign open refused as {other:?}, not Corrupt"),
            );
        }
        Ok(foreign) => {
            if live {
                return fail(
                    at,
                    format!("tenant {tenant}: a foreign tenant read the file"),
                );
            }
            // An empty file names no namespace, so the open succeeds — and
            // the fence is that it reads nothing at all.
            let got = live_journal_keys(&foreign).map_err(|error| -> Box<dyn Error> {
                format!(
                    "seed {} step {} op {} tenant {tenant}: foreign live read refused: {error}",
                    at.seed, at.step, at.op
                )
                .into()
            })?;
            if !got.is_empty() {
                return fail(
                    at,
                    format!("tenant {tenant}: a foreign tenant read a record from an empty file"),
                );
            }
            if foreign.torn_tail_repaired() {
                return fail(
                    at,
                    format!("tenant {tenant}: a foreign open of an empty file reported a repair"),
                );
            }
            drop(foreign);
        }
    }
    let still = std::fs::read(&op.ctx.paths[tenant])?;
    if still != good {
        return fail(
            at,
            format!("tenant {tenant}: the refused foreign open moved a byte"),
        );
    }
    reopened(op, "foreign-refused")
}

/// Apply one seeded op, then prove the replay idempotent before the next
/// step. The proof replays and checks every invariant, so one verification
/// covers the step instead of two.
fn apply_step(
    handles: Handles,
    model: &mut Model,
    trace: &mut Trace,
    rng: &mut Rng,
    ctx: &mut SeedCtx,
    seed: u64,
    step: u32,
) -> Result<Handles, Box<dyn Error>> {
    let tenant = usize::try_from(rng.below(TENANTS))?;
    let op_code = rng.below(100);
    let at = At {
        seed,
        step,
        op: op_code,
    };
    trace.record_number("op", op_code);
    let tenant_number = u64::try_from(tenant)?;
    trace.record_number("tenant", tenant_number);
    let mut op = Op {
        model,
        trace,
        rng,
        ctx,
        at,
        tenant,
    };
    let live = !op.model.tenants[tenant].live.is_empty();
    let handles = match op_code {
        0..=39 => op_append(handles, &mut op)?,
        40..=54 => op_crash(handles, &mut op)?,
        55..=64 if live => op_torn(handles, &mut op)?,
        65..=74 if live => op_lost_write(handles, &mut op)?,
        75..=82 if live => op_lying(handles, &mut op)?,
        83..=89 => op_dirent_loss(handles, &mut op)?,
        90..=94 if live => op_reappend(handles, &mut op)?,
        95..=99 => op_wrong_tenant(handles, &mut op)?,
        _ => op_append(handles, &mut op)?,
    };
    let handles = prove_idempotent(&mut op, handles)?;
    for tenant_model in op.model.tenants.iter() {
        let held = u64::try_from(tenant_model.live.len())?;
        op.trace.record_number("live", held);
    }
    Ok(handles)
}

/// One seed, start to finish: eight scheduled ops over two sealed journals.
fn run_seed(seed: u64) -> Result<SeedRun, Box<dyn Error>> {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    trace.record_number("seed", seed);
    let dir = Scratch::new("sealed-disk")?;
    let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(dir.path())?);
    let mut ctx = SeedCtx {
        paths: [dir.path().join("a.jrnl"), dir.path().join("b.jrnl")],
        provider,
        expect_repair: [false, false],
        _dir: dir,
    };
    let mut model = Model::new();
    let zero = At {
        seed,
        step: 0,
        op: 0,
    };
    let mut handles = reopen(&mut ctx, zero)?;
    sync_fold(&handles, &mut model);
    check(&handles, &mut model, zero)?;
    for step in 1..=STEPS {
        handles = apply_step(
            handles, &mut model, &mut trace, &mut rng, &mut ctx, seed, step,
        )
        .map_err(|error| -> Box<dyn Error> {
            format!("seed {seed} step {step}: {error}").into()
        })?;
    }
    drop(handles);
    let facts: usize = model.tenants.iter().map(|tenant| tenant.live.len()).sum();
    trace.record_number("facts", u64::try_from(facts)?);
    Ok(SeedRun {
        hash: trace.hash(),
        facts,
    })
}

/// Run one seed twice from the seed and require the same trace hash.
fn replay_seed(seed: u64) -> Result<SeedRun, String> {
    let first = run_seed(seed).map_err(|error| format!("seed {seed} first run: {error}"))?;
    let second = run_seed(seed).map_err(|error| format!("seed {seed} second run: {error}"))?;
    if first.hash != second.hash {
        let refusal = format!(
            "seed {seed}: two runs hashed {:016x} and {:016x}",
            first.hash, second.hash
        );
        lgwks_std::trace::debug!(error = %refusal, seed, "sim_sealed_disk: one seed replayed to two traces");
        return Err(refusal);
    }
    if first.facts != second.facts {
        let refusal = format!(
            "seed {seed}: two runs kept {} and {} facts",
            first.facts, second.facts
        );
        lgwks_std::trace::debug!(error = %refusal, seed, "sim_sealed_disk: one seed replayed to two fact counts");
        return Err(refusal);
    }
    Ok(first)
}

/// Pull seeds from a shared counter until the sweep is exhausted. The error
/// is a string so the seed text crosses the worker boundary: `Box<dyn Error>`
/// is not `Send`, and a sweep error that cannot name its seed is decorative.
fn sweep_worker(next: &AtomicUsize) -> Result<(usize, usize, u64), String> {
    let bound = usize::try_from(SWEEP_SEEDS).map_err(|error| format!("sweep bound: {error}"))?;
    let mut seeds = 0usize;
    let mut facts = 0usize;
    let mut fingerprint: u64 = 0;
    loop {
        let index = next.fetch_add(1, Ordering::Relaxed);
        if index >= bound {
            return Ok((seeds, facts, fingerprint));
        }
        let seed = FIRST_SEED
            .saturating_add(u64::try_from(index).map_err(|error| format!("sweep index: {error}"))?);
        let run = replay_seed(seed)?;
        seeds = seeds.saturating_add(1);
        facts = facts.saturating_add(run.facts);
        fingerprint ^= run.hash;
    }
}

/// Write one measured line where `--nocapture` and a success-output override
/// show it (the `process_fire` precedent: stderr, never stdout).
fn report(line: &str) {
    let mut sink = std::io::stderr().lock();
    if let Err(error) = writeln!(sink, "sim_sealed_disk: {line}") {
        lgwks_std::trace::debug!(%error, "sim_sealed_disk: the measurement line was not written");
    }
}

/// The same seed replays the same trace hash, and distinct seeds diverge.
#[test]
fn sealed_disk_same_seed_same_trace() -> TestResult {
    for seed in [1u64, 7, 0xDEAD_BEEF, 819] {
        let first = replay_seed(seed).map_err(|failure| -> Box<dyn Error> { failure.into() })?;
        let second = replay_seed(seed).map_err(|failure| -> Box<dyn Error> { failure.into() })?;
        if first.hash != second.hash {
            return refuse(format!("seed {seed}: two runs hashed different traces"));
        }
    }
    let one = replay_seed(FIRST_SEED).map_err(|failure| -> Box<dyn Error> { failure.into() })?;
    let two = replay_seed(FIRST_SEED.saturating_add(1))
        .map_err(|failure| -> Box<dyn Error> { failure.into() })?;
    if one.hash == two.hash {
        return refuse("distinct seeds collapsed to one trace");
    }
    Ok(())
}

/// The #819 acceptance: 1,024 seeds, every step checked, under 60 s with
/// bounded peak RSS, both printed by the test.
#[test]
fn sealed_disk_seed_sweep() -> TestResult {
    let workers = match std::thread::available_parallelism() {
        Ok(count) => count.get().clamp(1, MAX_WORKERS),
        Err(_) => 1,
    };
    let next = AtomicUsize::new(0);
    let mut seeds = 0usize;
    let mut facts = 0usize;
    let mut fingerprint: u64 = 0;
    let started = Instant::now();
    std::thread::scope(|scope| -> Result<(), Box<dyn Error>> {
        let mut handles = Vec::new();
        for _ in 0..workers {
            handles.push(scope.spawn(|| sweep_worker(&next)));
        }
        for handle in handles {
            let done = match handle.join() {
                Ok(worker) => worker?,
                Err(_) => {
                    return refuse("a sweep worker stopped without answering");
                }
            };
            seeds = seeds.saturating_add(done.0);
            facts = facts.saturating_add(done.1);
            fingerprint ^= done.2;
        }
        Ok(())
    })?;
    let elapsed = started.elapsed();
    let total = usize::try_from(SWEEP_SEEDS)?;
    if seeds != total {
        return refuse(format!("the sweep ran {seeds} of {SWEEP_SEEDS} seeds"));
    }
    let rss = peak_rss_bytes();
    if let Some(peak) = rss
        && peak >= RSS_CEILING_BYTES
    {
        return refuse(format!(
            "the 1,024-seed sweep peaked at {peak} bytes, past {RSS_CEILING_BYTES}"
        ));
    }
    let rss_text = match rss {
        Some(peak) => format!("{peak} bytes"),
        None => "unreported (the platform has no high-water mark)".to_owned(),
    };
    report(&format!(
        "{} seeds x {STEPS} steps x {TENANTS} tenants, {facts} kept appends in {:.2}s on {workers} workers, peak rss {rss_text}, trace fingerprint {fingerprint:016x}",
        SWEEP_SEEDS,
        elapsed.as_secs_f64()
    ));
    if elapsed.as_secs() >= SWEEP_BUDGET_SECS {
        return refuse(format!(
            "the 1,024-seed sweep took {elapsed:?}, past {SWEEP_BUDGET_SECS}s"
        ));
    }
    Ok(())
}
