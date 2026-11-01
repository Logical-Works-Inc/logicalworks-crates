//! The outside-words test rig (#783), shared by the point tests and the
//! seeded families so a refusal reads the same in both.
//!
//! [`StubMailer`] is the [`SendPort`](lgwks_bot::script::SendPort) the tests
//! dispatch through: it counts dispatches, holds what landed under each key,
//! remembers the last key it dispatched, and can lose a reply, fail a read or
//! refuse a dispatch on demand, so the crash windows are draws rather than
//! races. [`Upstream`] is the loopback HTTP server the `fetch` tests dial:
//! one fixed behavior per server — slow, dropping, storming, oversized or
//! well — counted per connection and joined when the test ends, so no work
//! outlives the test that started it.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lgwks_bot::journal::EffectEvidence;
use lgwks_bot::script::{FlowError, SendOutcome, SendPort};

/// What an outside test can fail with: every arm is a typed refusal from the
/// run path, so a failure names the fact that moved rather than a string.
#[derive(Debug)]
pub(crate) enum OutsideError {
    /// The run failed.
    Flow(FlowError),
    /// The host was refused.
    Host(lgwks_bot::task::HostError),
    /// The store was refused.
    Store(lgwks_bot::task::StoreError),
    /// The device refused.
    Io(std::io::Error),
    /// The fixture could not be built, or a run ended other than the test
    /// required.
    Setup { what: &'static str },
}

impl std::fmt::Display for OutsideError {
    /// The refusal behind the failure.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Flow(ref error) => write!(formatter, "the run failed: {error}"),
            Self::Host(ref error) => write!(formatter, "the host failed: {error}"),
            Self::Store(ref error) => write!(formatter, "the store failed: {error}"),
            Self::Io(ref error) => write!(formatter, "the device failed: {error}"),
            Self::Setup { what } => write!(formatter, "the fixture failed: {what}"),
        }
    }
}

impl std::error::Error for OutsideError {
    /// The refusal behind the failure, when it has one.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Flow(ref error) => Some(error),
            Self::Host(ref error) => Some(error),
            Self::Store(ref error) => Some(error),
            Self::Io(ref error) => Some(error),
            Self::Setup { .. } => None,
        }
    }
}

impl From<FlowError> for OutsideError {
    fn from(error: FlowError) -> Self {
        Self::Flow(error)
    }
}

impl From<lgwks_bot::task::HostError> for OutsideError {
    fn from(error: lgwks_bot::task::HostError) -> Self {
        Self::Host(error)
    }
}

impl From<lgwks_bot::task::StoreError> for OutsideError {
    fn from(error: lgwks_bot::task::StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<std::io::Error> for OutsideError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<Box<dyn std::error::Error>> for OutsideError {
    fn from(error: Box<dyn std::error::Error>) -> Self {
        Self::Io(std::io::Error::other(error.to_string()))
    }
}

/// Log and return the fixture refusal `what`, so the trace names the signal.
pub(crate) fn fail<T>(what: &'static str) -> Result<T, OutsideError> {
    lgwks_std::trace::debug!(what, "outside fixture: refusing the test");
    Err(OutsideError::Setup { what })
}

/// A host for `tenant` over the run store in `dir`, holding `grants`.
pub(crate) fn host_on(
    dir: &std::path::Path,
    tenant: &str,
    grants: lgwks_bot::GrantSet,
) -> Result<lgwks_bot::task::Host, OutsideError> {
    Ok(lgwks_bot::task::Host::builder(tenant)?
        .run_store(dir)?
        .grants(grants)
        .build()?)
}

/// Grants for the three outside capabilities.
pub(crate) fn outside_grants() -> lgwks_bot::GrantSet {
    use lgwks_bot::Cap;
    lgwks_bot::GrantSet::empty()
        .grant(Cap::net())
        .grant(Cap::new("bot.model"))
        .grant(Cap::notify())
}

/// The run id a report names, or a refusal when the run kept nothing.
pub(crate) fn run_of(
    report: &lgwks_bot::task::Report<SendOutcome>,
) -> Result<lgwks_bot::effect::RunId, OutsideError> {
    match report.run_id() {
        Some(run) => Ok(run),
        None => fail("a durable run names its run"),
    }
}

lgwks_bot::script! {
    /// GET the URL under options.
    pub(crate) flow get(url: &str, options: &lgwks_bot::script::FetchOptions) -> u16:
        let answered = fetch url with (options)
        give back answered.status()

    /// Send the payload through the mailer.
    pub(crate) flow mail(mailer: &StubMailer, payload: Vec<u8>) -> SendOutcome:
        let outcome = send mailer with (payload)
        give back outcome

    /// Send, then fail when `crash` is set: the crash between dispatch and
    /// settlement, drawn without a race.
    pub(crate) flow mail_crash(mailer: &StubMailer, payload: Vec<u8>, crash: bool) -> SendOutcome:
        let outcome = send mailer with (payload)
        if crash:
            fail with "crashed between dispatch and settlement"
        give back outcome

    /// All three outside words, for the admission and the plan.
    pub(crate) flow calls(
        url: &str,
        model: &lgwks_bot::proposal::StubModel,
        mailer: &StubMailer,
        payload: Vec<u8>,
    ) -> (u16, String, SendOutcome):
        let code = fetch url with (lgwks_bot::script::FetchOptions::default())
        let reply: String = ask model with (lgwks_bot::script::AskRequest::default())
        let outcome = send mailer with (payload)
        give back (code.status(), reply, outcome)
}

/// The port's own failure, as the stub reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MailError {
    /// The receiver refused the dispatch and landed nothing.
    Refused,
    /// The dispatch may have landed and the reply was lost.
    Lost,
    /// The read-back failed.
    Unreadable,
}

impl std::fmt::Display for MailError {
    /// The arm, as the run reports it.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Refused => formatter.write_str("the receiver refused the dispatch"),
            Self::Lost => formatter.write_str("the dispatch reply was lost"),
            Self::Unreadable => formatter.write_str("the read-back failed"),
        }
    }
}

