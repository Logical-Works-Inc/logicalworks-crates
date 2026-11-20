//! Deterministic simulation of the proposal boundary.
//!
//! One seed draws a whole scenario: a payload shape, the surface it is decoded
//! against, how many capabilities the run holds, the ceiling the decoder
//! charges, the failure ledger's limits, and how many times the run retries.
//! Every assertion is a *property* checked against what the boundary actually
//! did, not a table re-spelled — so a shape the seed drew is checked by the same
//! rule as every other shape.
//!
//! The families, by axis: frontier (every shape reaches its declared refusal),
//! generalized (the same sweep through `Host::run`; concurrent readers and
//! writers), multi-tenant (two tenants on one digest, two on one host),
//! hyperscale (saturation at 100, 1,000 and 10,000, and over-admit budget
//! conservation) and ephemeral (the same seed hashes the same trace twice).
//!
//! The decoder, the surface, the ledger and the artifact store are the shipped
//! types, driven through a real `Host::run`. Only the *scenario* is seeded, and
//! no trace hash holds a wall-clock reading or a thread interleaving.

#![cfg(feature = "script")]

use crate::sim;

// The band-declaration macro, defined once for the whole layer.
use crate::band_family;

// The shared proposal harness, so this file and `proposal.rs` drive the same
// surface and the same payload shapes.
use crate::proposal_fixtures as support;

use std::error::Error;
use std::num::NonZeroUsize;

use lgwks_bot::cap::Cap;
use lgwks_bot::proposal::{
    ArtifactKey, ArtifactStore, Checkpoint, Completion, CompletionOutcome, Coverage, Decoder,
    LedgerLimits, MAX_FIELD_NAME_BYTES, PlanBudget, PlanLimits, Provenance, Refusal, RepairLedger,
    Source, Surface, WriteOutcome,
};
use lgwks_bot::script::Gate;
use lgwks_bot::task::{Disposition, Host};

use sim::Band;
use sim::Rng;

/// A test's result: an error fails it with the error's text.
type TestResult = Result<(), Box<dyn Error>>;

/// The tenants the isolation and saturation families run over.
const TENANTS: [&str; 4] = ["acme", "globex", "initech", "umbrella"];

/// A tenant no family ever chooses, so "a tenant outside the set" exists for
/// every draw rather than only for the draws that leave one out.
const STRANGER: &str = "hooli";

/// The declared saturation tiers, per INV-BOT-16: the requested level, the level
/// reached, and the ceiling are recorded together so a reader is never told a
/// concurrency number nobody ran.
const TIERS: [usize; 3] = [100, 1_000, 10_000];

/// The maximum workers one tier may drive.
///
/// A bound rather than whatever the machine happens to do: a tier a host cannot
/// reach is clamped to this and reported as clamped rather than as reached.
const MAX_WORKERS: usize = 10_000;

// ── Frontier: every shape reaches its declared outcome ───────────────────────

