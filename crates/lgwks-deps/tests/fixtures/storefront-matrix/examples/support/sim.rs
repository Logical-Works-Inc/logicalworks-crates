//! The seeded mode the agent-standards journeys share: `sim <first> <count>`
//! replays seeds `first..first+count`, each driving its own
//! `lgwks_std::seeded::Seeded` stream, and prints one trace line whose hash is a
//! function of the seeds alone, so the same range replays the same hash.

use std::hash::{DefaultHasher, Hasher};
use std::io::Write;
use std::ops::Range;

/// The seeds named on the command line, or `None` for the plain journey.
pub(crate) fn seeds() -> Option<Range<u64>> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("sim") {
        return None;
    }
    let first: u64 = args.next()?.parse().ok()?;
    let count: u64 = args.next()?.parse().ok()?;
    Some(first..first.checked_add(count)?)
}

/// What one seeded run observed: a fold of every recorded fact, and named
/// counters the caller's test reads back.
pub(crate) struct Trace {
    hasher: DefaultHasher,
    counters: Vec<(&'static str, u64)>,
}

impl Trace {
    pub(crate) fn new() -> Self {
        Self {
            hasher: DefaultHasher::new(),
            counters: Vec::new(),
        }
    }

    /// Fold `fact` into the hash and count it under `name`.
    pub(crate) fn record(&mut self, name: &'static str, fact: u64) {
        self.hasher.write(name.as_bytes());
        self.hasher.write_u64(fact);
        match self.counters.iter_mut().find(|(known, _)| *known == name) {
            Some((_, count)) => *count = count.saturating_add(1),
            None => self.counters.push((name, 1)),
        }
    }

    /// Write `trace hash=<hex> <name>=<count>...` for the test to read; false
    /// when stdout refused it, so a lost trace fails the run.
    pub(crate) fn report(&self) -> bool {
        let counts: Vec<String> = self
            .counters
            .iter()
            .map(|(name, count)| format!("{name}={count}"))
            .collect();
        let line = format!(
            "trace hash={:016x} {}\n",
            self.hasher.finish(),
            counts.join(" ")
        );
        std::io::stdout().lock().write_all(line.as_bytes()).is_ok()
    }
}