impl std::error::Error for MailError {}

impl From<MailError> for FlowError {
    fn from(error: MailError) -> Self {
        match error {
            MailError::Refused => FlowError::failed("the stub receiver refused the dispatch"),
            MailError::Lost => {
                FlowError::transient("the stub dispatch reply was lost after it may have landed")
            }
            MailError::Unreadable => {
                FlowError::failed("the stub read-back failed and nothing is known")
            }
        }
    }
}

/// The stub receiver every `send` test dispatches through.
///
/// What landed is held under each key, so a resume reads back exactly what an
/// earlier attempt left; the last dispatched key is remembered, so late
/// evidence can land where the dispatch went. The three fault flags draw the
/// crash windows without a race: `lose_reply` lands nothing and reports the
/// reply lost, `fail_read` fails the next read-back, and `refuse_dispatch`
/// lands nothing and says so.
pub(crate) struct StubMailer {
    /// Dispatches attempted, whatever they reported.
    calls: AtomicUsize,
    /// What landed under each key.
    landed: Mutex<HashMap<Vec<u8>, EffectEvidence>>,
    /// The last key a dispatch was attempted under, for late evidence.
    last_key: Mutex<Option<Vec<u8>>>,
    /// Fail the next read-back instead of answering it.
    fail_read: AtomicBool,
    /// Report the next dispatch lost, landing nothing.
    lose_reply: AtomicBool,
    /// Refuse the next dispatch, landing nothing.
    refuse_dispatch: AtomicBool,
}

impl StubMailer {
    /// A stub holding nothing, refusing nothing.
    pub(crate) fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            landed: Mutex::new(HashMap::new()),
            last_key: Mutex::new(None),
            fail_read: AtomicBool::new(false),
            lose_reply: AtomicBool::new(false),
            refuse_dispatch: AtomicBool::new(false),
        }
    }

    /// Dispatches attempted so far.
    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Fail the next read-back.
    pub(crate) fn fail_next_read(&self) {
        self.fail_read.store(true, Ordering::SeqCst);
    }

    /// Report the next dispatch lost, landing nothing.
    pub(crate) fn lose_next_reply(&self) {
        self.lose_reply.store(true, Ordering::SeqCst);
    }

    /// Refuse the next dispatch, landing nothing.
    pub(crate) fn refuse_next_dispatch(&self) {
        self.refuse_dispatch.store(true, Ordering::SeqCst);
    }

    /// Forget everything that landed, as a crash that takes the evidence does.
    pub(crate) fn forget(&self) {
        if let Ok(mut held) = self.landed.lock() {
            held.clear();
        }
    }

    /// Land applied evidence under the last dispatched key, as evidence that
    /// arrives after the crash does.
    pub(crate) fn land_applied(&self) {
        let key = match self.last_key.lock() {
            Ok(held) => held.clone(),
            Err(_) => None,
        };
        if let Some(key) = key
            && let Ok(mut held) = self.landed.lock()
        {
            held.insert(key, EffectEvidence::Applied);
        }
    }
}

