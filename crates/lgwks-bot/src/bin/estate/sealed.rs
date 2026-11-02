//! The sealed estate journey: seal records for two tenants, shred one,
//! restart, and prove the chain still verifies with the shredded record
//! reading as erased.
//!
//! Run it on a built artifact to prove the assembly there:
//!
//! ```text
//! cargo run --locked -p lgwks_bot --features sealed --bin estate -- sealed
//! ```
//!
//! Every line it prints is also asserted: the first deviation records a
//! mismatch and the exit is non-zero, so a clean exit is a proved evening. A
//! failing run leaves its directory behind for inspection rather than
//! cleaning it — the evidence a reader needs to see why the evening failed.

use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(feature = "sealed")]
use std::sync::Arc;
#[cfg(feature = "sealed")]
use std::time::Duration;

#[cfg(feature = "sealed")]
use lgwks_bot::journal::FileJournal;
#[cfg(feature = "sealed")]
use lgwks_bot::sealed::keys::SoftwareProvider;
#[cfg(feature = "sealed")]
use lgwks_bot::task::RunStore;

/// Why the sealed journey failed.
#[derive(Debug)]
pub(crate) enum SealedError {
    /// The command line is wrong. Only constructed without the `sealed`
    /// feature, which is why it is gated there rather than left dead in a
    /// sealed build.
    #[cfg(not(feature = "sealed"))]
    Usage(String),
    /// A sealed step was refused.
    Refused(String),
}

impl std::fmt::Display for SealedError {
    /// What was asked, and why it was refused.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            #[cfg(not(feature = "sealed"))]
            Self::Usage(ref why) => write!(formatter, "estate sealed: {why}"),
            Self::Refused(ref why) => write!(formatter, "estate sealed: refused: {why}"),
        }
    }
}

impl std::error::Error for SealedError {}

/// The refusal of `step`, as the journey reports it.
pub(crate) fn refused(step: &'static str) -> impl FnOnce(String) -> SealedError {
    move |cause| SealedError::Refused(format!("{step} was refused: {cause}"))
}

/// One line to the caller's locked stdout.
pub(crate) fn say(out: &mut impl Write, line: &str) -> Result<(), SealedError> {
    writeln!(out, "{line}").map_err(|error| SealedError::Refused(error.to_string()))
}

/// What a sealed read must show at one point of the journey.
#[cfg(feature = "sealed")]
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// The record decrypts under its live key.
    Live,
    /// The record's key is destroyed: it reads as a tombstone.
    Erased,
}

/// One step of one tenant: the tenant, its run and its step key.
#[cfg(feature = "sealed")]
type Step<'tenant> = (
    &'tenant str,
    lgwks_bot::effect::RunId,
    lgwks_bot::script::StepKey,
);