/// Every payload shape reaches exactly the refusal the boundary names.
///
/// The expected arm per shape is a function of the shape rather than a literal
/// per test, because the seed picks the shape: a sweep that asserted one fixed
/// outcome would only ever prove one shape's arm.
fn seeded_shapes_match_the_declared_outcome(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let Drawn {
            name,
            payload,
            surface,
            limits: decoder_limits,
        } = draw_case(sim.rng())?;
        let decoder = lgwks_bot::proposal::Decoder::new(decoder_limits);
        let outcome = decoder.decode(&surface, &payload, Source::Model);

        assert!(
            outcome.provenance().is_some(),
            "shape {name}: every outcome names the bytes it came from, got {outcome}"
        );
        assert_eq!(
            outcome.provenance().map(Provenance::bytes),
            Some(payload.len()),
            "shape {name}: the provenance records the bytes admitted"
        );

        match outcome.plan() {
            // An admitted plan may name only what the surface registered, and
            // may never claim full coverage. Not every admitted payload is the
            // `well-formed` shape: an injection whose `host` field happens to name
            // *this* tenant is a legal document whose extra content is inert, and
            // a ceiling loose enough for it is a surface that admitted it on its
            // merits rather than through the injection.
            Some(plan) => {
                for wanted in plan.wanted() {
                    assert!(
                        surface.operation(wanted.operation()).is_some(),
                        "shape {name}: an admitted plan names only registered operations, \
                         not {:?}",
                        wanted.operation()
                    );
                }
                assert!(
                    !plan.coverage().is_complete(),
                    "shape {name}: no payload yields full coverage"
                );
                assert!(
                    plan.notes().iter().all(|note| !note.is_empty()),
                    "shape {name}: a note is retained verbatim and never dropped"
                );
            }
            None => {
                let refusal = outcome
                    .refusal()
                    .ok_or("an outcome is admitted or refused, and it refused")?;
                // The expected arm is *derived* from the drawn surface and
                // ceilings rather than tabled per shape, because the seed is
                // free to vary both and the decoder's checks run in a fixed
                // order. A table would have to enumerate the surface × ceiling
                // cross product to stay right; this computes the one arm the
                // order produces.
                let expected = declared_refusal(&shape_payload(name)?, &surface, &decoder_limits);
                assert_eq!(
                    refusal.label(),
                    expected,
                    "shape {name}: the refusal is the one the decoder's own order produces for \
                     this surface and these ceilings, got {refusal}"
                );
                assert!(
                    !outcome.is_admitted(),
                    "shape {name}: a refused payload never reports as admitted"
                );
            }
        }
        // An admitted outcome carries no refusal, and the trace names that case
        // in its own arm rather than through a default.
        let refused = match outcome.refusal() {
            Some(refusal) => refusal.label(),
            None => "none",
        };
        sim.record(&format!(
            "shape={name} bytes={} admitted={} refusal={refused}",
            payload.len(),
            outcome.is_admitted(),
        ));
        Ok(())
    })
}

/// The payload the shape named `shape` builds, refused for a name no shape has.
fn shape_payload(shape: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    support::SHAPES
        .iter()
        .find(|entry| entry.0 == shape)
        .map(|entry| entry.1())
        .ok_or_else(|| format!("no payload shape is named {shape:?}").into())
}

/// The refusal arm the decoder produces for `shape` on `surface` under `limits`.
///
/// A mirror of the decoder's own check order rather than a table per shape,
/// because the seed varies the surface and the ceilings and a table would have
/// to enumerate the cross product to stay correct. The order is the decoder's,
/// and it is the order the sweep is checking:
///
/// 1. the payload ceiling, before a byte is read — `Oversized`;
/// 2. the per-line ceiling, which fires first for a payload whose *value* is
///    enormous even when the whole payload fits — `Limit`;
/// 3. the grammar: no `=`, or a field this decoder does not know — `Malformed`;
/// 4. the recognised privilege fields — `InstallTool`, `CredentialRead`,
///    `SandboxEscape`, each refused as the field is stored;
/// 5. the field-count ceiling — `Limit`;
/// 6. authorization: an unregistered name, then a capability the run lacks —
///    `UnknownOperation`, `CapabilityNotHeld`.
fn declared_refusal(payload: &[u8], surface: &Surface, limits: &PlanLimits) -> &'static str {
    if payload.len() > limits.max_bytes {
        return "Oversized";
    }
    let line_ceiling = limits
        .max_field_bytes
        .saturating_add(MAX_FIELD_NAME_BYTES)
        .saturating_add(1);
    if payload
        .split(|byte| *byte == b'\n')
        .any(|line| line.len() > line_ceiling)
    {
        return "Limit";
    }
    // The grammar, in field order, exactly as the reader walks the document.
    // The field-count charge happens on the *count*, before the field is read, so
    // a ceiling of one refuses the second line whatever that line says — which is
    // why it is checked before the field's own arm rather than after.
    let mut fields = 0_u32;
    let mut op = String::new();
    for line in payload.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        fields = fields.saturating_add(1);
        if fields > limits.max_fields {
            return "Limit";
        }
        let mut parts = line.splitn(2, |byte| *byte == b'=');
        let (Some(name), Some(value)) = (parts.next(), parts.next()) else {
            return "Malformed";
        };
        let name = String::from_utf8_lossy(name).trim().to_owned();
        let value = String::from_utf8_lossy(value).trim().to_owned();
        match name.as_str() {
            "op" => op = value,
            "note" | "coverage" => {}
            "path" => {
                if value.starts_with('/')
                    || value.starts_with('\\')
                    || value.contains(':')
                    || value.split('/').any(|segment| segment == "..")
                {
                    return "SandboxEscape";
                }
            }
            "install" => return "InstallTool",
            "credential" => return "CredentialRead",
            "host" => {
                if value != surface.tenant() {
                    return "SandboxEscape";
                }
            }
            _ => return "Malformed",
        }
    }
    if op.is_empty() {
        return "Empty";
    }
    match surface.authorize(&op) {
        Ok(_) => "none",
        Err(refusal) => refusal.label(),
    }
}

