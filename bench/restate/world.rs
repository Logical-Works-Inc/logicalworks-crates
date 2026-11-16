//! What the Restate flow talks to: the two upstream calls with a fixed latency,
//! the failure plan the driver wrote, the step log and the notices the driver
//! waits on.
//!
//! The plan is the one `durable/world.rs` drew from the seed, written to a file
//! by the driver, so both rows fail the same attempts of the same runs. Restate
//! owns the retry: this file only says which attempt of a call fails.

use std::collections::HashMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use lgwks_bot::clock::Clock;

use crate::rival_wire::{PARKED_WORD, key_of, parse_plan};

/// Attempts an upstream call gets: the first and four retries.
pub(crate) const ATTEMPTS: u32 = 5;

/// What each pure step adds to the running total: its position in the flow.
pub(crate) const STEP_WEIGHTS: [u64; 7] = [1, 2, 4, 5, 7, 9, 10];

/// The word that opens the notice of a run that ended without finishing.
const LOST_WORD: &str = "lost";

/// Which upstream a call goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Call {
    /// The outside call of step 3: 2 ms.
    Outside,
    /// The model call of step 6: 20 ms.
    Model,
}

impl Call {
    /// The fixed latency of the upstream.
    const fn delay(self) -> Duration {
        match self {
            Self::Outside => Duration::from_millis(2),
            Self::Model => Duration::from_millis(20),
        }
    }

    /// What the upstream returns for `ordinal`.
    const fn value(self, ordinal: u32) -> u64 {
        let ordinal = ordinal as u64;
        match self {
            Self::Outside => ordinal.wrapping_mul(3).wrapping_add(1),
            Self::Model => ordinal.wrapping_mul(5).wrapping_add(2),
        }
    }

    /// The failing attempts the plan names for this call.
    const fn failing(self, plan: (u32, u32)) -> u32 {
        match self {
            Self::Outside => plan.0,
            Self::Model => plan.1,
        }
    }
}

/// Why the world refused.
#[derive(Debug)]
pub(crate) enum WorldError {
    /// A file or stdout operation failed.
    Io {
        /// What was being done.
        context: &'static str,
        /// What the OS said.
        source: io::Error,
    },
    /// The plan file has a line that is not `<outside> <model>`.
    PlanMalformed,
    /// A run the plan does not cover.
    OutsidePlan {
        /// The run.
        ordinal: u32,
    },
    /// A lock was poisoned by a panic elsewhere.
    Poisoned,
    /// The plan says this attempt fails.
    Injected {
        /// The step.
        step: &'static str,
        /// The run.
        ordinal: u32,
        /// The attempt.
        attempt: u32,
    },
}

impl fmt::Display for WorldError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Io { context, .. } => write!(formatter, "{context} failed"),
            Self::PlanMalformed => formatter.write_str("the plan file does not read"),
            Self::OutsidePlan { ordinal } => write!(formatter, "run {ordinal} is outside the plan"),
            Self::Poisoned => formatter.write_str("a lock was poisoned"),
            Self::Injected {
                step,
                ordinal,
                attempt,
            } => write!(formatter, "injected failure: run {ordinal} {step} attempt {attempt}"),
        }
    }
}

impl std::error::Error for WorldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Io { ref source, .. } => Some(source),
            _ => None,
        }
    }
}

/// An append-only text file many tasks write whole lines to.
struct Lines {
    /// The file, behind a lock so two lines never interleave.
    file: Mutex<File>,
}

impl Lines {
    /// Open `path` for appending, creating it.
    fn open(path: &Path, context: &'static str) -> Result<Self, WorldError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|source| WorldError::Io { context, source })?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }

    /// Append one line, newline included, in a single write.
    fn append(&self, line: &str) -> Result<(), WorldError> {
        let mut file = self.file.lock().map_err(|poisoned| {
            lgwks_std::trace::debug!(error = %poisoned, "a log's lock was poisoned");
            WorldError::Poisoned
        })?;
        file.write_all(format!("{line}\n").as_bytes())
            .map_err(|source| WorldError::Io {
                context: "writing a log line",
                source,
            })
    }
}

/// Everything the flow reaches that is not Restate.
pub(crate) struct World {
    /// The failing attempts of each run's two calls, by run number.
    plan: Vec<(u32, u32)>,
    /// The clock every upstream wait is made on: the wall clock in the worker
    /// binary, the same wait the lgwks row makes, and a virtual one a simulation
    /// moves itself.
    clock: Clock,
    /// The step log, when recording step bodies was asked for.
    steps: Option<Lines>,
    /// One line for each retry and each injected failure.
    attempts: Lines,
    /// Executions of each step body so far, for the steps in flight only: a step
    /// that finished leaves no entry, so a second run of it starts at one again.
    tries: Mutex<HashMap<(String, &'static str), u32>>,
}

impl World {
    /// The world over the plan file `plan`, logging to `steps` when given and to
    /// `attempts` always, waiting its upstream latencies on `clock`.
    pub(crate) fn load(
        plan: &Path,
        steps: Option<&Path>,
        attempts: &Path,
        clock: Clock,
    ) -> Result<Self, WorldError> {
        let text = std::fs::read_to_string(plan).map_err(|source| WorldError::Io {
            context: "reading the plan file",
            source,
        })?;
        let Some(plan) = parse_plan(&text) else {
            lgwks_std::trace::debug!("the plan file has a line that is not two numbers");
            return Err(WorldError::PlanMalformed);
        };
        let steps = steps
            .map(|path| Lines::open(path, "opening the step log"))
            .transpose()?;
        Ok(Self {
            plan,
            clock,
            steps,
            attempts: Lines::open(attempts, "opening the attempt log")?,
            tries: Mutex::new(HashMap::new()),
        })
    }