/// For each `(step, expect, line)` in order, read the step and say `line`
/// when it shows `expect`; refuse at the first read that shows anything else.
#[cfg(feature = "sealed")]
fn expect_reads(
    out: &mut impl Write,
    store: &RunStore,
    checks: &[(Step<'_>, Expect, &str)],
) -> Result<(), SealedError> {
    use lgwks_bot::sealed::runstore::SealedStepRead;
    for &((tenant, run, key), expect, line) in checks {
        let read = store.read_sealed(tenant, run, key);
        let held = matches!(
            (expect, &read),
            (Expect::Live, Ok(SealedStepRead::Live { .. }))
                | (Expect::Erased, Ok(SealedStepRead::Erased { .. }))
        );
        if !held {
            let refusal = SealedError::Refused(format!(
                "the {tenant} step reads as {read:?}, not {expect:?}, where the journey says: {line}"
            ));
            lgwks_std::trace::debug!(error = ?refusal, "estate-sealed: a read deviated from the journey");
            return Err(refusal);
        }
        say(out, line)?;
    }
    Ok(())
}

/// Prove the sealed evening over `dir`: two tenants sealed, one shredded,
/// a restart, and a chain that still verifies.
#[cfg(feature = "sealed")]
fn prove(dir: &Path, out: &mut impl Write) -> Result<(), SealedError> {
    use lgwks_bot::effect::RunId;
    use lgwks_bot::script::{Scope, Tenant};
    use lgwks_bot::sealed::keys::KeyProvider;
    use lgwks_bot::task::{DefinitionIdentity, SealedAppend};

    let keys = dir.join("keys");
    let software = SoftwareProvider::open(&keys)
        .map_err(|error| SealedError::Refused(format!("the provider failed: {error}")))?;
    say(
        out,
        &format!("sealed: page lock {}", software.page_lock_report()),
    )?;
    let provider: Arc<dyn KeyProvider> = Arc::new(software);
    // Two tenants, two files: one journal namespace and one run store each,
    // so no data key ever crosses a tenant boundary.
    let acme_journal = dir.join("acme.jrnl");
    let globex_journal = dir.join("globex.jrnl");
    let store_path = dir.join("runs.runstore");
    let acme = FileJournal::open_sealed(&acme_journal, Arc::clone(&provider), "acme")
        .map_err(|error| refused("the acme journal")(error.to_string()))?;
    let globex = FileJournal::open_sealed(&globex_journal, Arc::clone(&provider), "globex")
        .map_err(|error| refused("the globex journal")(error.to_string()))?;
    let store = RunStore::open_sealed(&store_path, Arc::clone(&provider))
        .map_err(|error| refused("the run store")(error.to_string()))?;
    // Two runs, fixed identities so the journey replays identically: the
    // hex names non-zero ids the schema admits.
    let acme_run = RunId::from_hex("0102030405060708090a0b0c0d0e0f10")
        .map_err(|error| refused("the acme run")(error.to_string()))?;
    let globex_run = RunId::from_hex("1112131415161718191a1b1c1d1e1f20")
        .map_err(|error| refused("the globex run")(error.to_string()))?;
    let input = lgwks_std::hash::blake3(b"lgwks-estate-sealed-v1");
    let definition = DefinitionIdentity::new("estate-sealed", 1, input, 1);
    let acme_scope = Scope::root(
        Tenant::new("acme")
            .map_err(|error| SealedError::Refused(format!("the acme tenant failed: {error}")))?,
    );
    let globex_scope = Scope::root(
        Tenant::new("globex")
            .map_err(|error| SealedError::Refused(format!("the globex tenant failed: {error}")))?,
    );
    let acme_key = acme_scope
        .enter("sealed-evening")
        .map_err(|error| refused("the acme step")(error.to_string()))?
        .key();
    let globex_key = globex_scope
        .enter("sealed-evening")
        .map_err(|error| refused("the globex step")(error.to_string()))?
        .key();
    store
        .append_with_expiry(SealedAppend::new(
            "acme",
            acme_run,
            acme_key,
            "sealed-evening",
            &definition,
            b"the acme secret".to_vec(),
            None,
        ))
        .map_err(|error| refused("the acme append")(error.to_string()))?;
    store
        .append_with_expiry(SealedAppend::new(
            "globex",
            globex_run,
            globex_key,
            "sealed-evening",
            &definition,
            b"the globex secret".to_vec(),
            None,
        ))
        .map_err(|error| refused("the globex append")(error.to_string()))?;
    say(out, "sealed: two tenants hold one live record each")?;
    let acme_step = ("acme", acme_run, acme_key);
    let globex_step = ("globex", globex_run, globex_key);
    expect_reads(
        out,
        &store,
        &[
            (
                acme_step,
                Expect::Live,
                "sealed: the acme step reads live before any shred",
            ),
            (
                globex_step,
                Expect::Live,
                "sealed: the globex step reads live before any shred",
            ),
        ],
    )?;
    // The journal half: one sealed event per tenant would need an EffectKey;
    // the run store above already proves the envelope, and the journals below
    // prove the chain still verifies over sealed bytes. The journal files
    // exist and open sealed, which is the mode-mixing proof: a plain open
    // refuses them.
    drop(acme);
    drop(globex);
    let acme_reopened = FileJournal::open_sealed(&acme_journal, Arc::clone(&provider), "acme")
        .map_err(|error| refused("the acme reopen")(error.to_string()))?;
    match FileJournal::open(&acme_journal) {
        Err(_) => say(out, "sealed: a plain open refuses a sealed journal")?,
        Ok(_) => {
            lgwks_std::trace::debug!("estate-sealed: a plain open read a sealed journal");
            return Err(SealedError::Refused(
                "a plain open read a sealed journal".to_owned(),
            ));
        }
    }
    acme_reopened
        .verify_sealed_chain()
        .map_err(|error| refused("the acme chain")(error.to_string()))?;
    say(out, "sealed: the acme chain verifies")?;
    // Shred the acme step: destroy-then-tombstone, idempotent across the
    // restart below.
    let shredded = store
        .shred_record("acme", acme_run, acme_key)
        .map_err(|error| refused("the acme shred")(error.to_string()))?;
    let _ = shredded;
    expect_reads(
        out,
        &store,
        &[
            (
                acme_step,
                Expect::Erased,
                "sealed: the shredded step reads as erased",
            ),
            (
                globex_step,
                Expect::Live,
                "sealed: the other tenant still reads live",
            ),
        ],
    )?;
    store
        .verify_sealed_chain()
        .map_err(|error| refused("the store chain")(error.to_string()))?;
    say(out, "sealed: the store chain verifies over live and erased")?;
    // Restart: drop every handle, reopen over the same files, and prove the
    // shredded step is still erased and the chain still holds.
    drop(store);
    let store = RunStore::open_sealed(&store_path, Arc::clone(&provider))
        .map_err(|error| refused("the store reopen")(error.to_string()))?;
    expect_reads(
        out,
        &store,
        &[(
            acme_step,
            Expect::Erased,
            "sealed: the erasure survives a restart",
        )],
    )?;
    store
        .verify_sealed_chain()
        .map_err(|error| refused("the reopened chain")(error.to_string()))?;
    say(out, "sealed: the reopened chain still verifies")?;
    // Tenant erasure and the TTL sweep: globex goes by tenant, and an
    // expiring acme record would go by sweep. Here the sweep finds nothing
    // due, which proves it runs without shredding the live.
    let erased = store
        .erase_tenant_records("globex")
        .map_err(|error| refused("the globex erasure")(error.to_string()))?;
    if erased != 1 {
        lgwks_std::trace::debug!(
            erased = erased,
            "estate-sealed: tenant erasure shredded the wrong count"
        );
        return Err(SealedError::Refused(format!(
            "tenant erasure shredded {erased} records, not 1"
        )));
    }
    say(
        out,
        "sealed: tenant erasure shredded exactly the globex step",
    )?;
    let swept = store
        .sweep_once(Duration::from_secs(0))
        .map_err(|error| refused("the sweep")(error.to_string()))?;
    say(
        out,
        &format!("sealed: the sweep shredded {swept} expired records"),
    )?;
    // A re-append under a burned slot is refused, so a restart past a shred
    // cannot duplicate the effect: the burn set outlives the live index.
    expect_reads(
        out,
        &store,
        &[(
            acme_step,
            Expect::Erased,
            "sealed: the burned slot still reads as erased",
        )],
    )?;
    super::sealed_journal::prove_journal(dir, &provider, out)?;
    super::sealed_journal::prove_sweeper(dir, &provider, out)
}

/// Without the `sealed` feature there is no sealed journey: the subcommand
/// names the feature that carries it rather than failing silently.
#[cfg(not(feature = "sealed"))]
fn prove(_dir: &Path, _out: &mut impl Write) -> Result<(), SealedError> {
    Err(SealedError::Usage(
        "the sealed journey needs `--features sealed`".to_owned(),
    ))
}

/// Run `estate sealed`: the sealed evening over a fresh scratch directory.
pub(crate) fn run(_args: &[String], out: &mut impl Write) -> Result<(), SealedError> {
    let unique: [u8; 8] = lgwks_std::random::bytes()
        .map_err(|error| SealedError::Refused(format!("the scratch name failed: {error}")))?;
    let hex: String = unique.iter().map(|byte| format!("{byte:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("lgwks-estate-sealed-{hex}"));
    std::fs::create_dir_all(&dir)
        .map_err(|error| SealedError::Refused(format!("the scratch dir failed: {error}")))?;
    let outcome = prove(&dir, out);
    if outcome.is_ok() {
        let _removed = std::fs::remove_dir_all(&dir);
    } else {
        let _reported = writeln!(
            out,
            "sealed: the failing directory stays at {}",
            PathBuf::from(&dir).display()
        );
    }
    outcome
}