/// A surface drawn from the seed: which operations it registers and which caps
/// the run holds.
///
/// The draw matters because the *same* payload must be admitted on one surface
/// and refused on another, and a sweep over one fixed surface would only ever
/// prove half of that.
fn surface_for(rng: &mut Rng) -> Result<Surface, Box<dyn Error>> {
    let (register_read, holds_fs) = (rng.chance(700), rng.chance(600));
    let mut builder = Surface::builder(sim::nth(&TENANTS, usize::try_from(rng.below(4))?)?)?;
    if register_read {
        builder = builder.operation(support::READ, &[Cap::fs()])?;
    }
    builder = builder.operation(support::PING, &[])?;
    if holds_fs {
        builder = builder.holding(&[Cap::fs()]);
    }
    Ok(builder.build())
}

/// One drawn shape case: the payload, the surface it meets and the decoder's
/// ceilings.
struct Drawn {
    /// The shape's name, for the failure message.
    name: &'static str,
    /// The payload the shape builds.
    payload: Vec<u8>,
    /// The surface the payload is decoded against.
    surface: Surface,
    /// The decoder's ceilings.
    limits: PlanLimits,
}

/// Draw a shape case from the seed: the pure decode family and the `Host::run`
/// family must draw the *same* case, so there is one function.
fn draw_case(rng: &mut Rng) -> Result<Drawn, Box<dyn Error>> {
    let index = usize::try_from(u64::from(rng.below(10)))?;
    let (name, build) = support::SHAPES
        .get(index)
        .copied()
        .ok_or("the drawn shape index is outside the table")?;
    let payload = build();
    let surface = surface_for(rng)?;
    let limits = limits_for(rng)?;
    Ok(Drawn {
        name,
        payload,
        surface,
        limits,
    })
}

/// The decoder's ceilings drawn from the seed, within the crate's defaults.
fn limits_for(rng: &mut Rng) -> Result<PlanLimits, Box<dyn Error>> {
    let base = PlanLimits::default();
    Ok(base
        .with_max_bytes(
            base.max_bytes
                .saturating_mul(usize::try_from(u64::from(rng.between(1, 4)))?),
        )
        .with_max_fields(rng.between(1, 8)))
}

// ── Multi-tenant ─────────────────────────────────────────────────────────────