    /// Count one execution of a step body. The first one is the step running: it
    /// writes its step-log line, in the format the driver's judge reads.
    fn begin(&self, tenant: &str, ordinal: u32, step: &'static str) -> Result<u32, WorldError> {
        let n = {
            let mut tries = self.tries.lock().map_err(|poisoned| {
                lgwks_std::trace::debug!(error = %poisoned, "the tries lock was poisoned");
                WorldError::Poisoned
            })?;
            let seen = tries.entry((key_of(tenant, ordinal), step)).or_insert(0);
            *seen = seen.saturating_add(1);
            *seen
        };
        if n == 1
            && let Some(log) = self.steps.as_ref()
        {
            log.append(&format!("{tenant} {ordinal} {step}"))?;
        }
        Ok(n)
    }

    /// The step body finished: forget its count.
    fn finish(&self, tenant: &str, ordinal: u32, step: &'static str) -> Result<(), WorldError> {
        let mut tries = self.tries.lock().map_err(|poisoned| {
            lgwks_std::trace::debug!(error = %poisoned, "the tries lock was poisoned");
            WorldError::Poisoned
        })?;
        tries.remove(&(key_of(tenant, ordinal), step));
        Ok(())
    }

    /// A local step's body: it is logged once and has nothing to fail on.
    pub(crate) fn ran(&self, tenant: &str, ordinal: u32, step: &'static str) -> Result<(), WorldError> {
        self.begin(tenant, ordinal, step)?;
        self.finish(tenant, ordinal, step)
    }

    /// One attempt of an upstream call: wait its latency, then either fail as the
    /// plan says or return the value the reference model knows.
    pub(crate) async fn call(
        &self,
        call: Call,
        tenant: &str,
        ordinal: u32,
        step: &'static str,
    ) -> Result<u64, WorldError> {
        let attempt = self.begin(tenant, ordinal, step)?;
        self.clock.sleep(call.delay()).await;
        let Some(plan) = usize::try_from(ordinal)
            .ok()
            .and_then(|at| self.plan.get(at).copied())
        else {
            lgwks_std::trace::debug!(ordinal, "a run outside the plan called an upstream");
            return Err(WorldError::OutsidePlan { ordinal });
        };
        let failing = attempt <= call.failing(plan);
        if attempt >= 2 || failing {
            let word = if failing { "failed" } else { "ok" };
            let key = key_of(tenant, ordinal);
            self.attempts
                .append(&format!("{key} {step} {attempt} {word}"))?;
        }
        if failing {
            lgwks_std::trace::debug!(ordinal, attempt, step, "an upstream attempt fails as planned");
            return Err(WorldError::Injected {
                step,
                ordinal,
                attempt,
            });
        }
        self.finish(tenant, ordinal, step)?;
        Ok(call.value(ordinal))
    }

    /// Tell the driver run `key` reached its approval.
    pub(crate) fn parked(&self, key: &str) -> Result<(), WorldError> {
        notice(&format!("{PARKED_WORD} {key}"))
    }

    /// Tell the driver run `key` ended without finishing.
    pub(crate) fn lost(&self, key: &str) -> Result<(), WorldError> {
        notice(&format!("{LOST_WORD} {key}"))
    }
}

/// One whole line on stdout, which the driver reads.
fn notice(line: &str) -> Result<(), WorldError> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}")
        .and_then(|()| out.flush())
        .map_err(|source| WorldError::Io {
            context: "writing a notice to stdout",
            source,
        })
}
