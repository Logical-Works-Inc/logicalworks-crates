//! The steps of one job log: which command is running, what each `##[error]`
//! said and in which step, and the `NEXTEST_BUILD` flags the env dump names.
//!
//! The state is two things: the step now running (opened by a `##[group]Run`
//! line) and a bounded queue of the `##[error]` lines seen, each tagged with the
//! step it came in. The failing step is the one of the last `Process completed
//! with exit code` error, or of the last error of any kind when none says that.

use std::collections::VecDeque;

use super::failure::{MESSAGE_CHARS, REPRO_CHARS};
use super::line::clip;

/// The `##[error]` lines kept. A step that prints more keeps its newest: the
/// failing one is at the end of the log, and the queue is the bound.
const MAX_ERRORS: usize = 256;

/// The most characters of the `NEXTEST_BUILD` value kept.
const FLAG_CHARS: usize = 400;

/// The step now running.
#[derive(Debug)]
struct Active {
    /// How many steps had opened when this one did.
    serial: usize,
    /// The first line of its command, trimmed.
    command: String,
}

/// One `##[error]` line and where it came in.
#[derive(Debug)]
struct ErrorEvent {
    /// The serial of the step running, when one was.
    group: Option<usize>,
    /// That step's command.
    command: Option<String>,
    /// The error's text.
    text: String,
    /// Whether it is the `Process completed with exit code` of a failed command.
    completed: bool,
}

/// The step a failure is held to, and what its errors said.
#[derive(Debug)]
pub(crate) struct Anchor {
    /// The failing step's command.
    pub(crate) command: Option<String>,
    /// The `##[error]` texts of that step, one to a line.
    pub(crate) message: String,
}

/// The step state of one log.
#[derive(Debug, Default)]
pub(crate) struct Steps {
    /// How many steps have opened.
    serial: usize,
    /// The step running.
    active: Option<Active>,
    /// The newest `##[error]` lines, oldest first.
    errors: VecDeque<ErrorEvent>,
    /// The first `NEXTEST_BUILD` value the log's env dumps held.
    flags: Option<String>,
}

impl Steps {
    /// Reads one cleaned line.
    pub(crate) fn feed(&mut self, line: &str) {
        if let Some(rest) = line.strip_prefix("##[group]Run ") {
            self.serial = self.serial.saturating_add(1);
            self.active = Some(Active {
                serial: self.serial,
                command: clip(rest.trim(), REPRO_CHARS),
            });
        } else if let Some(text) = line.strip_prefix("##[error]") {
            self.record_error(text.trim());
        } else if self.flags.is_none()
            && let Some(value) = line.trim().strip_prefix("NEXTEST_BUILD:")
            && !value.trim().is_empty()
        {
            self.flags = Some(clip(value.trim(), FLAG_CHARS));
        }
    }

    /// Queues one `##[error]` text under the step running.
    fn record_error(&mut self, text: &str) {
        if self.errors.len() >= MAX_ERRORS {
            self.errors.pop_front();
            lgwks_std::trace::debug!(limit = MAX_ERRORS, "the oldest ##[error] line was dropped");
        }
        let (group, command) = match self.active.as_ref() {
            Some(active) => (Some(active.serial), Some(active.command.clone())),
            None => (None, None),
        };
        self.errors.push_back(ErrorEvent {
            group,
            command,
            text: text.to_owned(),
            completed: text.starts_with("Process completed with exit code"),
        });
    }

    /// The command of the step running now.
    pub(crate) fn command(&self) -> Option<&str> {
        self.active.as_ref().map(|active| active.command.as_str())
    }

    /// The first `NEXTEST_BUILD` value, as the log printed it.
    pub(crate) fn flags(&self) -> Option<&str> {
        self.flags.as_deref()
    }

    /// The failing step: where the last failed command's error came, with that
    /// step's newest error texts that fit [`MESSAGE_CHARS`]; `None` when the log
    /// holds no `##[error]`.
    pub(crate) fn anchor(&self) -> Option<Anchor> {
        let last = self
            .errors
            .iter()
            .rev()
            .find(|event| event.completed)
            .or_else(|| self.errors.back())?;
        // Newest first, up to the message bound: a step that floods its errors
        // keeps the ones that end it, where the exit code is (sim
        // `sim_ci_receipt_a_giant_log_stays_bounded_and_says_so`).
        let mut texts: Vec<&str> = Vec::new();
        let mut size = 0_usize;
        for event in self
            .errors
            .iter()
            .rev()
            .filter(|event| event.group == last.group && !event.text.is_empty())
        {
            let width = event
                .text
                .chars()
                .count()
                .saturating_add(usize::from(!texts.is_empty()));
            if !texts.is_empty() && size.saturating_add(width) > MESSAGE_CHARS {
                break;
            }
            size = size.saturating_add(width);
            texts.push(event.text.as_str());
        }
        texts.reverse();
        Some(Anchor {
            command: last.command.clone(),
            message: texts.join("\n"),
        })
    }
}