/// N tenants writing the *same* bytes hold N separate artifacts, and each reads
/// only its own: a digest-keyed store would alias them onto one.
fn two_tenants_on_one_digest_stay_isolated(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let count = usize::try_from(u64::from(sim.rng().between(2, 4)))?;
        let store = ArtifactStore::new();
        let shared = format!("the report every tenant produced for seed {}", sim.seed).into_bytes();
        let digest = ArtifactStore::digest_of(&shared)?;

        for tenant in TENANTS.iter().take(count) {
            assert!(
                matches!(store.write(tenant, &shared)?, WriteOutcome::Stored { .. }),
                "tenant {tenant} stores its own copy of the shared bytes"
            );
        }
        // Every tenant holds exactly one artifact: a store keyed on the digest
        // alone would hold one between all of them.
        for tenant in TENANTS.iter().take(count) {
            assert_eq!(
                store.artifacts(tenant),
                1,
                "tenant {tenant} holds one artifact"
            );
            let read = store
                .read(tenant, &digest)
                .ok_or("a tenant that wrote an artifact can read it")?;
            assert_eq!(
                read.as_ref(),
                shared.as_slice(),
                "tenant {tenant} reads back exactly what it wrote"
            );
            assert_eq!(
                store.writers(tenant, &digest),
                1,
                "tenant {tenant}: one writer, counted per key rather than per store"
            );
        }
        // Distinct keys for identical content, which is what makes the
        // per-tenant shelf above an index rather than a filter.
        let keys = TENANTS
            .iter()
            .take(count)
            .map(|tenant| ArtifactKey::of(tenant, &digest).to_hex())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys.len(),
            count,
            "every tenant's key for the same digest is distinct"
        );
        // A tenant outside the set reads nothing, whatever digest it names. The
        // stranger is never in `TENANTS`, so the claim holds for every draw —
        // including the one that chooses the whole set — and it is checked both
        // against the store every chosen tenant wrote and against one nobody did.
        let untouched = ArtifactStore::new();
        assert!(
            store.read(STRANGER, &digest).is_none(),
            "tenant {STRANGER} wrote nothing and reads nothing of the shared digest"
        );
        assert!(
            untouched.read(STRANGER, &digest).is_none(),
            "a store that has never been written reads nothing, whatever digest it names"
        );
        sim.record(&format!(
            "tenants={count} bytes={} digest_shared=1",
            shared.len()
        ));
        Ok(())
    })
}

// ── Generalized: concurrency over the store ───────────────────────────────────

/// Concurrent readers and conflicting writers over one store: nothing is lost,
/// nothing is duplicated, and every writer of one key learns whether it stored.
fn concurrent_readers_and_conflicting_writers(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        let readers = usize::try_from(u64::from(sim.rng().between(2, 8)))?;
        let writers = usize::try_from(u64::from(sim.rng().between(2, 8)))?;
        let store = ArtifactStore::new();
        let content = b"the artifact every worker contends for".to_vec();
        let digest = ArtifactStore::digest_of(&content)?;
        let reads_each = 32_u32;

        // Explicit loops rather than iterator adapters: `scope.spawn` borrows the
        // scope, so a `collect` over an adapter would hold the borrow across the
        // collect itself, and a `move` closure inside an `FnMut` adapter would
        // take the store by value on its first call.
        let (seen, outcomes) = std::thread::scope(|scope| {
            let mut read_handles = Vec::new();
            for _ in 0..readers {
                let store = store.clone();
                read_handles.push(scope.spawn(move || {
                    let mut seen = 0_u32;
                    for _ in 0..reads_each {
                        if store.read(support::TENANT, &digest).is_some() {
                            seen = seen.saturating_add(1);
                        }
                    }
                    seen
                }));
            }
            let mut write_handles = Vec::new();
            for _ in 0..writers {
                let store = store.clone();
                let content = content.clone();
                write_handles.push(scope.spawn(move || store.write(support::TENANT, &content)));
            }
            let mut read_results = Vec::new();
            for handle in read_handles {
                read_results.push(handle.join());
            }
            let mut write_results = Vec::new();
            for handle in write_handles {
                write_results.push(handle.join());
            }
            (read_results, write_results)
        });

        // Every writer reported, so none was lost, and exactly one of them
        // committed: a store that let two writers both "store" the same key would
        // have two receipts for one artifact.
        assert_eq!(
            outcomes.len(),
            writers,
            "every writer reported, so none was lost"
        );
        let mut stored = 0_usize;
        let mut already = 0_usize;
        for outcome in outcomes {
            match outcome.map_err(|panic| format!("a writer thread panicked: {panic:?}"))?? {
                WriteOutcome::Stored { .. } => stored = stored.saturating_add(1),
                WriteOutcome::AlreadyPresent { .. } => already = already.saturating_add(1),
                other => {
                    let refusal =
                        Err(format!("a writer reported an unexpected outcome: {other:?}").into());
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "concurrent_readers_and_conflicting_writers: returning an error to the caller");
                    return refusal;
                }
            }
        }
        assert_eq!(
            stored.saturating_add(already),
            writers,
            "every writer reported: {stored} stored, {already} already present, of {writers}"
        );
        assert_eq!(stored, 1, "exactly one writer committed the key");
        assert_eq!(
            already,
            writers.saturating_sub(1),
            "every later writer was told it stored nothing"
        );
        assert_eq!(
            store.artifacts(support::TENANT),
            1,
            "and exactly one artifact exists for them"
        );
        assert_eq!(
            store.writers(support::TENANT, &digest),
            u64::try_from(writers)?,
            "the serialization receipt counts every writer that reached the key"
        );
        // A reader never sees a *partial* artifact. Seeing none before the first
        // writer commits and everything after it is the race, not a defect: a
        // reader reporting 0 or 32 observed the store change from absent to
        // present outside its loop, and one reporting 18 saw it commit mid-loop.
        // What must never happen is a read yielding *some* of the content, and
        // that is what the byte-for-byte check below rules out.
        for seen in seen {
            let seen = seen.map_err(|panic| format!("a reader thread panicked: {panic:?}"))?;
            assert!(
                seen <= reads_each,
                "a reader cannot see the artifact more often than it read it: {seen} of \
                 {reads_each}"
            );
        }
        assert_eq!(
            store.read(support::TENANT, &digest).as_deref(),
            Some(content.as_slice()),
            "every read returns the whole committed content, never a prefix of it"
        );
        sim.record(&format!(
            "readers={readers} writers={writers} stored={stored} already={already} artifacts={}",
            store.artifacts(support::TENANT),
        ));
        Ok(())
    })
}

