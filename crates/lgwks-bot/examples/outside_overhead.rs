//! Matched-workload overhead: what each outside word costs over the raw call.
//!
//! Three measurements over the same loopback server, the same [`StubModel`]
//! and the same stub mailer, so a difference in the numbers is a difference
//! in what each mechanism does rather than in what it talked to:
//!
//! 1. `fetch` — the [`script::fetch`](lgwks_bot::script::fetch) word through a
//!    real [`Host`] run against the word's raw [`Client`] exchange;
//! 2. `ask` — the [`script::ask`](lgwks_bot::script::ask) word through a real
//!    run against the model's raw [`ModelPort`] completion plus its decode;
//! 3. `send` — the [`script::send`](lgwks_bot::script::send) word through a
//!    real run against the mailer's raw [`SendPort`] dispatch.
//!
//! Each iteration measures the raw call and then the word back to back, and
//! the sample is the difference, so a slow minute moves both arms together
//! rather than into the overhead. This is a measurement harness, not a
//! pass/fail gate: wall time varies with the machine, the shape of the
//! distribution does not.
//!
//! ```text
//! cargo run -p lgwks_bot --features script,ephemeral,time,net --example outside_overhead
//! ```
//!
//! The output is written through `std::io::Write` rather than `println!`
//! because `clippy::print_stdout` is forbidden workspace-wide with no
//! example-only carve-out.

use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use lgwks_bot::journal::EffectEvidence;
use lgwks_bot::proposal::StubModel;
use lgwks_bot::script::{
    AskRequest, EffectDisposition, EffectPolicy, FetchOptions, FlowError, ModelPort, Scope,
    SendOutcome, SendPort,
};
use lgwks_bot::task::{Host, task};
use lgwks_bot::verb::EffectLifetime;
use lgwks_bot::{Cap, GrantSet};

use lgwks_std::http::{BodyPolicy, Client, Options, PoolConfig};

#[path = "support/measure.rs"]
mod measure;
#[path = "../tests/support/scratch.rs"]
mod scratch;

use scratch::Scratch;

/// How many overhead samples each word reports.
const SAMPLES: usize = 1_200;

/// The body every loopback answer carries, so the word and the raw call agree
/// on what they read.
const BODY: &[u8] = b"hello";

/// The one outcome a benchmark dispatch lands.
fn applied(evidence: EffectEvidence) -> Result<(), String> {
    match evidence {
        EffectEvidence::Applied => Ok(()),
        _ => {
            let refusal = Err("the benchmark dispatch was not applied".to_owned());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "applied: the dispatch landed nowhere");
            refusal
        }
    }
}

/// The stub mailer the benchmark sends through: a single slot holding the
/// last landing, overwritten by the next dispatch.
struct BenchMailer {
    /// Dispatches attempted, whatever they reported.
    calls: AtomicUsize,
    /// The last landing, whatever key it arrived under.
    last: Mutex<Option<(Vec<u8>, EffectEvidence)>>,
}

impl BenchMailer {
    /// A mailer holding nothing.
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            last: Mutex::new(None),
        }
    }
}

impl SendPort for BenchMailer {
    /// The stub never fails: the benchmark draws no faults.
    type Error = FlowError;

    /// The last landing when its key matches, or nothing.
    async fn find(&self, key: &[u8]) -> Result<Option<EffectEvidence>, FlowError> {
        let held = match self.last.lock() {
            Ok(held) => held,
            Err(_) => {
                let refusal = Err(FlowError::failed("the bench mailer lock failed"));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "bench mailer: the read-back failed");
                return refusal;
            }
        };
        match held.as_ref() {
            Some(landed) if landed.0.as_slice() == key => Ok(Some(landed.1)),
            Some(_) | None => Ok(None),
        }
    }

    /// Hand over under `key`, overwriting whatever landed before.
    async fn dispatch(&self, key: &[u8], _payload: &[u8]) -> Result<EffectEvidence, FlowError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut held = match self.last.lock() {
            Ok(held) => held,
            Err(_) => {
                let refusal = Err(FlowError::failed("the bench mailer lock failed"));
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "bench mailer: the dispatch failed");
                return refusal;
            }
        };
        *held = Some((key.to_vec(), EffectEvidence::Applied));
        Ok(EffectEvidence::Applied)
    }

    /// The stub never reports an unknown landing.
    fn may_have_landed(_error: &FlowError) -> bool {
        false
    }
}

