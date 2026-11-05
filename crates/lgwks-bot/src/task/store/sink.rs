//! The durable sink's readiness contract and the start gate over it.
//!
//! A dead durable sink still boots and serves is the defect issue #820 records:
//! the readiness probe exists and performs a real durability barrier, but
//! nothing on the start path consults it. This module is the one home for the
//! three pieces that close it: [`DurableSink`], the readiness contract every
//! sink probes through; [`StoreSink`], the shipped file-backed sink whose probe
//! is a real fsync barrier rather than a constant; and [`CompositeSink`], the
//! AND-combine that names the child which failed.
//!
//! The production start paths consult the composite before serving anything:
//! the estate's durable tier after it creates the store directory, and `serve`
//! before it converts the listener. A dead directory refuses with [`SinkRefusal`], whose one-line render names the sink
//! and the failure for the operator log, and never a path, a credential, or a
//! second line.
//!
//! # Bounds
//!
//! One probe stages one fixed-name file of 64 constant bytes, created with
//! `create_new` so no concurrent gate and no crashed start is ever overwritten,
//! synced, and removed. A stale file from a crashed start is removed once and
//! restaged once; a squatter that is not a file blocks the path and is refused
//! rather than deleted, because the probe must never delete operator data.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The probe file a barrier write stages in, fixed so a crashed start leaves at
/// most one reusable file and never an accumulating litter.
///
/// Public because a test that simulates "dead after mount" must squat exactly
/// this path: guessing the literal would couple the test to a string rather
/// than to the gate it proves.
///
/// # Examples
///
/// ```text
/// use lgwks_bot::task::START_PROBE_FILE;
/// assert_eq!(START_PROBE_FILE, ".lgwks-start-probe");
/// ```
pub const START_PROBE_FILE: &str = ".lgwks-start-probe";

/// The bytes one probe persists: constant, so two concurrent gates stage the
/// same file and neither can mistake the other's bytes for a foreign record.
const PROBE_BYTES: &[u8; 64] = b"lgwks-bot start-gate barrier probe v1...........................";

/// Why a durable sink refused the start gate: the sink that failed and the
/// failure, both operator-facing.
///
/// The reason carries the failure kind, never a path and never a credential:
/// the gate runs on unauthenticated operator surface, like the kernel's
/// degraded `/readyz`, so a connection string reaching it would leak host,
/// port, and secrets to anyone who can reach the log.
///
/// # Examples
///
/// ```text
/// use lgwks_bot::task::SinkRefusal;
/// let refusal = SinkRefusal::new("runstore", "unready");
/// assert_eq!(refusal.sink(), "runstore");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SinkRefusal {
    /// The sink that failed, or the composite that found it: a constant name,
    /// never a path, so the refusal carries no secret.
    sink: String,
    /// What failed, as a failure kind rather than a path or an OS sentence:
    /// safe for the unauthenticated operator log.
    reason: String,
}

impl SinkRefusal {
    /// Record that `sink` refused for `reason`: both are operator-facing, and
    /// neither may carry a path or a credential.
    ///
    /// # Examples
    ///
    /// ```text
    /// use lgwks_bot::task::SinkRefusal;
    /// let refusal = SinkRefusal::new("runstore", "disk barrier failed");
    /// assert_eq!(refusal.sink(), "runstore");
    /// ```
    #[must_use]
    pub fn new(sink: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            sink: sink.into(),
            reason: reason.into(),
        }
    }

    /// The name the refusal is filed under: the composite's name when the
    /// composite found the failure, else the child's own constant name.
    ///
    /// # Examples
    ///
    /// ```text
    /// use lgwks_bot::task::SinkRefusal;
    /// let refusal = SinkRefusal::new("runstore", "read-only");
    /// assert_eq!(refusal.sink(), "runstore");
    /// ```
    #[must_use]
    pub fn sink(&self) -> &str {
        self.sink.as_str()
    }

    /// What failed, as a failure kind rather than a path or an OS sentence:
    /// safe for the unauthenticated operator log.
    ///
    /// # Examples
    ///
    /// ```text
    /// use lgwks_bot::task::SinkRefusal;
    /// let refusal = SinkRefusal::new("runstore", "read-only");
    /// assert_eq!(refusal.reason(), "read-only");
    /// ```
    #[must_use]
    pub fn reason(&self) -> &str {
        self.reason.as_str()
    }
}

impl fmt::Display for SinkRefusal {
    /// The one-line operator reason: the sink, then the failure.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "durable sink '{}' is not ready: {}",
            self.sink, self.reason
        )
    }
}

