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
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Entropy(ref cause) => write!(f, "could not name a scratch directory: {cause}"),
            Self::Scratch(ref cause) => write!(f, "could not create a scratch directory: {cause}"),
            Self::Output(ref cause) => write!(f, "could not write the report: {cause}"),
            Self::Encode(ref cause) => write!(f, "could not encode a violation: {cause}"),
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
    let file_conformed = report(&mut out, "file", &file, json)?;
    let memory_conformed = report(&mut out, "memory", &double, json)?;
    if file_conformed && memory_conformed && !json {
        writeln!(out, "both stores conform").map_err(RunError::Output)?;
    }
    Ok(file_conformed && memory_conformed)
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