impl SendPort for StubMailer {
    /// The stub's own typed failure.
    type Error = MailError;

    /// What landed under `key`, or the next read-back failure.
    async fn find(&self, key: &[u8]) -> Result<Option<EffectEvidence>, MailError> {
        if self.fail_read.swap(false, Ordering::SeqCst) {
            let refusal = Err(MailError::Unreadable);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "outside fixture: the stub read-back failed");
            return refusal;
        }
        match self.landed.lock() {
            Ok(held) => Ok(held.get(key).copied()),
            Err(_) => Err(MailError::Unreadable),
        }
    }

    /// Hand over once under `key`, unless told to lose or refuse it.
    async fn dispatch(&self, key: &[u8], _payload: &[u8]) -> Result<EffectEvidence, MailError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut held) = self.last_key.lock() {
            *held = Some(key.to_vec());
        }
        if self.refuse_dispatch.swap(false, Ordering::SeqCst) {
            let refusal = Err(MailError::Refused);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "outside fixture: the stub refused the dispatch");
            return refusal;
        }
        if self.lose_reply.swap(false, Ordering::SeqCst) {
            let refusal = Err(MailError::Lost);
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "outside fixture: the stub lost the dispatch reply");
            return refusal;
        }
        match self.landed.lock() {
            Ok(mut held) => {
                held.insert(key.to_vec(), EffectEvidence::Applied);
                Ok(EffectEvidence::Applied)
            }
            Err(_) => Err(MailError::Unreadable),
        }
    }

    /// Only a lost reply leaves the effect unknown.
    fn may_have_landed(error: &MailError) -> bool {
        matches!(*error, MailError::Lost)
    }
}

/// What one loopback upstream serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Behavior {
    /// Answer 200 with a small body at once.
    Ok,
    /// Sleep past the caller's deadline, then answer 200.
    Slow,
    /// Send headers and half a body, then close the connection.
    DropMidBody,
    /// Answer 500 to every request.
    Storm,
    /// Answer 200 with `bytes` of body.
    Oversized(usize),
}

/// One loopback HTTP server with a fixed behavior, counted and joined.
///
/// The server answers at most `expect` connections on the shared blocking
/// pool, then its task ends and `join` collects it. A test that fails early
/// drops the server instead, and the drop cancels it — the stop flag goes up
/// and one dial wakes the parked `accept` — so no work outlives the test that
/// started it. A client that times out leaves the server writing to a closed
/// socket, which ends that connection without ending the task's accounting.
pub(crate) struct Upstream {
    /// The port the server listens on.
    port: u16,
    /// Connections answered so far.
    hits: Arc<AtomicUsize>,
    /// Raised to end the server before its `expect` connections arrive.
    cancelled: Arc<AtomicBool>,
    /// The server task, joined by `join`.
    task: Option<lgwks_std::task::JoinHandle<()>>,
}

impl Upstream {
    /// Serve `behavior` on a loopback port for at most `expect` connections.
    pub(crate) fn serve(
        behavior: Behavior,
        expect: usize,
        delay: Duration,
    ) -> Result<Self, OutsideError> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&hits);
        let cancelled = Arc::new(AtomicBool::new(false));
        let watched = Arc::clone(&cancelled);
        let task = lgwks_std::task::try_spawn_blocking(move || {
            for _ in 0..expect {
                let stream = match listener.accept() {
                    Ok(accepted) => accepted.0,
                    Err(failed) => {
                        lgwks_std::trace::debug!(error = ?failed, "upstream: accept failed");
                        return;
                    }
                };
                if watched.load(Ordering::SeqCst) {
                    return;
                }
                counted.fetch_add(1, Ordering::SeqCst);
                serve_one(stream, behavior, delay);
            }
        })
        .map_err(std::io::Error::other)?;
        Ok(Self {
            port,
            hits,
            cancelled,
            task: Some(task),
        })
    }

    /// The URL to dial.
    pub(crate) fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// Connections answered so far.
    pub(crate) fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    /// Join the server task, which has answered its connections by now.
    pub(crate) fn join(mut self) {
        if let Some(task) = self.task.take() {
            lgwks_std::task::block_on(task);
        }
    }
}