impl std::error::Error for SinkRefusal {}

/// The readiness contract every durable sink probes through.
///
/// Readiness is a real durability barrier, not a constant and not a
/// file-existence check: a sink that has to do nothing to look ready is a sink
/// that silently drops every record. Requiring the probe makes every
/// implementation state what it does with a record — including a stub, which
/// says so in as many words and is refused for it.
///
/// # Examples
///
/// ```text
/// use lgwks_bot::task::{DurableSink, SinkRefusal};
/// struct MySink;
/// impl DurableSink for MySink {
///     fn sink_name(&self) -> &str { "mock" }
///     fn probe_barrier(&self) -> Result<(), SinkRefusal> { Ok(()) }
/// }
/// ```
pub trait DurableSink: Send + Sync {
    /// The constant name this sink reports under: `runstore` for the file
    /// sink, the composite's own name for a composite.
    fn sink_name(&self) -> &str;

    /// Perform the real durability barrier, refusing with this sink's name.
    ///
    /// # Errors
    ///
    /// [`SinkRefusal`] naming this sink when the barrier does not persist.
    fn probe_barrier(&self) -> Result<(), SinkRefusal>;
}

/// The shipped file-backed run-store sink over one store directory.
///
/// The probe stages [`START_PROBE_FILE`], writes `PROBE_BYTES`, syncs, and
/// removes it: on a directory that cannot persist — read-only remount,
/// replaced data directory, a squatter on the probe path — the create, the
/// write, or the sync refuses, and readiness is false.
///
/// # Examples
///
/// ```text
/// use std::path::Path;
/// use lgwks_bot::task::StoreSink;
/// let sink = StoreSink::for_dir(Path::new("/tmp"));
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StoreSink {
    /// The constant name refusals carry: `runstore`.
    name: String,
    /// The store directory the barrier probes.
    dir: PathBuf,
}

impl StoreSink {
    /// Build the file-backed sink over `dir`: readiness stages, syncs and
    /// removes the probe file there, so a directory that cannot persist reads
    /// unready.
    ///
    /// # Examples
    ///
    /// ```text
    /// use std::path::Path;
    /// use lgwks_bot::task::StoreSink;
    /// let sink = StoreSink::for_dir(Path::new("/tmp/test"));
    /// ```
    #[must_use]
    pub fn for_dir(dir: &Path) -> Self {
        Self {
            name: String::from("runstore"),
            dir: dir.to_path_buf(),
        }
    }
}

impl DurableSink for StoreSink {
    fn sink_name(&self) -> &str {
        self.name.as_str()
    }

    fn probe_barrier(&self) -> Result<(), SinkRefusal> {
        probe_store_dir(&self.dir)
            .map_err(|failure| SinkRefusal::new(self.name.clone(), failure.to_string()))
    }
}

/// The AND-combine over the production sinks: one unready child makes the
/// composite unready, and the refusal names the child that failed.
///
/// An empty fan-out holds no durable sink at all, so it is unready too: every
/// child must also answer ready, or one dead child would leave the composite
/// reporting a false green.
///
/// # Examples
///
/// ```text
/// use lgwks_bot::task::CompositeSink;
/// let composite = CompositeSink::new("composite", Vec::new());
/// ```
#[derive(Clone)]
pub struct CompositeSink {
    /// The composite's own name, carried on every refusal it reports.
    name: String,
    /// The children, in probe order.
    sinks: Vec<Arc<dyn DurableSink>>,
}

impl CompositeSink {
    /// Combine `sinks` under `name`.
    ///
    /// # Examples
    ///
    /// ```text
    /// use lgwks_bot::task::CompositeSink;
    /// let composite = CompositeSink::new("production", Vec::new());
    /// ```
    #[must_use]
    pub fn new(name: &str, sinks: impl IntoIterator<Item = Arc<dyn DurableSink>>) -> Self {
        Self {
            name: name.to_owned(),
            sinks: sinks.into_iter().collect(),
        }
    }

