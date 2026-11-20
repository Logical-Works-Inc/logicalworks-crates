//! The getting-started program: one source, one condition, one effect, written
//! as a `script!` flow.
//!
//! ```text
//! cargo run -p lgwks_bot --example queue_watch
//! ```
//!
//! The flow reads a queue's depth through the `queue::depth` source its host's
//! registry declares and, over ten, runs the registry's `notify::page` action.
//! The program runs it three times and prints what each run did: at depth 3
//! nothing is paged, at depth 11 a host granted `bot.notify` pages once, and the
//! same flow on a host with no grant is refused before the page runs. Output
//! goes through `std::io::Write` because `clippy::print_stdout` is forbidden
//! workspace-wide, examples included.

use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use lgwks_bot::cap::{Auth, Cap};
use lgwks_bot::domains;
use lgwks_bot::ecs::EcsError;
use lgwks_bot::gate::GrantSet;
use lgwks_bot::registry::{Action, Source};
use lgwks_bot::script::{FlowError, Scope};
use lgwks_bot::task::{Host, HostError, task};
use lgwks_bot::verb::{EffectLifetime, Execute, Observe};

/// Why the program failed, one variant per step.
#[derive(Debug)]
enum WatchError {
    /// The host refused to build.
    Host(HostError),
    /// The flow refused to register as a task.
    Task(FlowError),
    /// The run was refused or failed; its report, as text.
    Run(String),
    /// A host without the grant ran the page.
    Ungated,
    /// Writing the output failed.
    Output(std::io::Error),
}

impl std::fmt::Display for WatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Host(ref cause) => write!(formatter, "the host did not build: {cause}"),
            Self::Task(ref cause) => write!(formatter, "the task did not register: {cause}"),
            Self::Run(ref cause) => formatter.write_str(cause),
            Self::Ungated => formatter.write_str("a host without the grant ran the page"),
            Self::Output(ref cause) => write!(formatter, "the output was not written: {cause}"),
        }
    }
}

impl std::error::Error for WatchError {}

impl From<HostError> for WatchError {
    fn from(cause: HostError) -> Self {
        Self::Host(cause)
    }
}

impl From<FlowError> for WatchError {
    fn from(cause: FlowError) -> Self {
        Self::Task(cause)
    }
}

impl From<std::io::Error> for WatchError {
    fn from(cause: std::io::Error) -> Self {
        Self::Output(cause)
    }
}

/// The queue's depth. A real source reads a socket, a file or an API; this one
/// reads a counter the program owns, so every run is deterministic.
static DEPTH: AtomicU32 = AtomicU32::new(3);

/// Pages delivered so far.
static PAGES: AtomicU32 = AtomicU32::new(0);

/// The observed source: how deep the queue is.
struct QueueDepth;

impl QueueDepth {
    /// Build the source from the name a flow gives after `of`.
    fn from_target(_queue: &str) -> Source {
        Source::ordered(Self)
    }
}

/// The registry's constructor for [`QueueDepth`], which cannot fail.
const DEPTH_SOURCE: fn(&str) -> Result<Source, EcsError> =
    |queue| Ok(QueueDepth::from_target(queue));

impl Observe for QueueDepth {
    type Output = u32;

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    async fn poll(&self, call: (Auth, ())) -> Result<u32, EcsError> {
        call.0.check(self.required_caps())?;
        Ok(DEPTH.load(Ordering::SeqCst))
    }

    fn domain_id(&self) -> &str {
        "queue::depth"
    }
}

/// The effect: delivering a page is what needs `bot.notify`, so that is the
/// capability the action declares.
struct Page {
    /// What the action declares it needs.
    caps: [Cap; 1],
}

impl Page {
    /// Build the action from the name a flow gives after `on`.
    fn from_target(_who: &str) -> Action {
        Action::new(Self {
            caps: [Cap::notify()],
        })
    }
}

/// The registry's constructor for [`Page`], which cannot fail.
const PAGE_ACTION: fn(&str) -> Result<Action, EcsError> = |who| Ok(Page::from_target(who));

impl Execute for Page {
    type Input = u32;
    type Output = ();

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    /// The page only moves a counter this process owns. An action that does not
    /// say so is treated as reaching outside the process, and `act` refuses it.
    fn effect_lifetime(&self) -> EffectLifetime {
        EffectLifetime::Local
    }

    async fn execute_action(&self, call: (Auth, &u32)) -> Result<(), EcsError> {
        // Checked before the effect, not after it.
        call.0.check(self.required_caps())?;
        PAGES.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn domain_id(&self) -> &str {
        "notify::page"
    }
}

domains! {
    /// What the flow below may name: one source and one action.
    QUEUE_DOMAINS {
        observe {
            "queue::depth" => DEPTH_SOURCE,
        }
        execute {
            "notify::page" => PAGE_ACTION,
        }
    }
}

lgwks_bot::script! {
    /// Read the depth of the queue called `name`; page on call when it is over ten.
    flow watch(name: &str) -> u32:
        let depth: u32 = observe queue::depth of name
        if depth > 10:
            act notify::page on "oncall" with depth
        give back depth
}

/// A host for the `queue-watch` tenant over [`QUEUE_DOMAINS`], holding `grants`.
fn host(grants: GrantSet) -> Result<Host, WatchError> {
    Ok(Host::builder("queue-watch")?
        .domains(&QUEUE_DOMAINS)
        .grants(grants)
        .default_deadline(Duration::from_secs(30))
        .build()?)
}

/// Set the queue to `depth` and run the flow once on `host`: the depth it read,
/// or the reason the run was refused.
fn run_at(host: &Host, depth: u32) -> Result<u32, WatchError> {
    DEPTH.store(depth, Ordering::SeqCst);
    let watching = task("watching", |scope: Scope, name: &'static str| async move {
        watch(&scope, name).await
    })?;
    let report = lgwks_bot::script::block_on(host.run(&watching, "jobs"));
    report
        .result()
        .copied()
        .map_err(|failure| WatchError::Run(failure.to_string()))
}

fn main() -> Result<(), WatchError> {
    let mut out = std::io::stdout().lock();

    // The grant belongs to the host: a flow can only page where the host was
    // granted `bot.notify`.
    let granted = host(GrantSet::empty().grant(Cap::notify()))?;

    // Depth 3 is under the threshold, so nothing is paged.
    let seen = run_at(&granted, 3)?;
    let pages = PAGES.load(Ordering::SeqCst);
    writeln!(out, "depth {seen}, pages delivered: {pages}")?;
    assert_eq!(pages, 0, "depth 3 is under the threshold");

    // Depth 11 is over it, so the action runs once.
    let seen = run_at(&granted, 11)?;
    let pages = PAGES.load(Ordering::SeqCst);
    writeln!(out, "depth {seen}, pages delivered: {pages}")?;
    assert_eq!(pages, 1, "one page was delivered");

    // The same flow on a host with no grant reads the depth, then is refused at
    // the page, which never runs.
    let refusal = run_at(&host(GrantSet::empty())?, 11)
        .err()
        .ok_or(WatchError::Ungated)?;
    let pages = PAGES.load(Ordering::SeqCst);
    writeln!(out, "no grant: {refusal}")?;
    assert_eq!(pages, 1, "a refused page is not delivered");
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The guide's program, run as the guide shows it: three runs, the page
    /// count asserted after each, and the ungranted host refused (#896).
    #[test]
    fn the_getting_started_program_runs() -> Result<(), super::WatchError> {
        super::main()
    }
}
