//! The smoke's outside-words section (#783): `fetch`, `ask` and `send`.
//!
//! One loopback server answers every `fetch` the smoke dials, [`StubModel`]
//! answers every `ask`, and a stub [`SendPort`] lands every `send`, all
//! through the public run path: the `script!` flows below expand to the same
//! `_at` calls and the same [`admit_outside`] entry a product flow expands
//! to, and the direct calls beside them name every new `script` item the
//! estate exposes, which is what makes this module the surface guard's caller
//! for them.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use lgwks_bot::GrantSet;
use lgwks_bot::cap::Cap;
use lgwks_bot::journal::EffectEvidence;
use lgwks_bot::proposal::StubModel;
use lgwks_bot::rt::runtime::Handle;
use lgwks_bot::script::{
    AskDecode, AskRequest, EffectDisposition, EffectPolicy, FetchOptions, FetchResponse, FlowError,
    Json, ModelOutput, ModelPort, OutsideWord, PostBody, Scope, SendOutcome, SendPort, Tenant,
    admit_outside, ask, fetch, send, with_effect_policy,
};
use lgwks_bot::task::{Host, task};
use lgwks_bot::verb::EffectLifetime;

use super::super::say;
use super::{Section, VocabularyError, refuse};

/// A JSON report, as the smoke's model stub answers it.
#[derive(Debug, PartialEq, Eq, lgwks_std::json::Serialize, lgwks_std::json::Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct Report {
    /// The status the model reported.
    code: u16,
}

/// A model that answers one fixed JSON report, for the decode check.
struct JsonModel;

impl ModelPort for JsonModel {
    /// Never fails: the answer is fixed.
    type Error = FlowError;

    /// The fixed report.
    async fn complete(&self, _prompt: &[u8], _max_tokens: u32) -> Result<ModelOutput, FlowError> {
        Ok((br#"{"code":200}"#.to_vec(), false).into())
    }
}

/// The stub receiver every smoke `send` dispatches through.
///
/// What landed is kept as an ordered log rather than a map, so a read-back
/// scans it linearly: the smoke sends a handful of effects in total, and the
/// log stays the honest record of what the receiver holds.
struct Mailer {
    /// Dispatches attempted, whatever they reported.
    calls: AtomicUsize,
    /// What landed, in arrival order.
    landed: Mutex<Vec<(Vec<u8>, EffectEvidence)>>,
}

impl Mailer {
    /// A mailer holding nothing.
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            landed: Mutex::new(Vec::new()),
        }
    }

    /// Dispatches attempted so far.
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The landed log, refused typed when a panicked holder poisoned it.
    fn held(&self) -> Result<std::sync::MutexGuard<'_, Landed>, FlowError> {
        match self.landed.lock() {
            Ok(held) => Ok(held),
            Err(poisoned) => {
                let refusal = Err(FlowError::failed(format!(
                    "the smoke mailer lock failed: {poisoned}"
                )));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "mailer: the landed log is poisoned");
                refusal
            }
        }
    }
}

/// The mailer's landed log: each key it dispatched and what landed under it.
type Landed = Vec<(Vec<u8>, EffectEvidence)>;

/// What landed under `key` in `held`, or nothing.
fn landed_under(held: &[(Vec<u8>, EffectEvidence)], key: &[u8]) -> Option<EffectEvidence> {
    held.iter()
        .find(|landed| landed.0.as_slice() == key)
        .map(|landed| landed.1)
}

impl SendPort for Mailer {
    /// The stub never fails: the smoke draws no faults here.
    type Error = FlowError;

    /// What landed under `key`, or nothing.
    async fn find(&self, key: &[u8]) -> Result<Option<EffectEvidence>, FlowError> {
        Ok(landed_under(&self.held()?, key))
    }

    /// Hand over once under `key`, idempotent on a second call.
    async fn dispatch(&self, key: &[u8], _payload: &[u8]) -> Result<EffectEvidence, FlowError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut held = self.held()?;
        if let Some(evidence) = landed_under(&held, key) {
            return Ok(evidence);
        }
        held.push((key.to_vec(), EffectEvidence::Applied));
        Ok(EffectEvidence::Applied)
    }

    /// The stub never reports an unknown landing.
    fn may_have_landed(_error: &FlowError) -> bool {
        false
    }
}

/// The smoke's explicit policy: the stub mailer and the loopback server are
/// test doubles, so an allow runs them without parking a decision.
struct AllowOutside;

impl EffectPolicy for AllowOutside {
    /// Every effect is reversible: the smoke owns its doubles.
    fn disposition(&self, _lifetime: EffectLifetime) -> EffectDisposition {
        EffectDisposition::Reversible
    }
}