    /// Probe every child, failing closed on the first refusal.
    ///
    /// # Examples
    ///
    /// ```text
    /// use lgwks_bot::task::CompositeSink;
    /// let composite = CompositeSink::new("production", Vec::new());
    /// assert!(composite.readiness().is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// [`SinkRefusal`] naming this composite, with the failed child's name and
    /// reason in the reason, when any child is unready — or when no sink is
    /// installed at all.
    pub fn readiness(&self) -> Result<(), SinkRefusal> {
        if self.sinks.is_empty() {
            let refusal = SinkRefusal::new(
                self.name.clone(),
                String::from("no durable sink is installed"),
            );
            lgwks_std::trace::debug!(error = ?refusal, "sink: the composite holds no sink");
            return Err(refusal);
        }
        for sink in &self.sinks {
            if let Err(refusal) = sink.probe_barrier() {
                let refusal = SinkRefusal::new(
                    self.name.clone(),
                    format!(
                        "child '{}' is not ready: {}",
                        sink.sink_name(),
                        refusal.reason()
                    ),
                );
                lgwks_std::trace::debug!(error = ?refusal, "sink: a child is not ready");
                return Err(refusal);
            }
        }
        Ok(())
    }
}

/// Why the barrier probe refused: a typed cause, matched rather than read.
///
/// The path is never carried: it may hold operator secrets, and an OS sentence
/// may vary by platform while the failure kind is the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarrierFailure {
    /// A squatter that is not a file blocks the probe path.
    Blocked,
    /// The device refused with this kind.
    Io(std::io::ErrorKind),
}

impl fmt::Display for BarrierFailure {
    /// The constant, secret-free, one-line reason a refusal carries.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Blocked => formatter.write_str("the store dir barrier path is blocked"),
            Self::Io(kind) => write!(formatter, "the store dir barrier failed: {kind:?}"),
        }
    }
}

impl std::error::Error for BarrierFailure {}

/// Probe the barrier on `dir`: stage, sync, and remove the probe file.
///
/// # Errors
///
/// [`BarrierFailure`] naming the failure kind.
fn probe_store_dir(dir: &Path) -> Result<(), BarrierFailure> {
    let probe = dir.join(START_PROBE_FILE);
    match fs::symlink_metadata(&probe) {
        Ok(metadata) if !metadata.is_file() => {
            // A squatter that is not a file blocks the barrier path: refuse
            // rather than delete, because the probe must never delete
            // operator data — not even to prove the directory is writable.
            lgwks_std::trace::debug!("sink: the barrier path is blocked by a non-file");
            return Err(BarrierFailure::Blocked);
        }
        Ok(_) => {
            // A stale file from a crashed start: remove once, then stage again
            // below. One removal, not a loop, so a racing gate cannot spin.
            if let Err(error) = fs::remove_file(&probe) {
                lgwks_std::trace::debug!(error = ?error.kind(), "sink: the stale probe is not removable");
                return Err(BarrierFailure::Io(error.kind()));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error.kind(), "sink: the barrier path is unreadable");
            return Err(BarrierFailure::Io(error.kind()));
        }
    }
    stage_probe(&probe)
}

/// Stage the probe file: create, write, sync, remove.
///
/// # Errors
///
/// [`BarrierFailure`] naming the failure kind, as in [`probe_store_dir`].
fn stage_probe(probe: &Path) -> Result<(), BarrierFailure> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = match options.open(probe) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // A concurrent gate staged first: remove once and retry once,
            // bounded. A second refusal means the directory, not the race.
            if let Err(error) = fs::remove_file(probe) {
                lgwks_std::trace::debug!(error = ?error.kind(), "sink: the raced probe is not removable");
                return Err(BarrierFailure::Io(error.kind()));
            }
            match options.open(probe) {
                Ok(file) => file,
                Err(error) => {
                    lgwks_std::trace::debug!(error = ?error.kind(), "sink: the barrier create is refused");
                    return Err(BarrierFailure::Io(error.kind()));
                }
            }
        }
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error.kind(), "sink: the barrier create is refused");
            return Err(BarrierFailure::Io(error.kind()));
        }
    };
    if let Err(error) = file.write_all(PROBE_BYTES) {
        lgwks_std::trace::debug!(error = ?error.kind(), "sink: the barrier write is refused");
        return Err(BarrierFailure::Io(error.kind()));
    }
    if let Err(error) = file.sync_all() {
        lgwks_std::trace::debug!(error = ?error.kind(), "sink: the barrier sync is refused");
        return Err(BarrierFailure::Io(error.kind()));
    }
    drop(file);
    if let Err(error) = fs::remove_file(probe) {
        // The barrier proved the directory persists, but a probe that cannot
        // be removed would accumulate one file per start: refuse rather than
        // litter, fail-closed to the end.
        lgwks_std::trace::debug!(error = ?error.kind(), "sink: the probe is not removable");
        return Err(BarrierFailure::Io(error.kind()));
    }
    Ok(())
}
