//! `store-conformance`: the run-store conformance suite, run against the shipped
//! file store and against an in-memory store of the caller's own, both installed
//! through the one `HostBuilder::records` door.
//!
//! ```text
//! cargo run -p lgwks_bot --features testing --bin store-conformance
//! cargo run -p lgwks_bot --features testing --bin store-conformance -- --json
//! ```
//!
//! One line per store and check, and a non-zero exit when any store violates any
//! check. `--json` prints one object per line for an agent. The in-memory store
//! (`store_conformance/memory_records.rs`) is the shape a developer copies to put
//! their own database behind durable steps.
//!
//! Built with `sealed` too (`--features full`), it adds a `keys` row: the sealed
//! key-provider kit (`lgwks_bot::sealed::conformance`, #1060) against the shipped
//! software provider over `KEY_SEEDS` seeds, then concurrent mints that must stay
//! distinct and a foreign tenant that must be refused every key.
//!
//! Output is written through locked `std::io` handles rather than `println!`
//! because `clippy::print_stdout` is forbidden workspace-wide.

use std::error::Error;
use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use lgwks_bot::script::run_store::conformance::{self, Conformance, Opened};
use lgwks_bot::task::RunStore;

#[path = "store_conformance/memory_records.rs"]
mod memory_records;

use memory_records::MemoryRecords;

/// What opening a store under test can fail with.
type OpenError = Box<dyn Error + Send + Sync>;

/// Why the suite could not be run or reported, as distinct from a store that
/// violated it.
#[derive(Debug)]
enum RunError {
    /// The entropy source could not name the scratch directory.
    Entropy(String),
    /// The scratch directory could not be created.
    Scratch(std::io::Error),
    /// The report could not be written.
    Output(std::io::Error),
    /// A violation could not be encoded as JSON.
    Encode(String),
    /// The key provider under test could not be opened or asked.
    #[cfg(feature = "sealed")]
    Keys(lgwks_bot::sealed::keys::KeyError),
}

/// Seeds the `keys` row runs the sealed provider kit for.
#[cfg(feature = "sealed")]
const KEY_SEEDS: u64 = 16;

/// Threads the `keys` row mints from at once.
#[cfg(feature = "sealed")]
const KEY_THREADS: u8 = 4;

/// Keys each of those threads mints.
#[cfg(feature = "sealed")]
const KEYS_PER_THREAD: usize = 64;

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Entropy(ref cause) => write!(f, "could not name a scratch directory: {cause}"),
            Self::Scratch(ref cause) => write!(f, "could not create a scratch directory: {cause}"),
            Self::Output(ref cause) => write!(f, "could not write the report: {cause}"),
            Self::Encode(ref cause) => write!(f, "could not encode a violation: {cause}"),
            #[cfg(feature = "sealed")]
            Self::Keys(ref cause) => write!(f, "the key provider refused: {cause}"),
        }
    }
}

/// A directory under the temp root, removed when dropped.
struct Scratch(std::path::PathBuf);

impl Scratch {
    /// A fresh directory with a name the entropy source drew, never a pid or a
    /// clock the OS reuses (INV-BOT-116).
    fn new() -> Result<Self, RunError> {
        let unique = lgwks_std::random::bytes::<8>()
            .map_err(|refused| RunError::Entropy(refused.to_string()))?;
        let hex: String = unique.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = std::env::temp_dir().join(format!("lgwks-store-conformance-{hex}"));
        std::fs::create_dir_all(&path).map_err(RunError::Scratch)?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    /// Best effort: a directory the system refuses to remove is litter under the
    /// temp root, not a wrong answer.
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.0));
    }
}

/// Write `store`'s outcome, one line per check, and say whether it conformed.
fn report(
    out: &mut impl Write,
    store: &str,
    found: &Conformance,
    json: bool,
) -> Result<bool, RunError> {
    for check in found.passed() {
        if json {
            writeln!(
                out,
                r#"{{"store":"{store}","check":"{check}","pass":true}}"#
            )
        } else {
            writeln!(out, "{store}: {check}: pass")
        }
        .map_err(RunError::Output)?;
    }
    let mut conformed = true;
    for (check, violation) in found.violations() {
        conformed = false;
        if json {
            let text = lgwks_std::json::to_string(&violation.to_string())
                .map_err(|refused| RunError::Encode(refused.to_string()))?;
            writeln!(
                out,
                r#"{{"store":"{store}","check":"{check}","pass":false,"violation":{text}}}"#
            )
        } else {
            writeln!(out, "{store}: {check}: VIOLATION: {violation}")
        }
        .map_err(RunError::Output)?;
    }
    Ok(conformed)
}