impl Drop for Upstream {
    /// Cancel a server the test never joined, so its task ends on its own.
    fn drop(&mut self) {
        if self.task.is_some() {
            self.cancelled.store(true, Ordering::SeqCst);
            if let Err(failed) = std::net::TcpStream::connect(("127.0.0.1", self.port)) {
                lgwks_std::trace::debug!(error = ?failed, "upstream: the server had already ended");
            }
        }
    }
}

/// Answer one connection with `behavior`.
fn serve_one(mut stream: std::net::TcpStream, behavior: Behavior, delay: Duration) {
    // Read the whole request — headers plus the promised body — before
    // answering: closing on unread bytes resets the connection and turns the
    // client's body read into a drop, which is exactly the flake a probe that
    // answers from the headers alone buys.
    let mut received = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        received.extend_from_slice(&chunk[..read]);
        if received.len() > 2_097_152 {
            return;
        }
        let Some(end) = header_end(&received) else {
            continue;
        };
        let framing = framing_of(&received[..end]);
        let complete = match framing {
            Framing::Chunked => received[end..]
                .windows(5)
                .any(|window| window == b"0\r\n\r\n"),
            Framing::Sized(length) => received.len() >= end.saturating_add(length),
        };
        if complete {
            break;
        }
    }
    match behavior {
        Behavior::Ok => {
            let body = b"hello";
            reply(&mut stream, 200, body);
        }
        Behavior::Slow => {
            // Wait out the delay on a condition that never arrives: the late
            // reply lands on a socket the caller already closed, and no
            // executor thread is held for it.
            let gate = std::sync::Mutex::new(false);
            let bell = std::sync::Condvar::new();
            let held = match gate.lock() {
                Ok(held) => held,
                Err(_) => return,
            };
            if bell.wait_timeout_while(held, delay, |open| !*open).is_err() {
                return;
            }
            let body = b"late";
            reply(&mut stream, 200, body);
        }
        Behavior::DropMidBody => {
            let head = b"HTTP/1.1 200 OK\r\nContent-Length: 64\r\nConnection: close\r\n\r\n";
            drop(stream.write_all(head));
            drop(stream.write_all(b"half of a body, cut "));
        }
        Behavior::Storm => {
            let body = b"down";
            reply(&mut stream, 500, body);
        }
        Behavior::Oversized(bytes) => {
            let head =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {bytes}\r\nConnection: close\r\n\r\n");
            if stream.write_all(head.as_bytes()).is_err() {
                return;
            }
            let chunk = vec![b'x'; 8_192];
            let mut left = bytes;
            while left > 0 {
                let width = left.min(chunk.len());
                if stream.write_all(&chunk[..width]).is_err() {
                    return;
                }
                left = left.saturating_sub(width);
            }
        }
    }
}

/// The end of the headers in `received`, one past the blank line, when the
/// headers are complete.
fn header_end(received: &[u8]) -> Option<usize> {
    received
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|at| at.saturating_add(4))
}

/// How the request's body ends on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    /// `Transfer-Encoding: chunked`: the body ends at its zero chunk.
    Chunked,
    /// A counted body of `Content-Length` bytes, or none.
    Sized(usize),
}

/// The framing the headers declare: chunked wins over a length, as the
/// specification orders them, and anything undeclared is an empty body for
/// the shapes this server answers.
fn framing_of(head: &[u8]) -> Framing {
    let text = match std::str::from_utf8(head) {
        Ok(text) => text,
        Err(_) => return Framing::Sized(0),
    };
    let mut chunked = false;
    let mut length = 0_usize;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:")
            && let Ok(declared) = value.trim().parse::<usize>()
        {
            length = declared.min(1_048_576);
        }
        if let Some(value) = lower.strip_prefix("transfer-encoding:") {
            chunked = value.split(',').any(|coding| coding.trim() == "chunked");
        }
    }
    if chunked {
        Framing::Chunked
    } else {
        Framing::Sized(length)
    }
}

/// Answer with `status` and `body`.
fn reply(stream: &mut std::net::TcpStream, status: u16, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reason(status),
        body.len()
    );
    drop(stream.write_all(head.as_bytes()));
    drop(stream.write_all(body));
}

/// The reason phrase for a served status.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

/// Whether the outcome is the run's unknown rather than a settlement.
pub(crate) fn is_unknown(outcome: &SendOutcome) -> bool {
    matches!(*outcome, SendOutcome::Unknown)
}

/// Whether the outcome settles on the evidence the stub holds.
pub(crate) fn is_applied(outcome: &SendOutcome) -> bool {
    matches!(*outcome, SendOutcome::Settled(EffectEvidence::Applied))
}