/// One loopback HTTP server answering a fixed body, counted and joined.
///
/// The server answers at most `expect` connections on the shared blocking
/// pool, then its task ends and `join` collects it. A section that stops
/// early drops the server instead, and the drop cancels it: it raises the
/// stop flag and dials the port once, so the parked `accept` returns and the
/// task ends without answering. No work outlives the section that started it.
struct OnceUp {
    /// The port the server listens on.
    port: u16,
    /// Connections answered so far.
    hits: Arc<AtomicUsize>,
    /// Raised to end the server before its `expect` connections arrive.
    stop: Arc<AtomicBool>,
    /// The server task, joined by `join`.
    task: Option<lgwks_std::task::JoinHandle<()>>,
}

impl OnceUp {
    /// Serve `body` on a loopback port for at most `expect` connections.
    fn serve(body: Vec<u8>, expect: usize) -> Result<Self, VocabularyError> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&hits);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let task = lgwks_std::task::try_spawn_blocking(move || {
            for _ in 0..expect {
                let (mut stream, _) = match listener.accept() {
                    Ok(accepted) => accepted,
                    Err(failed) => {
                        lgwks_std::trace::debug!(error = ?failed, "smoke server: accept failed");
                        return;
                    }
                };
                if stopped.load(Ordering::SeqCst) {
                    return;
                }
                counted.fetch_add(1, Ordering::SeqCst);
                // Read the whole request — headers plus the promised body, in
                // whatever case or framing it arrives in — before answering:
                // closing on unread bytes resets the connection and turns the
                // client's body read into a drop.
                let mut received = Vec::new();
                let mut chunk = [0_u8; 1024];
                loop {
                    let read = match stream.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(read) => read,
                        Err(failed) => {
                            lgwks_std::trace::debug!(error = ?failed, "smoke server: the request read ended");
                            break;
                        }
                    };
                    received.extend_from_slice(&chunk[..read]);
                    if received.len() > 2_097_152 {
                        break;
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
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                drop(stream.write_all(head.as_bytes()));
                drop(stream.write_all(&body));
            }
        })
        .map_err(std::io::Error::other)?;
        Ok(Self {
            port,
            hits,
            stop,
            task: Some(task),
        })
    }

    /// The URL to dial.
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// Connections answered so far.
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    /// Join the server task on the smoke's driver, once it has answered every
    /// connection it was started for.
    fn join(mut self, driver: &Handle) -> Result<(), VocabularyError> {
        if let Some(task) = self.task.take() {
            driver.block_on(task);
        }
        Ok(())
    }
}