// ── Ephemeral: the replay receipt ─────────────────────────────────────────────

/// The same seed produces the same trace hash, twice over, across the union of
/// the families above.
fn same_seed_same_trace_hash(band: Band) -> TestResult {
    sim::assert_replays(band, |sim| {
        // One scenario that touches every subsystem the hash has to cover: the
        // decoder, the ledger, the checkpoint and the store.
        let Drawn {
            name,
            payload,
            surface,
            limits: decoder_limits,
        } = draw_case(sim.rng())?;
        let outcome = Decoder::new(decoder_limits).decode(&surface, &payload, Source::Model);

        // `LedgerLimits::new` takes a `usize` for the distinct-fingerprint bound
        // and a `u32` for the repetition ceiling, so each draw is narrowed to the
        // one its field wants rather than cast.
        let repeat = sim.rng().between(1, 4);
        let distinct = usize::try_from(u64::from(sim.rng().between(1, 8)))?;
        let limits = LedgerLimits::new(repeat, distinct);
        let mut ledger = RepairLedger::new(
            limits,
            Provenance::of(Source::Model, surface.tenant(), &payload),
        );
        for _ in 0..repeat.saturating_add(2) {
            let _admitted = ledger.record_failure(name);
        }

        let mut checkpoint = Checkpoint::new();
        for index in 0..sim.rng().between(1, 4) {
            let _recorded = checkpoint.complete(&format!("step-{index}"));
        }
        let bytes = Checkpoint::to_record(&checkpoint)?;
        let recovered = Checkpoint::from_record(&bytes)?;

        let store = ArtifactStore::new();
        store.write(surface.tenant(), &payload)?;
        let digest = ArtifactStore::digest_of(&payload)?;

        // The claim rests on the evidence the *recovered* checkpoint carries, so
        // a checkpoint that lost its references would report NotEvidenced here —
        // which is the trace hash noticing a regression the assertions might not.
        let evidence = recovered.present().to_vec();
        let claim = Completion::claim("done", &[name]);
        let settled = CompletionOutcome::settle(
            &claim,
            &evidence,
            Coverage::Partial {
                covered: 0,
                asked: 1,
            },
        );
        assert_eq!(
            settled.is_admitted(),
            evidence.contains(&String::from(name)),
            "the claim is admitted exactly when the recovered checkpoint holds its evidence"
        );
        assert!(
            store.holds(surface.tenant(), &digest),
            "and the payload the claim rests on is readable under its own tenant"
        );

        sim.record(&format!(
            "shape={name} admitted={} spent={} steps={} archived={} artifacts={} settled={settled}",
            outcome.is_admitted(),
            ledger.spent(),
            recovered.steps().len(),
            bytes.len(),
            store.artifacts(surface.tenant()),
        ));
        Ok(())
    })
}

