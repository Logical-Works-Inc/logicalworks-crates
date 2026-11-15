//! `fs` owns the filesystem domain. Requires `bot.fs`.

use std::path::PathBuf;

use crate::cap::{Auth, Cap};
use crate::ecs::EcsError;
use crate::error::{DispatchCertainty, KernelError};
use crate::verb;

/// Observe a filesystem path for changes. Supports Observe and Query.
#[derive(Debug)]
pub struct Path {
    /// The path inspected on each poll. Resolved at construction, so a relative
    /// path is relative to the working directory as it was when the domain was
    /// built rather than at poll time.
    target: PathBuf,
    /// Forced to `[bot.fs]` by the constructor: a stat is a filesystem
    /// operation and no constructor omits the capability.
    caps: Vec<Cap>,
}

/// Filesystem state returned by observation or query.
///
/// `#[non_exhaustive]`: the reported shape grows with the domain, and a
/// consumer that destructured this literally would break on each addition.
#[derive(PartialEq, Debug, Clone)]
#[non_exhaustive]
pub struct FsState {
    /// Whether the path was modified since last poll.
    pub modified: bool,
    /// Whether the path exists.
    pub exists: bool,
    /// File size in bytes, if it exists.
    pub size: Option<u64>,
}

impl Path {
    /// Create a filesystem path observer.
    pub fn new(target: impl Into<PathBuf>) -> Self {
        Self {
            target: target.into(),
            caps: vec![Cap::fs()],
        }
    }
}

/// Whether `target` exists and its size in bytes, read on a blocking-pool
/// thread. A metadata read that fails after the existence check (the file was
/// removed between the two calls) reports no size and is traced, never hidden.
fn stat_of(target: &std::path::Path) -> (bool, Option<u64>) {
    if !target.exists() {
        return (false, None);
    }
    match std::fs::metadata(target) {
        Ok(metadata) => (true, Some(metadata.len())),
        Err(error) => {
            lgwks_std::trace::debug!(%error, path = %target.display(), "fs::path: metadata read failed after the path existed");
            (true, None)
        }
    }
}

impl verb::Observe for Path {
    type Output = FsState;

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    async fn poll(&self, call: (Auth, ())) -> Result<FsState, EcsError> {
        call.0.check(self.required_caps())?;
        let target = self.target.clone();
        // The stat runs on the bounded blocking pool, and its handle is awaited
        // here, so the poll owns the job; a full pool is a typed refusal rather
        // than an unbounded queue.
        let stat =
            lgwks_std::task::try_spawn_blocking(move || stat_of(&target)).map_err(|refusal| {
                KernelError::DomainError {
                    domain: String::from("fs::path"),
                    certainty: DispatchCertainty::Refused,
                    cause: refusal.to_string(),
                }
            })?;
        let (exists, size) = stat.await;
        Ok(FsState {
            modified: false,
            exists,
            size,
        })
    }

    fn domain_id(&self) -> &str {
        "fs::path"
    }
}

impl verb::Query for Path {
    type Input = ();
    type Output = FsState;

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    async fn query(&self, call: (Auth, &())) -> Result<FsState, EcsError> {
        let (auth, _) = call;
        verb::Observe::poll(self, (auth, ())).await
    }

    fn domain_id(&self) -> &str {
        "fs::path"
    }
}

/// Convenience: create a filesystem path observer.
pub fn path(target: impl Into<PathBuf>) -> Path {
    Path::new(target)
}
