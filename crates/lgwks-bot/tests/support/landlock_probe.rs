//! The Landlock probe `sim_landlock` asserts and the `landlock_cost` example
//! times, kept as one copy so the two read the same allowlist (#1114).
//!
//! The probe fills a root with directories of one file each, grants all but
//! the last few beside the system grants, and runs a shell that reads both ends
//! of the grant and is refused one left out. Included by path from each target.

use std::error::Error;
use std::path::Path;

use lgwks_bot::rt::process::{LandlockRules, PathAccess};

/// The system directories a shell and `cat` need to load, granted read-only.
pub(crate) fn system_rules() -> LandlockRules {
    let mut rules = LandlockRules::new();
    for dir in ["/usr", "/bin", "/lib", "/lib64", "/etc"] {
        if Path::new(dir).exists() {
            rules.allow(dir, PathAccess::Read);
        }
    }
    rules
}

/// One probe: two allowlists over one tree and the shell that tests them.
pub(crate) struct Probe {
    /// Exactly the requested number of grants.
    pub(crate) full: LandlockRules,
    /// The system grants and the two ends only, the baseline beside `full`.
    pub(crate) few: LandlockRules,
    /// Exits zero only when both ends are read and the left-out file is refused.
    pub(crate) script: String,
}

/// Build a probe of `rules` grants under `root`, which gets `rules` directories.
pub(crate) fn probe(root: &Path, rules: usize) -> Result<Probe, Box<dyn Error>> {
    let mut dirs = Vec::with_capacity(rules);
    for index in 0..rules {
        let dir = root.join(format!("d{index}"));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("secret"), format!("s{index}"))?;
        dirs.push(dir);
    }
    let mut base = system_rules();
    // `/dev/null` is the probe's sink; `/dev` holds no tenant file.
    base.allow("/dev", PathAccess::ReadWrite);
    let room = rules
        .checked_sub(base.rules().len())
        .ok_or("the system grants fit")?;
    let (granted, left_out) = dirs.split_at(room);
    let first = granted.first().ok_or("a first grant")?;
    let last = granted.last().ok_or("a last grant")?;
    let outside = left_out.first().ok_or("a directory left out")?;
    let script = format!(
        "cat {} >/dev/null && cat {} >/dev/null && ! cat {} >/dev/null 2>&1",
        first.join("secret").display(),
        last.join("secret").display(),
        outside.join("secret").display()
    );
    let mut few = base.clone();
    few.allow(first, PathAccess::Read)
        .allow(last, PathAccess::Read);
    let mut full = base;
    for dir in granted {
        full.allow(dir, PathAccess::Read);
    }
    Ok(Probe { full, few, script })
}