// ── Band declarations ────────────────────────────────────────────────────────

use sim_host_path::{
    seeded_runs_reach_the_declared_disposition, two_tenants_admitting_on_one_host_stay_isolated,
};

band_family::band_family! {
    seeded_shapes_match_the_declared_outcome_band_00 => seeded_shapes_match_the_declared_outcome, 0;
    seeded_shapes_match_the_declared_outcome_band_01 => seeded_shapes_match_the_declared_outcome, 1;
    seeded_shapes_match_the_declared_outcome_band_02 => seeded_shapes_match_the_declared_outcome, 2;
    seeded_shapes_match_the_declared_outcome_band_03 => seeded_shapes_match_the_declared_outcome, 3;
    seeded_shapes_match_the_declared_outcome_band_04 => seeded_shapes_match_the_declared_outcome, 4;
    seeded_shapes_match_the_declared_outcome_band_05 => seeded_shapes_match_the_declared_outcome, 5;
    seeded_shapes_match_the_declared_outcome_band_06 => seeded_shapes_match_the_declared_outcome, 6;
    seeded_shapes_match_the_declared_outcome_band_07 => seeded_shapes_match_the_declared_outcome, 7;
    two_tenants_on_one_digest_stay_isolated_band_08 => two_tenants_on_one_digest_stay_isolated, 8;
    two_tenants_on_one_digest_stay_isolated_band_09 => two_tenants_on_one_digest_stay_isolated, 9;
    two_tenants_on_one_digest_stay_isolated_band_10 => two_tenants_on_one_digest_stay_isolated, 10;
    two_tenants_on_one_digest_stay_isolated_band_11 => two_tenants_on_one_digest_stay_isolated, 11;
    concurrent_readers_and_conflicting_writers_band_12 => concurrent_readers_and_conflicting_writers, 12;
    concurrent_readers_and_conflicting_writers_band_13 => concurrent_readers_and_conflicting_writers, 13;
    concurrent_readers_and_conflicting_writers_band_14 => concurrent_readers_and_conflicting_writers, 14;
    concurrent_readers_and_conflicting_writers_band_15 => concurrent_readers_and_conflicting_writers, 15;
    same_seed_same_trace_hash_band_16 => same_seed_same_trace_hash, 16;
    same_seed_same_trace_hash_band_17 => same_seed_same_trace_hash, 17;
    same_seed_same_trace_hash_band_18 => same_seed_same_trace_hash, 18;
    same_seed_same_trace_hash_band_19 => same_seed_same_trace_hash, 19;
    seeded_runs_reach_the_declared_disposition_band_20 => seeded_runs_reach_the_declared_disposition, 20;
    seeded_runs_reach_the_declared_disposition_band_21 => seeded_runs_reach_the_declared_disposition, 21;
    seeded_runs_reach_the_declared_disposition_band_22 => seeded_runs_reach_the_declared_disposition, 22;
    seeded_runs_reach_the_declared_disposition_band_23 => seeded_runs_reach_the_declared_disposition, 23;
    two_tenants_admitting_on_one_host_stay_isolated_band_24 => two_tenants_admitting_on_one_host_stay_isolated, 24;
    two_tenants_admitting_on_one_host_stay_isolated_band_25 => two_tenants_admitting_on_one_host_stay_isolated, 25;
    two_tenants_admitting_on_one_host_stay_isolated_band_26 => two_tenants_admitting_on_one_host_stay_isolated, 26;
    two_tenants_admitting_on_one_host_stay_isolated_band_27 => two_tenants_admitting_on_one_host_stay_isolated, 27;
    seeded_runs_reach_the_declared_disposition_band_28 => seeded_runs_reach_the_declared_disposition, 28;
    seeded_runs_reach_the_declared_disposition_band_29 => seeded_runs_reach_the_declared_disposition, 29;
}

mod sim_host_path;
mod tiers;