/// The benchmark's explicit policy: the stub mailer is a test double, so an
/// allow runs it without parking a decision.
struct AllowBench;

impl EffectPolicy for AllowBench {
    /// Every effect is reversible: the benchmark owns its doubles.
    fn disposition(&self, _lifetime: EffectLifetime) -> EffectDisposition {
        EffectDisposition::Reversible
    }
}

/// Report one word's overhead distribution.
fn report(label: &str, samples: &mut [u128]) {
    let summary = measure::Summary::of(samples);
    let mut out = std::io::stdout().lock();
    let _written = writeln!(out, "{}", summary.line(&format!("{label} overhead")));
}

/// Run the benchmark.
///
/// Cancels the bench server when `run` returns before joining it: the stop
/// flag goes up and one dial wakes the parked `accept`, so the server task
/// ends instead of outliving the run that started it.
struct CancelOnDrop {
    /// The port the server listens on.
    port: u16,
    /// Raised to end the server before its connections arrive.
    stop: std::sync::Arc<AtomicBool>,
    /// Whether the server is still unjoined, so a drop must cancel it.
    armed: bool,
}

impl CancelOnDrop {
    /// The server was joined, so there is nothing left to cancel.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelOnDrop {
    /// Raise the flag and wake the parked `accept`.
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.stop.store(true, Ordering::SeqCst);
        if let Err(failed) = std::net::TcpStream::connect(("127.0.0.1", self.port)) {
            lgwks_std::trace::debug!(error = ?failed, "bench server: it had already ended");
        }
    }
}