impl Drop for OnceUp {
    /// Cancel a server the section did not join: raise the stop flag and wake
    /// the parked `accept` with one dial, so the task ends on its own.
    fn drop(&mut self) {
        if self.task.is_none() {
            return;
        }
        self.stop.store(true, Ordering::SeqCst);
        if let Err(failed) = std::net::TcpStream::connect(("127.0.0.1", self.port)) {
            lgwks_std::trace::debug!(error = ?failed, "smoke server: the wake-up dial failed; the task ended already");
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

lgwks_bot::script! {
    /// GET the URL under options.
    flow smoke_fetch(url: &str, options: &FetchOptions) -> u16:
        let answered = fetch url with (options)
        give back answered.status()

    /// Ask the model, decoded as text.
    flow smoke_ask(model: &StubModel, request: &AskRequest) -> String:
        let reply: String = ask model with (request)
        give back reply

    /// Send the payload through the mailer.
    flow smoke_send(mailer: &Mailer, payload: Vec<u8>) -> SendOutcome:
        let outcome = send mailer with (payload)
        give back outcome

    /// All three outside words, for the admission and the plan.
    flow smoke_all(
        url: &str,
        model: &StubModel,
        mailer: &Mailer,
        payload: Vec<u8>,
    ) -> (u16, String, SendOutcome):
        let code = fetch url with (FetchOptions::default())
        let reply: String = ask model with (AskRequest::default())
        let outcome = send mailer with (payload)
        give back (code.status(), reply, outcome)
}

/// Drive the outside section over a fresh store of its own.
///
/// # Errors
///
/// The first failed check, or the runtime error the run returned.
pub(super) fn run(driver: &Handle) -> Result<(), VocabularyError> {
    let unique = lgwks_std::random::bytes::<8>().map_err(std::io::Error::other)?;
    let hex: String = unique.iter().map(|byte| format!("{byte:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("lgwks-estate-outside-{hex}"));
    std::fs::create_dir_all(&dir)?;
    let _keep = super::super::Scratch(dir.clone());
    let grants = GrantSet::empty()
        .grant(Cap::net())
        .grant(Cap::new("bot.model"))
        .grant(Cap::notify());
    let host = Host::builder("vocab")?
        .run_store(&dir)?
        .grants(grants)
        .build()?;
    // Four exchanges: one word GET, one direct GET, one combined GET and one
    // word POST under the allowing policy.
    let upstream = OnceUp::serve(b"hello".to_vec(), 4)?;
    let url = upstream.url();

    let fetching = task("outside-fetch", |scope: Scope, url: String| async move {
        let options: FetchOptions = FetchOptions::default();
        smoke_fetch(&scope, &url, &options).await
    })?;
    let report = driver.block_on(host.run(&fetching, url.clone()));
    if report.output() != Some(&200_u16) {
        return refuse(Section::Outside, "a word GET answers its status");
    }
    say("outside: a word GET answers 200")?;

    let scope = Scope::root(Tenant::new("vocab")?);
    let direct_options: FetchOptions = FetchOptions::default()
        .max_body_bytes(65_536)
        .deadline(std::time::Duration::from_secs(10))
        .attempts(1)
        .header("accept", "text/plain");
    let direct: FetchResponse =
        driver.block_on(fetch(&scope, "outside-direct", &url, &direct_options))?;
    if direct.status() != 200 || direct.body() != b"hello" {
        return refuse(Section::Outside, "a direct GET answers the whole body");
    }
    if direct.headers().is_empty() {
        return refuse(Section::Outside, "a direct GET carries its headers");
    }
    say("outside: a direct GET answers the whole body")?;

    // The model headline runs against StubModel: the first seed whose output
    // is whole, so bytes decode without a truncation refusal.
    let mut seed = 0_u64;
    let model = loop {
        let candidate = StubModel::from_seed(seed);
        let probe: ModelOutput = driver.block_on(ModelPort::complete(&candidate, b"hi", 1_000))?;
        if !probe.truncated() {
            break candidate;
        }
        seed = seed.saturating_add(1);
        if seed > 64 {
            return refuse(Section::Outside, "a seed answers whole output");
        }
    };
    let asking = task("outside-ask", |scope: Scope, model: StubModel| async move {
        let request: AskRequest = AskRequest::default()
            .prompt(b"steady".to_vec())
            .max_tokens(1_000)
            .deadline(std::time::Duration::from_secs(10));
        smoke_ask(&scope, &model, &request).await
    })?;
    let answered = driver.block_on(host.run(&asking, model.clone()));
    let Some(text) = answered.output() else {
        return refuse(Section::Outside, "a word ask decodes text");
    };
    if text.as_bytes() != model.output() {
        return refuse(Section::Outside, "a word ask decodes the model's bytes");
    }
    say("outside: a word ask decodes the model's bytes")?;

    let request: AskRequest = AskRequest::default();
    let bytes: Vec<u8> = driver.block_on(ask(&scope, "outside-ask-direct", &model, &request))?;
    if bytes != model.output() {
        return refuse(Section::Outside, "a direct ask decodes whole bytes");
    }
    let back: Vec<u8> = Vec::<u8>::decode(&bytes)?;
    if back != bytes {
        return refuse(Section::Outside, "bytes decode through the trait door");
    }
    let made: ModelOutput = (b"hi".to_vec(), false).into();
    if made.truncated() || made.bytes() != b"hi" {
        return refuse(Section::Outside, "a whole output reports itself whole");
    }
    let stub = JsonModel;
    let decoded: Json<Report> = driver.block_on(ask(
        &scope,
        "outside-ask-json",
        &stub,
        &AskRequest::default(),
    ))?;
    if decoded.value.code != 200 {
        return refuse(Section::Outside, "JSON decodes into the declared report");
    }
    say("outside: a direct ask decodes through the trait door")?;

    // The combined flow names every word once, so the entry admits all three
    // grants in one pass and the plan carries each word with its capability.
    let mailer = Arc::new(Mailer::new());
    let combined = task(
        "outside-all",
        |scope: Scope, input: (String, StubModel, Arc<Mailer>, Vec<u8>)| async move {
            let (url, model, mailer, payload) = input;
            smoke_all(&scope, &url, &model, &mailer, payload).await
        },
    )?;
    let payload = b"release-42".to_vec();
    let all = driver.block_on(with_effect_policy(
        AllowOutside,
        host.run(
            &combined,
            (url.clone(), model.clone(), Arc::clone(&mailer), payload),
        ),
    ));
    let Some(combined) = all.output() else {
        return refuse(Section::Outside, "the combined flow answers");
    };
    if combined.0 != 200 || combined.1.as_bytes() != model.output() {
        return refuse(Section::Outside, "the combined flow runs fetch and ask");
    }
    if !matches!(combined.2, SendOutcome::Settled(EffectEvidence::Applied)) {
        return refuse(Section::Outside, "the combined flow settles its send");
    }
    if mailer.calls() != 1 {
        return refuse(Section::Outside, "the combined flow dispatches once");
    }
    say("outside: fetch, ask and send run under one admission")?;

    // The entry half, called directly: every word the flow names is admitted
    // in one pass before its first step.
    let admitting = task(
        "outside-admit",
        |scope: Scope, words: Vec<OutsideWord>| async move {
            admit_outside(&scope, &words)?;
            Ok::<(), FlowError>(())
        },
    )?;
    let admitted = driver.block_on(host.run(
        &admitting,
        vec![OutsideWord::Fetch, OutsideWord::Ask, OutsideWord::Send],
    ));
    if admitted.error().is_some() {
        return refuse(Section::Outside, "the entry admits every word at once");
    }
    say("outside: the entry admits every word at once")?;

    // A POST hands an effect out, so under the allowing policy it runs; the
    // typed POST body travels beside the options it was built from.
    let posting = task("outside-post", |scope: Scope, url: String| async move {
        let posted: PostBody = PostBody::new("text/plain", b"hook".to_vec());
        let options: FetchOptions = FetchOptions::default().post(posted);
        smoke_fetch(&scope, &url, &options).await
    })?;
    let posted = driver.block_on(with_effect_policy(AllowOutside, host.run(&posting, url)));
    if posted.output() != Some(&200_u16) {
        return refuse(Section::Outside, "an allowed POST answers its status");
    }

    // A send dispatches once and settles from the receiver's evidence; the
    // typed outcome travels beside the payload it was sent with, through the
    // direct call as well as the word.
    let sending = task(
        "outside-send",
        |scope: Scope, input: (Arc<Mailer>, Vec<u8>)| async move {
            let (mailer, payload) = input;
            smoke_send(&scope, &mailer, payload).await
        },
    )?;
    let sent = driver.block_on(with_effect_policy(
        AllowOutside,
        host.run(&sending, (Arc::clone(&mailer), b"hello".to_vec())),
    ));
    let Some(sent_outcome) = sent.output() else {
        return refuse(Section::Outside, "a word send settles");
    };
    if !matches!(*sent_outcome, SendOutcome::Settled(EffectEvidence::Applied)) {
        return refuse(Section::Outside, "a word send settles on the evidence");
    }
    let direct_send = task(
        "outside-send-direct",
        |scope: Scope, input: (Arc<Mailer>, Vec<u8>)| async move {
            let (mailer, payload) = input;
            let outcome: SendOutcome = send(&scope, "direct", mailer.as_ref(), &payload).await?;
            Ok::<SendOutcome, FlowError>(outcome)
        },
    )?;
    let direct_sent = driver.block_on(with_effect_policy(
        AllowOutside,
        host.run(&direct_send, (Arc::clone(&mailer), b"direct".to_vec())),
    ));
    let Some(direct_outcome) = direct_sent.output() else {
        return refuse(Section::Outside, "a direct send settles");
    };
    void_outcome(direct_outcome)?;
    say("outside: a word send settles on the evidence")?;

    // The plan exposes every outbound reach: the map carries each outside
    // word with the capability it declares.
    let mut kinds = Vec::new();
    for flow in ARCHITECTURE.flows() {
        for step in flow.steps() {
            kinds.push((step.kind(), step.subject().to_owned()));
        }
    }
    use lgwks_bot::script::StepKind;
    for (kind, capability) in [
        (StepKind::Fetch, "bot.net"),
        (StepKind::Ask, "bot.model"),
        (StepKind::Send, "bot.notify"),
    ] {
        if !kinds
            .iter()
            .any(|entry: &(StepKind, String)| entry.0 == kind && entry.1 == capability)
        {
            return refuse(
                Section::Outside,
                "the plan names the word and its capability",
            );
        }
    }
    if upstream.hits() != 4 {
        return refuse(Section::Outside, "four exchanges dialled four times");
    }
    upstream.join(driver)?;
    say("outside: the plan names every word and its capability")?;
    Ok(())
}

/// Name a settled-or-unknown outcome without copying the test helper.
///
/// A smoke that held no branch for `Unknown` would pass while the word it
/// proves stopped answering it, so the honest arm is matched here.
fn void_outcome(outcome: &SendOutcome) -> Result<(), VocabularyError> {
    match *outcome {
        SendOutcome::Settled(EffectEvidence::Applied) => Ok(()),
        _ => refuse(Section::Outside, "a direct send settles applied"),
    }
}
