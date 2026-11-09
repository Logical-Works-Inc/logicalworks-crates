//! `broker-fence`: a broker's fence, driven line by line from standard input.
//!
//! A person or an agent can try what a host does: take over a history, authenticate,
//! withdraw an operation and spend a reach, reading one JSON line per command. The
//! journal argument is a record journal of generations, so a takeover crosses a real
//! process boundary and survives a kill of the process before it.

use std::fmt::Display;
use std::io::{BufRead as _, Write as _};
use std::num::NonZeroU64;
use std::path::Path;
use std::process::ExitCode;

use lgwks_bot::broker::{Authority, Broker, BrokerError};
use lgwks_bot::effect::{
    ActionDigest, ActionId, AttemptId, EffectIdentity, EffectKey, EnvironmentEpoch, EnvironmentId,
    FlowRevision, RunId,
};
use lgwks_bot::journal::{JournalError, RecordJournal};

/// The usage text, for a first-timer and for a misuse.
const USAGE: &str = "usage: broker-fence <journal>
reads one command per line and prints one JSON line for each:
  register                    own the environment at its first generation
  adopt                       take over the generations the journal names
  record <generation>         write a generation to the journal, durably
  replace                     move to the next generation
  authenticate <generation>   the authority source reports this generation
  revoke <action>             withdraw one operation by name, one way
  bound <reaches>             cap the reaches between authentications, 0 stops all
  authorize <action> <gen>    mint a warrant and say what it names
  spend <action> <gen>        mint a warrant and count the reach it makes
  close                       release the environment
  status                      print what the broker holds";

/// The one environment this tool fences.
const ENVIRONMENT: &str = "2122232425262728292a2b2c2d2e2f30";
/// The one digest this tool binds, since a warrant does not read it.
const DIGEST: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

/// Why this tool stopped rather than answering a command.
enum CliError {
    /// The command line or a command could not be read.
    Usage(String),
    /// The journal refused.
    Journal(JournalError),
    /// Standard input or output failed.
    Io(std::io::Error),
}

impl From<JournalError> for CliError {
    fn from(cause: JournalError) -> Self {
        Self::Journal(cause)
    }
}

impl From<std::io::Error> for CliError {
    fn from(cause: std::io::Error) -> Self {
        Self::Io(cause)
    }
}

/// A broker, its one environment, and the journal a takeover reads.
struct Fence {
    /// The broker under test.
    broker: Broker,
    /// The environment every command names.
    id: EnvironmentId,
    /// The generations on disk.
    journal: RecordJournal<u64>,
}

/// A parse that failed, as what the person typed wrong.
fn parsed<T, E: Display>(result: Result<T, E>) -> Result<T, CliError> {
    result.map_err(|cause| CliError::Usage(cause.to_string()))
}

/// One number from a command's arguments.
fn number(word: Option<&str>) -> Result<u64, CliError> {
    word.and_then(|text| text.parse().ok())
        .ok_or_else(|| CliError::Usage(format!("{word:?} is not a number")))
}

/// One generation from a command's arguments.
fn generation(word: Option<&str>) -> Result<EnvironmentEpoch, CliError> {
    NonZeroU64::new(number(word)?)
        .map(EnvironmentEpoch::new)
        .ok_or_else(|| CliError::Usage("generation zero was never issued".to_owned()))
}

/// One operation from a command's arguments.
fn action(word: Option<&str>) -> Result<ActionId, CliError> {
    parsed(ActionId::from_hex(&format!("{:032x}", number(word)?)))
}

/// The key of attempt one of `operation` at `at`.
fn key(
    id: EnvironmentId,
    operation: ActionId,
    at: EnvironmentEpoch,
) -> Result<EffectKey, CliError> {
    let run = parsed(RunId::from_hex(ENVIRONMENT))?;
    let flow = parsed(FlowRevision::from_tagged("blake3_256", DIGEST))?;
    let digest = parsed(ActionDigest::from_tagged("blake3_256", DIGEST))?;
    let attempt = parsed(AttemptId::from_decimal("1"))?;
    Ok(EffectIdentity::new(run, id, flow).key(operation, attempt, digest, at))
}

/// The stable name of a refusal, for a caller that branches on it.
const fn kind(refusal: &BrokerError) -> &'static str {
    match *refusal {
        BrokerError::Revoked { .. } => "revoked",
        BrokerError::Superseded { .. } => "superseded",
        BrokerError::NeverIssued { .. } => "never_issued",
        BrokerError::OfflineExposureExhausted { .. } => "offline_spent",
        BrokerError::RevocationsFull { .. } => "revocations_full",
        BrokerError::Closed { .. } => "closed",
        BrokerError::UnknownEnvironment { .. } => "unknown_environment",
        BrokerError::AlreadyRegistered { .. } => "already_registered",
        BrokerError::NothingToAdopt { .. } => "nothing_to_adopt",
        BrokerError::ForeignEnvironment { .. } => "foreign_environment",
        BrokerError::Journal(_) => "journal",
        BrokerError::Exhausted { .. } => "exhausted",
        _ => "refused",
    }
}

/// What a broker call answered, as one line.
fn answer(result: Result<String, BrokerError>) -> String {
    match result {
        Ok(fields) => format!("{{\"ok\":true,{fields}}}"),
        Err(refusal) => format!(
            "{{\"ok\":false,\"refused\":\"{}\",\"why\":{:?}}}",
            kind(&refusal),
            refusal.to_string()
        ),
    }
}

/// The fields a warrant names.
fn named(warrant: &Authority) -> String {
    format!(
        "\"action\":\"{}\",\"generation\":{}",
        warrant.action().id().to_hex(),
        warrant.epoch().get()
    )
}