/// # Errors
///
/// Whatever the host, the loopback server, the store or a mismatched answer
/// reports. A sample that did not take the measured path is an error rather
/// than a dropped sample.
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let scratch = Scratch::new("outside-overhead")?;
    let host = Host::builder("bench")?
        .run_store(scratch.path())?
        .grants(
            GrantSet::empty()
                .grant(Cap::net())
                .grant(Cap::new("bot.model"))
                .grant(Cap::notify()),
        )
        .build()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}/");
    let bodies = SAMPLES.saturating_mul(2);
    let mut cancel = CancelOnDrop {
        port,
        stop: std::sync::Arc::new(AtomicBool::new(false)),
        armed: true,
    };
    let stop = std::sync::Arc::clone(&cancel.stop);
    let server = lgwks_std::task::try_spawn_blocking(move || {
        for _ in 0..bodies {
            let mut stream = match listener.accept() {
                Ok(accepted) => accepted.0,
                Err(failed) => {
                    lgwks_std::trace::debug!(error = ?failed, "bench server: accept failed");
                    return;
                }
            };
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let mut head = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                let read = match std::io::Read::read(&mut stream, &mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                head.extend_from_slice(&chunk[..read]);
                if head.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
                if head.len() > 16_384 {
                    break;
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                BODY.len()
            );
            drop(std::io::Write::write_all(&mut stream, head.as_bytes()));
            drop(std::io::Write::write_all(&mut stream, BODY));
        }
    })
    .map_err(|refused| format!("no blocking thread for the bench server: {refused}"))?;

    let exchange = Options::default()
        .timeout(Duration::from_secs(30))
        .deadline(Duration::from_secs(30))
        .max_body_bytes(1024 * 1024)
        .body_policy(BodyPolicy::Whole);
    let client = Client::with_pool(
        exchange,
        PoolConfig::default()
            .max_idle_connections(4)
            .max_idle_connections_per_host(2),
    )
    .map_err(|refused| format!("the raw client could not be built: {refused}"))?;

    let fetch_word = task("bench-fetch", |scope: Scope, url: String| async move {
        let options: FetchOptions = FetchOptions::default();
        let answered = lgwks_bot::script::fetch(&scope, "fetch", &url, &options).await?;
        Ok::<u16, FlowError>(answered.status())
    })?;

    let mut fetch_overhead = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let at = Instant::now();
        let call = client.options().clone();
        let raw = client
            .get_with(&url, &call)
            .map_err(|failed| format!("the raw exchange failed: {failed}"))?;
        if raw.status != 200 || raw.body() != BODY {
            let message = format!(
                "the raw exchange answered {} with {} bytes",
                raw.status,
                raw.body().len()
            );
            lgwks_std::trace::debug!(message = message.as_str(), "run: the raw fetch strayed");
            return Err(message.into());
        }
        let raw_micros = at.elapsed().as_micros();
        let at = Instant::now();
        let worded = lgwks_bot::block_on(host.run(&fetch_word, url.clone()));
        let word_micros = at.elapsed().as_micros();
        if worded.output() != Some(&200_u16) {
            let message = format!("the word fetch failed: {:?}", worded.error());
            lgwks_std::trace::debug!(message = message.as_str(), "run: the word fetch strayed");
            return Err(message.into());
        }
        fetch_overhead.push(word_micros.saturating_sub(raw_micros));
    }
    report("fetch", &mut fetch_overhead);

    let mut seed = 0_u64;
    let model = loop {
        let candidate = StubModel::from_seed(seed);
        let probe = lgwks_bot::block_on(ModelPort::complete(&candidate, b"hi", 1_000))
            .map_err(|failed| format!("the probe completion failed: {failed}"))?;
        if !probe.truncated() {
            break candidate;
        }
        seed = seed.saturating_add(1);
        if seed > 64 {
            let message = "no seed answered whole output".to_owned();
            lgwks_std::trace::debug!(message = message.as_str(), "run: the model search failed");
            return Err(message.into());
        }
    };
    let ask_word = task("bench-ask", |scope: Scope, model: StubModel| async move {
        let request: AskRequest = AskRequest::default();
        lgwks_bot::script::ask(&scope, "ask", &model, &request).await
    })?;

    let mut ask_overhead = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let prompt = b"steady".to_vec();
        let at = Instant::now();
        let raw = lgwks_bot::block_on(model.complete(&prompt, 1_000))
            .map_err(|failed| format!("the raw completion failed: {failed}"))?;
        if raw.truncated() {
            let message = "the raw output was truncated".to_owned();
            lgwks_std::trace::debug!(message = message.as_str(), "run: the raw ask strayed");
            return Err(message.into());
        }
        let raw_text = std::str::from_utf8(raw.bytes())
            .map_err(|failed| format!("the raw output was not text: {failed}"))?;
        let raw_micros = at.elapsed().as_micros();
        let at = Instant::now();
        let worded = lgwks_bot::block_on(host.run(&ask_word, model.clone()));
        let word_micros = at.elapsed().as_micros();
        if worded.output().map(String::as_str) != Some(raw_text) {
            let message = format!("the word ask failed: {:?}", worded.error());
            lgwks_std::trace::debug!(message = message.as_str(), "run: the word ask strayed");
            return Err(message.into());
        }
        ask_overhead.push(word_micros.saturating_sub(raw_micros));
    }
    report("ask", &mut ask_overhead);

    use std::sync::Arc as Shared;
    let mailer = Shared::new(BenchMailer::new());
    let send_word = task(
        "bench-send",
        |scope: Scope, input: (Shared<BenchMailer>, Vec<u8>)| async move {
            let (mailer, payload) = input;
            let outcome: SendOutcome =
                lgwks_bot::script::send(&scope, "send", mailer.as_ref(), &payload).await?;
            Ok::<SendOutcome, FlowError>(outcome)
        },
    )?;

    let mut send_overhead = Vec::with_capacity(SAMPLES);
    for index in 0..SAMPLES {
        let key = format!("bench-{index}").into_bytes();
        let payload = format!("payload-{index}").into_bytes();
        let at = Instant::now();
        let raw = lgwks_bot::block_on(mailer.dispatch(&key, &payload))
            .map_err(|failed| format!("the raw dispatch failed: {failed}"))?;
        applied(raw).map_err(|failed| format!("the raw dispatch failed: {failed}"))?;
        let raw_micros = at.elapsed().as_micros();
        let at = Instant::now();
        let worded = lgwks_bot::block_on(lgwks_bot::script::with_effect_policy(
            AllowBench,
            host.run(&send_word, (Shared::clone(&mailer), payload)),
        ));
        let word_micros = at.elapsed().as_micros();
        if !matches!(
            worded.output(),
            Some(SendOutcome::Settled(EffectEvidence::Applied))
        ) {
            let message = format!("the word send failed: {:?}", worded.error());
            lgwks_std::trace::debug!(message = message.as_str(), "run: the word send strayed");
            return Err(message.into());
        }
        send_overhead.push(word_micros.saturating_sub(raw_micros));
    }
    report("send", &mut send_overhead);

    lgwks_std::task::block_on(server);
    cancel.disarm();
    let mut out = std::io::stdout().lock();
    let _written = writeln!(out, "samples per word: {SAMPLES}");
    Ok(())
}

/// The benchmark's entry: the numbers go to stdout, the refusal to stderr.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}