/// The `keys` row: each check's name and, when it failed, what it found.
#[cfg(feature = "sealed")]
fn keys(scratch: &Scratch) -> Result<Vec<(String, Option<String>)>, RunError> {
    use lgwks_bot::sealed::conformance::{self as kit, mint_ids, try_unwrap};
    use lgwks_bot::sealed::keys::{KeyError, KeyId, KeyProvider, SoftwareProvider};

    let mut rows = Vec::new();
    for seed in 0..KEY_SEEDS {
        let dir = scratch.0.join(format!("keys-{seed}"));
        let mut open = || -> Result<Arc<dyn KeyProvider>, KeyError> {
            Ok(Arc::new(SoftwareProvider::open(&dir)?))
        };
        let found = kit::check(&mut open, seed);
        let failed = (!found.passed()).then(|| format!("{:?}", found.violations()));
        rows.push((format!("kit seed {seed}"), failed));
    }

    let provider: Arc<dyn KeyProvider> =
        Arc::new(SoftwareProvider::open(&scratch.0.join("keys-mint")).map_err(RunError::Keys)?);
    let minted: Vec<Result<Vec<KeyId>, KeyError>> = std::thread::scope(|scope| {
        // Every thread starts before the first join, so the mints race.
        let mut workers = Vec::with_capacity(usize::from(KEY_THREADS));
        for thread in 0..KEY_THREADS {
            let provider = Arc::clone(&provider);
            workers.push(scope.spawn(move || {
                mint_ids(
                    provider.as_ref(),
                    &format!("tenant-{thread}"),
                    KEYS_PER_THREAD,
                )
            }));
        }
        workers
            .into_iter()
            .map(|worker| match worker.join() {
                Ok(batch) => batch,
                Err(_panic) => {
                    lgwks_std::trace::error!("store-conformance: a minting thread panicked");
                    Err(KeyError::Transient {
                        reason: "a minting thread panicked",
                    })
                }
            })
            .collect()
    });
    let mut seen = std::collections::HashSet::new();
    let mut ids = Vec::new();
    for batch in minted {
        for id in batch.map_err(RunError::Keys)? {
            seen.insert(id.id());
            ids.push(id);
        }
    }
    let twice = ids.len().saturating_sub(seen.len());
    rows.push((
        "concurrent mints are distinct".to_owned(),
        (twice > 0).then(|| format!("{twice} identities were minted twice")),
    ));

    let mut leaked = 0_usize;
    for id in &ids {
        let wrapped = provider.wrap_key(id).map_err(RunError::Keys)?;
        let foreign = KeyId::new(format!("{}-foreign", id.tenant()), id.id());
        match try_unwrap(provider.as_ref(), &foreign, &wrapped) {
            Ok(()) => leaked = leaked.saturating_add(1),
            Err(refused) => {
                lgwks_std::trace::debug!(error = %refused, "store-conformance: a foreign tenant was refused");
            }
        }
    }
    rows.push((
        "a foreign tenant opens no key".to_owned(),
        (leaked > 0).then(|| format!("{leaked} keys opened for another tenant")),
    ));
    Ok(rows)
}

/// Write the `keys` row, one line per check, and say whether every check held.
#[cfg(feature = "sealed")]
fn report_keys(
    out: &mut impl Write,
    rows: &[(String, Option<String>)],
    json: bool,
) -> Result<bool, RunError> {
    for row in rows {
        let check = &row.0;
        let line = match row.1.as_deref() {
            None if json => format!(r#"{{"store":"keys","check":"{check}","pass":true}}"#),
            None => format!("keys: {check}: pass"),
            Some(found) if json => {
                let text = lgwks_std::json::to_string(found)
                    .map_err(|refused| RunError::Encode(refused.to_string()))?;
                format!(r#"{{"store":"keys","check":"{check}","pass":false,"violation":{text}}}"#)
            }
            Some(found) => format!("keys: {check}: VIOLATION: {found}"),
        };
        writeln!(out, "{line}").map_err(RunError::Output)?;
    }
    Ok(rows.iter().filter(|row| row.1.is_some()).count() == 0)
}

/// Run the suite against both stores.
fn run(json: bool) -> Result<bool, RunError> {
    let scratch = Scratch::new()?;
    let path = scratch.0.join("conformance.runstore");
    let mut open_file = || -> Result<Opened, OpenError> {
        let store = RunStore::open(&path)?;
        let faults = store.clone();
        Ok(Opened::new(Arc::new(store), move || {
            faults.fail_next_index_read();
        }))
    };
    let file = conformance::check(&mut open_file);

    let memory = MemoryRecords::default();
    let mut open_memory = || -> Result<Opened, OpenError> {
        let faults = memory.clone();
        Ok(Opened::new(Arc::new(memory.clone()), move || {
            faults.fail_next_read();
        }))
    };
    let double = conformance::check(&mut open_memory);

    let mut out = std::io::stdout().lock();
    let mut verdicts = vec![
        report(&mut out, "file", &file, json)?,
        report(&mut out, "memory", &double, json)?,
    ];
    #[cfg(feature = "sealed")]
    verdicts.push(report_keys(&mut out, &keys(&scratch)?, json)?);
    // Counted, not folded with `&&`: a conforming run then shows each verdict
    // in its exit code, and `--json` output stays one object per line.
    let failed = verdicts.iter().filter(|conformed| !**conformed).count();
    if failed == 0 {
        if json {
            lgwks_std::trace::debug!("store-conformance: every store conforms");
        } else {
            writeln!(out, "every store conforms").map_err(RunError::Output)?;
        }
    }
    Ok(failed == 0)
}

fn main() -> ExitCode {
    let json = std::env::args().skip(1).any(|arg| arg == "--json");
    match run(json) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(refused) => {
            lgwks_std::trace::error!(error = %refused, "store-conformance: could not run the suite");
            let mut err = std::io::stderr().lock();
            if let Err(unwritten) = writeln!(err, "store-conformance: {refused}") {
                lgwks_std::trace::error!(error = %unwritten, "store-conformance: stderr refused the report");
            }
            ExitCode::from(2)
        }
    }
}