/// A number the broker may not hold, as JSON.
fn number_or_null(value: Option<impl Display>) -> String {
    match value {
        Some(held) => held.to_string(),
        None => "null".to_owned(),
    }
}

/// The field a generation answers with.
fn at_generation(at: EnvironmentEpoch) -> String {
    format!("\"generation\":{}", at.get())
}

impl Fence {
    /// Mint a warrant for an operation at a generation, both as the person typed them.
    fn warrant(
        &self,
        operation: Option<&str>,
        at: Option<&str>,
    ) -> Result<Result<Authority, BrokerError>, CliError> {
        let minted = key(self.id, action(operation)?, generation(at)?)?;
        Ok(self.broker.authorize(minted))
    }

    /// What the broker holds now, and whether operation one is withdrawn.
    fn status(&self, watched: ActionId) -> String {
        format!(
            "{{\"ok\":true,\"generation\":{},\"open\":{},\"bound\":{},\"spent\":{},\"action_one_revoked\":{}}}",
            number_or_null(self.broker.epoch(self.id).map(EnvironmentEpoch::get)),
            self.broker.is_open(self.id),
            number_or_null(self.broker.offline_bound(self.id)),
            number_or_null(self.broker.offline_spent(self.id)),
            self.broker.is_revoked(self.id, watched),
        )
    }

    /// Take over the generations on disk.
    fn adopt(&mut self) -> Result<EnvironmentEpoch, BrokerError> {
        let id = self.id;
        self.broker.adopt_records(id, &self.journal, |record| {
            NonZeroU64::new(*record).map(|at| (id, EnvironmentEpoch::new(at)))
        })
    }

    /// Answer one command.
    fn apply(&mut self, line: &str) -> Result<String, CliError> {
        let mut words = line.split_whitespace();
        let command = words.next();
        let (first, second) = (words.next(), words.next());
        let id = self.id;
        Ok(match command {
            Some("register") => answer(self.broker.register(id).map(at_generation)),
            Some("replace") => answer(self.broker.replace(id).map(at_generation)),
            Some("adopt") => answer(self.adopt().map(at_generation)),
            Some("record") => {
                let at = number(first)?;
                self.journal.compare_and_append(self.journal.tail(), &at)?;
                format!("{{\"ok\":true,\"recorded\":{at}}}")
            }
            Some("authenticate") => {
                let reported = generation(first)?;
                answer(self.broker.authenticate(id, reported).map(at_generation))
            }
            Some("revoke") => {
                let operation = action(first)?;
                let withdrew = self.broker.revoke_operation(id, operation);
                answer(withdrew.map(|fresh| format!("\"withdrew\":{fresh}")))
            }
            Some("bound") => {
                let reaches = parsed(u32::try_from(number(first)?))?;
                let set = self.broker.set_offline_bound(id, reaches);
                answer(set.map(|()| format!("\"bound\":{reaches}")))
            }
            Some("authorize") => answer(self.warrant(first, second)?.map(|minted| named(&minted))),
            Some("spend") => answer(self.warrant(first, second)?.and_then(|minted| {
                self.broker.spend(&minted)?;
                Ok(named(&minted))
            })),
            Some("close") => answer(self.broker.close(id).map(|()| "\"closed\":true".to_owned())),
            Some("status") => self.status(action(Some("1"))?),
            other => {
                let why = match other {
                    Some(name) => format!("no command named {name:?}"),
                    None => "an empty line names no command".to_owned(),
                };
                let refusal = Err(CliError::Usage(why));
                lgwks_std::trace::debug!(?other, "broker-fence: answering a mistyped command");
                return refusal;
            }
        })
    }
}

/// Run every line of standard input against one broker.
fn run(journal: &Path) -> Result<(), CliError> {
    let mut fence = Fence {
        broker: Broker::new(),
        id: parsed(EnvironmentId::from_hex(ENVIRONMENT))?,
        journal: RecordJournal::open(journal)?,
    };
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let reply = match fence.apply(&line?) {
            Ok(reply) => reply,
            Err(CliError::Usage(why)) => {
                format!("{{\"ok\":false,\"refused\":\"usage\",\"why\":{why:?}}}")
            }
            Err(CliError::Journal(cause)) => {
                lgwks_std::trace::debug!(%cause, "broker-fence: the journal refused");
                return Err(CliError::Journal(cause));
            }
            Err(CliError::Io(cause)) => {
                lgwks_std::trace::debug!(%cause, "broker-fence: input or output failed");
                return Err(CliError::Io(cause));
            }
        };
        writeln!(out, "{reply}")?;
        out.flush()?;
    }
    Ok(())
}

/// Say why this process is ending, and record a closed pipe rather than panic on it.
fn say(why: &str) {
    let mut handle = std::io::stderr().lock();
    if let Err(cause) = writeln!(handle, "{why}") {
        lgwks_std::trace::debug!(error = ?cause, "standard error is closed");
    }
}

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let (Some(journal), None) = (arguments.next(), arguments.next()) else {
        say(USAGE);
        return ExitCode::from(64);
    };
    match run(Path::new(&journal)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(why)) => {
            say(&format!("{why}\n{USAGE}"));
            ExitCode::from(64)
        }
        Err(CliError::Journal(cause)) => {
            say(&format!("the journal refused: {cause}"));
            ExitCode::from(2)
        }
        Err(CliError::Io(cause)) => {
            say(&format!("input or output failed: {cause}"));
            ExitCode::FAILURE
        }
    }
}
